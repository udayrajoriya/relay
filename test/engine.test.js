const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const os=require('node:os');
const path=require('node:path');
const http=require('node:http');
const {createHash}=require('node:crypto');
const {spawn}=require('node:child_process');
const {Engine,safeName,validURL}=require('../src/engine');
const {startBridge}=require('../src/bridge');
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
const hash=data=>createHash('sha256').update(data).digest('hex');
const payload=Buffer.alloc(3*1024*1024+137);for(let i=0;i<payload.length;i++)payload[i]=(i*31+i%251)%256;
async function until(fn,timeout=12000){const start=Date.now();while(!fn()){if(Date.now()-start>timeout)throw new Error('Timed out');await sleep(20);}}
async function fixture(t){
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'relay-test-'));let version='"v1"';let requests=[];let inFlight=0,maxConcurrent=0;let broken=false;
  const server=http.createServer((req,res)=>{
    if(req.url==='/empty'){res.writeHead(req.headers.range?416:200,req.headers.range?{'Content-Range':'bytes */0'}:{'Content-Length':0});res.end();return;}
    const route=req.url;let data=route==='/small'?payload.subarray(0,128*1024):payload;
    if(route==='/redirect'){res.writeHead(302,{Location:'/file'});res.end();return;}
    if(route==='/error'){res.writeHead(403);res.end();return;}
    const range=/bytes=(\d+)-(\d+)/.exec(req.headers.range||'');requests.push({route,range:range?{start:+range[1],end:+range[2]}:null,ifRange:req.headers['if-range']});
    const partial=range&&route!=='/plain'&&route!=='/unknown'&&!(route==='/liar'&&range[2]!=='0')&&(!req.headers['if-range']||req.headers['if-range']===version);
    const start=partial?+range[1]:0,end=partial?Math.min(+range[2],data.length-1):data.length-1;
    const headers={'Content-Type':'application/octet-stream','ETag':version,'Content-Disposition':"attachment; filename*=UTF-8''test%20file.zip"};
    if(route!=='/unknown')headers['Content-Length']=end-start+1;
    if(partial)headers['Content-Range']=`bytes ${start}-${end}/${data.length}`;
    res.writeHead(partial?206:200,headers);
    inFlight++;maxConcurrent=Math.max(inFlight,maxConcurrent);let position=start;
    const timer=setInterval(()=>{
      if(route==='/flaky'&&range&&range[2]!=='0'&&!broken&&position>start+32000){broken=true;res.destroy();return;}
      if(position>end){clearInterval(timer);res.end();return;}
      const next=Math.min(position+16384,end+1);res.write(data.subarray(position,next));position=next;
    },3);
    res.on('close',()=>{clearInterval(timer);inFlight--;});
  });
  await new Promise(r=>server.listen(0,'127.0.0.1',r));
  const base=`http://127.0.0.1:${server.address().port}`;
  let engines=[];
  const create=()=>{const e=new Engine({dataDir:path.join(root,'data'),downloadDir:path.join(root,'out')});engines.push(e);return e;};
  t.after(async()=>{for(const e of engines)await e.close();await new Promise(r=>server.close(r));fs.rmSync(root,{recursive:true,force:true});});
  return {root,base,create,requests,setVersion:v=>version=v,maxConcurrent:()=>maxConcurrent};
}
async function finish(e,id){await until(()=>['completed','error'].includes(e.get(id).status));const j=e.get(id);assert.equal(j.status,'completed',j.error);return j;}

