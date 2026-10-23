const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');
const https = require('node:https');
const { randomUUID } = require('node:crypto');
const { EventEmitter, setMaxListeners } = require('node:events');

function validURL(input) {
  const u = new URL(String(input));
  if (!['http:', 'https:'].includes(u.protocol) || u.username || u.password) throw new Error('Use an HTTP or HTTPS URL without embedded credentials.');
  return u.href;
}
function request(url, headers = {}, signal, redirects = 0) {
  return new Promise((resolve, reject) => {
    if (redirects > 8) return reject(new Error('Too many redirects.'));
    let u; try { u = validURL(url); } catch (e) { reject(e); return; }
    const req = (u.startsWith('https:') ? https : http).get(u, { signal, headers: { 'User-Agent': 'Relay/0.1', 'Accept-Encoding': 'identity', ...headers } }, res => {
      if ([301,302,303,307,308].includes(res.statusCode) && res.headers.location) {
        res.destroy();
        request(new URL(res.headers.location, u).href, headers, signal, redirects + 1).then(resolve, reject);
      } else resolve(res);
    });
    req.setTimeout(25000, () => req.destroy(new Error('Connection timed out.')));
    req.on('error', reject);
  });
}
function safeName(name) {
  let n = String(name || 'download').replace(/[<>:"/\\|?*\x00-\x1f]/g, '_').replace(/[. ]+$/g, '').slice(0, 180);
  if (!n || /^(con|prn|aux|nul|com[0-9]|lpt[0-9])(\.|$)/i.test(n)) n = '_' + (n || 'download');
  return n;
}
function category(name) {
  const ext = path.extname(name).toLowerCase();
  for (const [kind, extensions] of Object.entries({Videos:'.mp4 .webm .mkv .mov .avi .m4v', Music:'.mp3 .wav .flac .aac .m4a .ogg', Archives:'.zip .rar .7z .gz .tar .xz .iso', Documents:'.pdf .doc .docx .xls .xlsx .csv .txt .epub .pptx', Images:'.png .jpg .jpeg .gif .svg .webp .avif', Programs:'.exe .msi .msix .dmg .deb .apk'})) {
    if (extensions.split(' ').includes(ext)) return kind;
  }
  return 'Other';
}
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));