test('segmented download, redirect, organization, and SHA-256 integrity',async t=>{
  const f=await fixture(t),e=f.create();const id=e.add({url:f.base+'/redirect'});const j=await finish(e,id);
  assert.equal(hash(fs.readFileSync(j.filePath)),hash(payload));assert.match(j.filePath,/Archives/);assert.equal(j.name,'test file.zip');assert.ok(f.maxConcurrent()>1);assert.ok(j.parts.length>6);
});
test('pause and resume uses existing bytes and never overwrites a completed file',async t=>{
  const f=await fixture(t),e=f.create();const id=e.add({url:f.base+'/file',name:'same.zip'});
  await until(()=>e.get(id).received>200000);await e.action(id,'pause');const count=e.get(id).received;assert.ok(count>0&&count<payload.length);
  const partial=e.get(id).parts.find(p=>p.done>0&&p.done<p.end-p.start+1);const resumeFrom=partial.start+partial.done;const before=f.requests.length;
  await e.action(id,'resume');const j=await finish(e,id);assert.equal(hash(fs.readFileSync(j.filePath)),hash(payload));assert.ok(f.requests.slice(before).some(r=>r.range?.start===resumeFrom));
  const next=await finish(e,e.add({url:f.base+'/file',name:'same.zip'}));assert.notEqual(next.filePath,j.filePath);assert.ok(fs.existsSync(j.filePath));
});
test('custom destination holds all partial bytes and survives changed defaults and restart',async t=>{
  const f=await fixture(t),e=f.create();const destination=path.join(f.root,'chosen folder');
  const id=e.add({url:f.base+'/file',destination});
  await until(()=>e.get(id).received>200000);await e.action(id,'pause');
  const j=e.get(id);assert.equal(j.destination,destination);
  assert.ok(fs.existsSync(path.join(destination,'.relay-parts',id)));
  assert.equal(fs.existsSync(path.join(f.root,'data','parts')),false);
  assert.equal(fs.existsSync(path.join(f.root,'out')),false);
  const partial=j.parts.find(p=>p.done>0&&p.done<p.end-p.start+1),resumeFrom=partial.start+partial.done;
  e.updateSettings({downloadDir:path.join(f.root,'new default')});await e.close();
  const restored=f.create();assert.equal(restored.get(id).destination,destination);const before=f.requests.length;
  await restored.action(id,'resume');const done=await finish(restored,id);
  assert.ok(f.requests.slice(before).some(r=>r.range?.start===resumeFrom));
  assert.ok(done.filePath.startsWith(destination+path.sep));assert.equal(hash(fs.readFileSync(done.filePath)),hash(payload));
  assert.equal(fs.existsSync(path.join(destination,'.relay-parts')),false);
});
async function legacyDownload(f){
  const e=f.create();const id=e.add({url:f.base+'/file'});await until(()=>e.get(id).received>200000);await e.action(id,'pause');await e.close();
  const j=e.get(id),legacy=e.partsDir(j,true);fs.mkdirSync(legacy,{recursive:true});
  for(const p of j.parts){const source=e.partPath(j,p);if(fs.existsSync(source))fs.copyFileSync(source,path.join(legacy,`${p.start}.part`));}
  e.removeParts(j);
  const stateFile=path.join(f.root,'data','state.json'),saved=JSON.parse(fs.readFileSync(stateFile,'utf8'));
  delete saved.jobs[0].partsInDestination;fs.writeFileSync(stateFile,JSON.stringify(saved));
  return {id,legacy,stateFile,saved};
}
test('legacy profile partials migrate to the destination without losing resume progress',async t=>{
  const f=await fixture(t),old=await legacyDownload(f),e=f.create();
  const j=e.get(old.id),partial=j.parts.find(p=>p.done>0&&p.done<p.end-p.start+1),resumeFrom=partial.start+partial.done;
  // A previously interrupted copy must not replace the intact originals with stale bytes.
  fs.mkdirSync(e.partsDir(j),{recursive:true});fs.writeFileSync(path.join(e.partsDir(j),`${partial.start}.part`),'incomplete migration');
  const before=f.requests.length;await e.action(j.id,'resume');const done=await finish(e,j.id);
  assert.equal(done.partsInDestination,true);assert.equal(fs.existsSync(old.legacy),false);
  assert.ok(f.requests.slice(before).some(r=>r.range?.start===resumeFrom));assert.equal(hash(fs.readFileSync(done.filePath)),hash(payload));
});
test('failed legacy migration preserves the originals and can be retried',async t=>{
  const f=await fixture(t),old=await legacyDownload(f);const unavailable=path.join(f.root,'unavailable');fs.writeFileSync(unavailable,'blocks the destination');
  old.saved.jobs[0].destination=unavailable;fs.writeFileSync(old.stateFile,JSON.stringify(old.saved));
  const e=f.create(),j=e.get(old.id),p=j.parts.find(p=>p.done>0),source=path.join(old.legacy,`${p.start}.part`),original=hash(fs.readFileSync(source));
  await e.action(j.id,'resume');await until(()=>j.status==='error');assert.equal(j.partsInDestination,undefined);assert.equal(hash(fs.readFileSync(source)),original);
  fs.unlinkSync(unavailable);fs.mkdirSync(unavailable);await e.action(j.id,'resume');const done=await finish(e,j.id);assert.equal(hash(fs.readFileSync(done.filePath)),hash(payload));
});
test('removing a partial download keeps other partials and completed files in its destination',async t=>{
  const f=await fixture(t),e=f.create(),destination=path.join(f.root,'shared');
  const a=e.get(e.add({url:f.base+'/file',destination,paused:true})),b=e.get(e.add({url:f.base+'/file',destination,paused:true}));
  for(const j of [a,b]){fs.mkdirSync(e.partsDir(j),{recursive:true});fs.writeFileSync(path.join(e.partsDir(j),'0.part'),'partial bytes');}
  const completed=path.join(destination,'keep.zip');fs.writeFileSync(completed,'existing completed file');
  await e.action(a.id,'remove');assert.equal(fs.existsSync(e.partsDir(a)),false);assert.ok(fs.existsSync(e.partsDir(b)));assert.equal(fs.readFileSync(completed,'utf8'),'existing completed file');
  await e.action(b.id,'remove');assert.equal(fs.existsSync(path.join(destination,'.relay-parts')),false);assert.ok(fs.existsSync(completed));
});
test('relative or unwritable per-download destinations are rejected before enqueueing',async t=>{
  const f=await fixture(t),e=f.create();assert.throws(()=>e.add({url:f.base+'/file',destination:'relative/path'}),/absolute/);
  const file=path.join(f.root,'file');fs.writeFileSync(file,'not a directory');assert.throws(()=>e.add({url:f.base+'/file',destination:file}));assert.equal(e.jobs.length,0);
});
test('saved state restores paused downloads after a process is forcibly killed',async t=>{
  const f=await fixture(t);const dataDir=path.join(f.root,'data'),downloadDir=path.join(f.root,'out');
  const script=`const {Engine}=require(${JSON.stringify(path.resolve(__dirname,'../src/engine'))});const e=new Engine(${JSON.stringify({dataDir,downloadDir})});e.updateSettings({bandwidthKB:1024});e.add({url:${JSON.stringify(f.base+'/file')}});e.on('change',s=>{if(s.jobs[0].received>200000&&s.jobs[0].status==='downloading')process.stdout.write('ready\\n')});`;
  const child=spawn(process.execPath,['-e',script],{stdio:['ignore','pipe','pipe']});t.after(()=>child.kill());
  await new Promise((resolve,reject)=>{child.stdout.once('data',resolve);child.once('error',reject);child.once('exit',code=>{if(code)reject(new Error('Fixture child exited '+code));});});
  const exited=new Promise(r=>child.once('exit',r));child.kill();await exited;
  const e=f.create();const j=e.jobs[0];assert.equal(j.status,'paused');assert.ok(j.received>0);await e.action(j.id,'resume');const done=await finish(e,j.id);assert.equal(hash(fs.readFileSync(done.filePath)),hash(payload));
});
test('changed validator discards stale segments before resuming',async t=>{
  const f=await fixture(t),e=f.create();const id=e.add({url:f.base+'/file'});await until(()=>e.get(id).received>200000);await e.action(id,'pause');f.setVersion('"v2"');const before=f.requests.length;await e.action(id,'resume');const j=await finish(e,id);assert.equal(j.validator,'"v2"');assert.equal(hash(fs.readFileSync(j.filePath)),hash(payload));assert.ok(f.requests.slice(before).some(r=>r.range?.start===0&&r.range?.end>0));
});
test('server without ranges restarts safely after pause',async t=>{
  const f=await fixture(t),e=f.create();const id=e.add({url:f.base+'/plain'});await until(()=>e.get(id).received>100000);await e.action(id,'pause');await e.action(id,'resume');const j=await finish(e,id);assert.equal(j.ranges,false);assert.equal(hash(fs.readFileSync(j.filePath)),hash(payload));
});
test('chunked unknown-size response completes with exact contents',async t=>{
  const f=await fixture(t),e=f.create();const j=await finish(e,e.add({url:f.base+'/unknown'}));assert.equal(j.size,payload.length);assert.equal(hash(fs.readFileSync(j.filePath)),hash(payload));
});
test('empty files complete even if range probe receives HTTP 416',async t=>{
  const f=await fixture(t),e=f.create();const j=await finish(e,e.add({url:f.base+'/empty'}));assert.equal(j.size,0);assert.equal(fs.statSync(j.filePath).size,0);
});
test('removing a bandwidth cap releases already throttled transfers',async t=>{
  const f=await fixture(t),e=f.create();e.updateSettings({bandwidthKB:1,connections:1});const id=e.add({url:f.base+'/small'});await until(()=>e.get(id).received>0);e.updateSettings({bandwidthKB:0});const now=Date.now();await finish(e,id);assert.ok(Date.now()-now<3000);
});
test('transient dropped connection retries its partial segment',async t=>{
  const f=await fixture(t),e=f.create();const j=await finish(e,e.add({url:f.base+'/flaky'}));assert.equal(hash(fs.readFileSync(j.filePath)),hash(payload));
});
test('range refusal and HTTP errors are visible, never corrupt completed files',async t=>{
  const f=await fixture(t),e=f.create();for(const route of ['/liar','/error']){const id=e.add({url:f.base+route});await until(()=>e.get(id).status==='error');assert.ok(e.get(id).error);assert.equal(e.get(id).filePath,undefined);}
});
test('scheduler waits, queues run in order, queued cancellation removes parts',async t=>{
  const f=await fixture(t),e=f.create();const start=Date.now()+1100;const a=e.add({url:f.base+'/small',scheduledAt:new Date(start).toISOString()});await sleep(150);assert.equal(e.get(a).status,'queued');assert.equal(f.requests.length,0);await finish(e,a);assert.ok(Date.parse(e.get(a).completedAt)>=start);
  const b=e.add({url:f.base+'/file',queue:'Ordered'}),c=e.add({url:f.base+'/small',queue:'Ordered'});await until(()=>e.get(b).status==='downloading');assert.equal(e.get(c).status,'queued');await finish(e,b);await finish(e,c);assert.ok(Date.parse(e.get(c).completedAt)>=Date.parse(e.get(b).completedAt));
  const d=e.add({url:f.base+'/file',paused:true});await e.action(d,'remove');assert.throws(()=>e.get(d));
});
test('global bandwidth limiting is applied',async t=>{
  const f=await fixture(t),e=f.create();e.updateSettings({bandwidthKB:128,connections:1});const now=Date.now();await finish(e,e.add({url:f.base+'/small'}));assert.ok(Date.now()-now>=850);
});
test('bridge rejects unauthenticated requests and validates download URLs',async t=>{
  const f=await fixture(t),e=f.create();const bridge=await startBridge(e,'test-token',0);t.after(()=>new Promise(r=>bridge.close(r)));const base=`http://127.0.0.1:${bridge.address().port}`;
  assert.equal((await fetch(base+'/health')).status,401);
  assert.equal((await fetch(base+'/health',{headers:{Authorization:'Bearer test-token'}})).status,200);
  const add=url=>fetch(base+'/downloads',{method:'POST',headers:{Authorization:'Bearer test-token'},body:JSON.stringify({url})});
  assert.equal((await add('file:///C:/Windows/system.ini')).status,400);const res=await add(f.base+'/small');assert.equal(res.status,201);await finish(e,(await res.json()).id);
});
test('Windows filename and URL guards',()=>{
  assert.equal(safeName('../../bad:thing?.exe'),'.._.._bad_thing_.exe');assert.equal(safeName('CON.txt'),'_CON.txt');assert.throws(()=>validURL('javascript:alert(1)'));assert.throws(()=>validURL('https://user:secret@example.org/file'));
});
test('browser additions choose a per-download folder and cancellation leaves the queue unchanged',async t=>{
  const f=await fixture(t),e=f.create();let selected=null,calls=0;const destination=path.join(f.root,'browser selected');
  const bridge=await startBridge(e,'folder-token',0,{selectDestination:async()=>{calls++;return selected;}});
  t.after(()=>new Promise(r=>bridge.close(r)));const base=`http://127.0.0.1:${bridge.address().port}`;
  const add=url=>fetch(base+'/downloads',{method:'POST',headers:{Authorization:'Bearer folder-token'},body:JSON.stringify({url})});
  assert.equal((await add('file:///invalid')).status,400);assert.equal(calls,0);
  assert.equal((await add(f.base+'/small')).status,409);assert.equal(e.jobs.length,0);
  selected=destination;const res=await add(f.base+'/small');assert.equal(res.status,201);const j=await finish(e,(await res.json()).id);
  assert.equal(j.destination,destination);assert.equal(e.settings.downloadDir,path.join(f.root,'out'));assert.ok(j.filePath.startsWith(destination+path.sep));
});
test('disconnected browser handoffs never enqueue after the folder picker resolves',async t=>{
  const f=await fixture(t),e=f.create();let release,opened=false;
  const bridge=await startBridge(e,'folder-token',0,{selectDestination:()=>{opened=true;return new Promise(resolve=>release=resolve);}});
  t.after(()=>new Promise(r=>bridge.close(r)));const controller=new AbortController();
  const pending=fetch(`http://127.0.0.1:${bridge.address().port}/downloads`,{method:'POST',headers:{Authorization:'Bearer folder-token'},body:JSON.stringify({url:f.base+'/small'}),signal:controller.signal}).catch(()=>{});
  await until(()=>opened);controller.abort();await pending;await sleep(50);release(path.join(f.root,'browser selected'));await sleep(50);assert.equal(e.jobs.length,0);
});