class Engine extends EventEmitter {
  constructor({dataDir, downloadDir}) {
    super();
    this.dataDir = dataDir; this.db = path.join(dataDir, 'state.json'); this.jobs = []; this.active = new Map(); this.closing = false;
    this.settings = { downloadDir, connections: 6, simultaneous: 2, bandwidthKB: 0, organize: true };
    fs.mkdirSync(dataDir, {recursive:true});
    if (fs.existsSync(this.db)) {
      const saved = JSON.parse(fs.readFileSync(this.db, 'utf8'));
      this.settings = {...this.settings, ...saved.settings};
      this.jobs = saved.jobs.map(j => ({...j, speed:0, status: ['downloading','probing','assembling'].includes(j.status) ? 'paused' : j.status}));
    }
    for (const j of this.jobs) this.recount(j);
    this.throttleAt = 0; this.throttleEpoch = 0;
    this.timer = setInterval(() => { this.persist(); this.emit('change', this.snapshot()); this.pump(); }, 750);
  }
  partsDir(j, legacy=false) {
    // Resolve and check the exact task directory before recursive deletion or migration.
    if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(j.id)) throw new Error('Invalid download ID.');
    const root=legacy ? path.resolve(this.dataDir,'parts') : path.resolve(j.destination,'.relay-parts');
    const target=path.resolve(root,j.id);
    if(path.dirname(target)!==root) throw new Error('Invalid partial download directory.');
    return target;
  }
  partPath(j, p) { return path.join(this.partsDir(j,!j.partsInDestination), `${p.start}.part`); }
  removeParts(j, legacy=false) {
    const dir=this.partsDir(j,legacy);
    fs.rmSync(dir,{recursive:true,force:true});
    // Remove only an empty container, leaving other downloads untouched.
    try {fs.rmdirSync(path.dirname(dir));} catch {}
  }
  async migrateParts(j) {
    if(j.partsInDestination) {this.removeParts(j,true);return;}
    const destination=this.partsDir(j);
    // Keep the original bytes until every copy is flushed and the new location is saved.
    // An interrupted or failed migration can therefore be retried from the originals.
    this.removeParts(j);
    fs.mkdirSync(destination,{recursive:true});
    for(const p of j.parts || []) {
      const source=path.join(this.partsDir(j,true),`${p.start}.part`);
      try {await fs.promises.access(source);} catch(e) {if(e.code==='ENOENT')continue;throw e;}
      const target=path.join(destination,`${p.start}.part`);
      await fs.promises.copyFile(source,target);
      const handle=await fs.promises.open(target,'r+');
      try {await handle.sync();} finally {await handle.close();}
    }
    j.partsInDestination=true;
    try {this.persist();} catch(e) {j.partsInDestination=false;throw e;}
    this.removeParts(j,true);
    this.recount(j);
  }
  recount(j) {
    if (j.status === 'completed') return;
    for (const p of j.parts || []) {
      try { p.done = fs.statSync(this.partPath(j,p)).size; } catch { p.done = 0; }
      if (p.end !== null && p.done > p.end - p.start + 1) p.done = 0;
    }
    j.received = (j.parts || []).reduce((sum,p) => sum + p.done, 0);
  }
  snapshot() { return { jobs: this.jobs.map(j => ({...j, parts:(j.parts||[]).map(p=>({...p})), connections: this.active.get(j.id)?.workers || 0})), settings:{...this.settings} }; }
  persist() {
    fs.writeFileSync(this.db + '.tmp', JSON.stringify({settings:this.settings, jobs:this.jobs}, null, 2));
    fs.renameSync(this.db + '.tmp', this.db);
  }
  changed() { this.persist(); this.emit('change', this.snapshot()); }
  add(input) {
    const url = validURL(input.url);
    if (this.jobs.filter(j=>j.status !== 'completed').length >= 500) throw new Error('The queue is full (500 unfinished downloads).');
    const connections = Math.max(1, Math.min(16, Number(input.connections) || this.settings.connections));
    const scheduledAt = input.scheduledAt ? new Date(input.scheduledAt).toISOString() : null;
    const requestedDirectory=input.destination ?? this.settings.downloadDir;
    if(typeof requestedDirectory!=='string' || !path.isAbsolute(requestedDirectory.trim())) throw new Error('Choose an absolute download directory.');
    const destination=path.resolve(requestedDirectory.trim());
    fs.mkdirSync(destination,{recursive:true});
    fs.accessSync(destination,fs.constants.W_OK);
    let name; try { name = decodeURIComponent(new URL(url).pathname.split('/').pop()); } catch { name = 'download'; }
    const j = {id:randomUUID(), url, name:safeName(input.name || name), customName:!!input.name, queue:String(input.queue || 'Main queue').slice(0,80), scheduledAt, maxConnections:connections, status:input.paused ? 'paused' : 'queued', size:null, received:0, speed:0, parts:[], createdAt:new Date().toISOString(), error:null, category:'Other', destination, partsInDestination:true, organize:this.settings.organize};
    this.jobs.push(j); this.changed(); this.pump(); return j.id;
  }
  get(id) { const j = this.jobs.find(j=>j.id===id); if (!j) throw new Error('Download not found.'); return j; }
  async action(id, action) {
    const j = this.get(id);
    if (action === 'pause') {
      if (['completed','error','paused'].includes(j.status)) return;
      j.status = 'paused'; const running=this.active.get(id); running?.controller.abort(); j.speed = 0;
      if (running) await running.promise;
    } else if (action === 'resume') {
      if (['paused','error'].includes(j.status)) { j.status = 'queued'; j.error = null; j.scheduledAt = null; }
    } else if (action === 'remove') {
      if (this.active.has(id)) {
        j.status = 'paused'; this.active.get(id).controller.abort();
        await this.active.get(id).promise;
      }
      this.removeParts(j); this.removeParts(j,true);
      this.jobs = this.jobs.filter(x=>x.id!==id);
    } else if (action === 'up') {
      const i = this.jobs.indexOf(j); if (i > 0) [this.jobs[i-1],this.jobs[i]] = [j,this.jobs[i-1]];
    } else throw new Error('Unknown action.');
    this.changed(); this.pump();
  }
  updateSettings(s) {
    for (const [key,min,max] of [['connections',1,16],['simultaneous',1,8],['bandwidthKB',0,1048576]]) {
      if (s[key] !== undefined) { const v = Number(s[key]); if (!Number.isFinite(v) || v < min || v > max) throw new Error(`Invalid ${key}.`); this.settings[key] = Math.floor(v); }
    }
    if (typeof s.organize === 'boolean') this.settings.organize = s.organize;
    if (s.downloadDir) { fs.mkdirSync(s.downloadDir, {recursive:true}); this.settings.downloadDir = path.resolve(s.downloadDir); }
    this.throttleAt = Date.now(); this.throttleEpoch++; this.changed(); this.pump();
  }
  pump() {
    if (this.closing) return;
    for (const j of this.jobs) {
      if (this.active.size >= this.settings.simultaneous) break;
      if (j.status !== 'queued' || this.active.has(j.id) || (j.scheduledAt && Date.parse(j.scheduledAt)>Date.now())) continue;
      // A named queue is ordered: only one file from each queue runs at a time.
      if ([...this.active.keys()].some(id=>this.get(id).queue === j.queue)) continue;
      const state = {controller:new AbortController(), workers:0};
      setMaxListeners(64,state.controller.signal);
      this.active.set(j.id,state);
      state.promise = this.run(j,state).catch(e=>{
        if (j.status !== 'paused') { j.status='error'; j.error=e.message; }
      }).finally(()=>{ j.speed=0; this.active.delete(j.id); this.changed(); this.pump(); });
    }
  }
  async throttle(bytes, signal) {
    const rate = this.settings.bandwidthKB * 1024;
    if (!rate) return;
    const now = Date.now(); this.throttleAt = Math.max(now, this.throttleAt) + bytes / rate * 1000;
    const until = this.throttleAt, epoch = this.throttleEpoch;
    while (Date.now() < until) {
      if (signal.aborted) throw new Error('Paused.');
      if (epoch !== this.throttleEpoch) return;
      // Each chunk reserves its own slot; new reservations must not prolong this one.
      await delay(Math.min(100, until-Date.now()));
      if (Date.now() >= until) return;
    }
  }
  async probe(j, signal) {
    const res = await request(j.url, {Range:'bytes=0-0'}, signal);
    const h = res.headers; const status = res.statusCode; res.on('error',()=>{}); res.destroy();
    if (status===416 && h['content-range']==='bytes */0') return {size:0,ranges:false,validator:null,disposition:h['content-disposition']};
    if (![200,206].includes(status)) throw new Error(`Server returned HTTP ${status}.`);
    const match = /^bytes 0-0\/(\d+)$/.exec(h['content-range'] || '');
    const size = status === 206 && match ? Number(match[1]) : h['content-length'] !== undefined ? Number(h['content-length']) : null;
    if (size !== null && (!Number.isSafeInteger(size) || size < 0)) throw new Error('Invalid file size from server.');
    const validator = h.etag && !h.etag.startsWith('W/') ? h.etag : h['last-modified'] || null;
    return { size, ranges:status === 206 && !!match && h['content-encoding'] !== 'gzip', validator, disposition:h['content-disposition'] };
  }
  async run(j,state) {
    const signal = state.controller.signal;
    j.status='probing'; j.error=null; this.emit('change',this.snapshot());
    await this.migrateParts(j);
    if(signal.aborted) throw new Error('Paused.');
    const info = await this.probe(j,signal);
    if (signal.aborted) throw new Error('Paused.');
    if (!j.customName && info.disposition) {
      const encoded = /filename\*=UTF-8''([^;]+)/i.exec(info.disposition);
      const plain = /filename="([^"]+)"|filename=([^;]+)/i.exec(info.disposition);
      try { if (encoded) j.name=safeName(decodeURIComponent(encoded[1])); else if (plain) j.name=safeName((plain[1]||plain[2]).trim()); } catch {}
    }
    const canKeep = j.parts.length && info.ranges && info.validator && info.validator === j.validator && info.size === j.size;
    if (!canKeep) {
      this.removeParts(j);
      j.parts=[]; j.received=0;
    }
    fs.mkdirSync(this.partsDir(j), {recursive:true});
    j.size=info.size; j.ranges=info.ranges; j.validator=info.validator; j.category=category(j.name);
    if (!j.parts.length) {
      if (info.ranges && j.size > 0) {
        // Workers take the next available segment as they finish, balancing fast and slow connections.
        const chunk = Math.max(256*1024, Math.ceil(j.size/(j.maxConnections*8)));
        for (let start=0; start<j.size; start+=chunk) j.parts.push({start,end:Math.min(j.size-1,start+chunk-1),done:0});
      } else j.parts=[{start:0,end:j.size === null ? null : j.size-1,done:0}];
    }
    this.recount(j); j.status='downloading'; this.changed();
    let tick = Date.now(), last = j.received;
    const meter = setInterval(()=>{const now=Date.now(); j.speed=Math.max(0,(j.received-last)*1000/(now-tick));last=j.received;tick=now;},750);
    const pending=j.parts.filter(p=>j.size===0 || p.end === null || p.done < p.end-p.start+1);
    let index=0, failure;
    const worker=async()=>{
      state.workers++;
      try {
        while (index<pending.length && !signal.aborted && !failure) {
          const part=pending[index++];
          for (let attempt=0;attempt<3;attempt++) {
            try { await this.transfer(j,part,signal); break; }
            catch(e) { if (signal.aborted || attempt===2 || e.noRetry) throw e; await delay(350*(attempt+1)); }
          }
        }
      } catch(e) { failure ||= e; state.controller.abort(); }
      finally { state.workers--; }
    };
    try { await Promise.all(Array.from({length:info.ranges?Math.min(j.maxConnections,pending.length):1},worker)); }
    finally { clearInterval(meter); }
    if (failure) throw failure;
    if (signal.aborted) throw new Error('Paused.');
    j.status='assembling'; this.changed();
    await this.assemble(j,signal);
    j.status='completed'; j.completedAt=new Date().toISOString(); j.received=j.size; this.changed();
    this.removeParts(j);
    this.emit('complete', {...j});
  }
  async transfer(j,p,signal) {
    if (j.ranges && p.end !== null && p.done === p.end-p.start+1) return;
    if (!j.ranges && p.done) { j.received-=p.done; p.done=0; }
    const start=p.start+p.done;
    const headers=j.ranges ? {Range:`bytes=${start}-${p.end}`, ...(j.validator?{'If-Range':j.validator}:{})} : {};
    const res=await request(j.url,headers,signal);
    const fail=message=>{res.on('error',()=>{});res.destroy();const e=new Error(message);e.noRetry=true;throw e;};
    if (j.ranges) {
      if (res.statusCode!==206 || res.headers['content-range']!==`bytes ${start}-${p.end}/${j.size}`) fail('Server changed or ignored a range. Resume to safely restart this file.');
      const current=res.headers.etag && !res.headers.etag.startsWith('W/') ? res.headers.etag : res.headers['last-modified'];
      if (j.validator && current && current!==j.validator) fail('The remote file changed. Resume to restart safely.');
    } else if (res.statusCode!==200) fail(`Server returned HTTP ${res.statusCode}.`);
    if (res.headers['content-encoding'] && res.headers['content-encoding']!=='identity') fail('Server sent compressed transfer data despite requesting identity encoding.');
    const fd=fs.openSync(this.partPath(j,p),p.done?'a':'w');
    try {
      for await (const buf of res) {
        if (signal.aborted) throw new Error('Paused.');
        if (p.end!==null && p.done+buf.length>p.end-p.start+1) throw new Error('Server sent more data than expected.');
        let offset=0; while(offset<buf.length) offset+=fs.writeSync(fd,buf,offset,buf.length-offset);
        p.done+=buf.length; j.received+=buf.length;
        await this.throttle(buf.length,signal);
      }
      if (p.end!==null && p.done!==p.end-p.start+1) throw new Error('Connection closed before the segment finished.');
      if (j.size===null) {j.size=p.done;p.end=p.done-1;}
    } finally {res.destroy();fs.fsyncSync(fd);fs.closeSync(fd);}
  }
  async assemble(j,signal) {
    const folder=path.join(j.destination,j.organize?j.category:''); fs.mkdirSync(folder,{recursive:true});
    const staging=path.join(folder,`.relay-${j.id}.tmp`);
    const output=await fs.promises.open(staging,'w');
    let assemblyError;
    try {
      for (const p of j.parts) {
        for await (const buf of fs.createReadStream(this.partPath(j,p))) {
          if (signal.aborted) throw new Error('Paused.');
          let offset=0;while(offset<buf.length){const {bytesWritten}=await output.write(buf,offset,buf.length-offset);offset+=bytesWritten;}
        }
      }
      await output.sync();
    } catch(e) {assemblyError=e;} finally {await output.close();}
    if(assemblyError){await fs.promises.rm(staging,{force:true});throw assemblyError;}
    try {
      for(let n=0;;n++) {
        const ext=path.extname(j.name); const name=n?`${path.basename(j.name,ext)} (${n})${ext}`:j.name;
        const dest=path.join(folder,name);
        try {
          // Same-volume hard links publish the fully flushed file atomically on NTFS.
          try {await fs.promises.link(staging,dest);}
          catch(e) {
            if (!['EPERM','ENOTSUP','EOPNOTSUPP','ENOSYS'].includes(e.code)) throw e;
            await fs.promises.copyFile(staging,dest,fs.constants.COPYFILE_EXCL);
          }
          j.filePath=dest;break;
        }
        catch(e) {if(e.code!=='EEXIST') throw e;}
      }
    } finally {await fs.promises.rm(staging,{force:true});}
  }
  async close() {
    this.closing=true;clearInterval(this.timer);
    for (const [id,s] of this.active) {this.get(id).status='paused';s.controller.abort();}
    await Promise.all([...this.active.values()].map(s=>s.promise));this.persist();
  }
}
module.exports={Engine,validURL,safeName,category};
