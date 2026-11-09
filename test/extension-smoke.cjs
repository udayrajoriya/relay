const {chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const os=require('node:os');
const path=require('node:path');
const http=require('node:http');
const {spawn}=require('node:child_process');
const readline=require('node:readline');
(async()=>{
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'relay-extension-'));
  const executable=process.env.RELAY_TEST_HOST||path.resolve(__dirname,`../target/debug/examples/integration_host${process.platform==='win32'?'.exe':''}`);
  const host=spawn(executable,[root],{stdio:['pipe','pipe','inherit']});
  const lines=readline.createInterface({input:host.stdout})[Symbol.asyncIterator]();
  const exited=new Promise(resolve=>host.once('exit',resolve));
  await new Promise((resolve,reject)=>{host.once('error',reject);host.once('spawn',resolve);});
  const ready=(await lines.next()).value;assert.match(ready,/^ready \d+$/);
  const endpoint=`http://127.0.0.1:${ready.split(' ')[1]}`;
  const state=async()=>{host.stdin.write('state\n');return JSON.parse((await lines.next()).value);};
  const data=Buffer.alloc(256*1024,71);
  const fixture=http.createServer((req,res)=>{
    if(req.url==='/'){res.setHeader('Content-Type','text/html');res.end('<!doctype html><a download href="/file.zip">Download file</a><video src="/clip.mp4" preload="auto"></video>');return;}
    const m=/bytes=(\d+)-(\d+)/.exec(req.headers.range||'');const start=m?+m[1]:0,end=m?Math.min(+m[2],data.length-1):data.length-1;
    const headers={'Content-Length':end-start+1,ETag:'"extension-fixture"','Content-Type':req.url==='/clip.mp4'?'video/mp4':'application/zip'};
    if(req.url==='/file.zip')headers['Content-Disposition']='attachment; filename="file.zip"';
    if(m)headers['Content-Range']=`bytes ${start}-${end}/${data.length}`;
    res.writeHead(m?206:200,headers);let pos=start;const timer=setInterval(()=>{if(pos>end){clearInterval(timer);res.end();return;}const next=Math.min(pos+8192,end+1);res.write(data.subarray(pos,next));pos=next;},10);res.on('close',()=>clearInterval(timer));
  });
  await new Promise(r=>fixture.listen(0,'127.0.0.1',r));const base=`http://127.0.0.1:${fixture.address().port}`;
  let context;
  try{
    // Only the bridge address changes in this isolated copy. This lets the
    // actual companion run against the Rust fixture without touching a user's Relay.
    const ext=path.join(root,'extension');fs.cpSync(path.resolve(__dirname,'../extension/chromium'),ext,{recursive:true});
    for(const name of ['background.js','popup.js']){const file=path.join(ext,name);const source=fs.readFileSync(file,'utf8');assert.ok(source.includes('http://127.0.0.1:17864'));fs.writeFileSync(file,source.replace('http://127.0.0.1:17864',endpoint));}
    context=await chromium.launchPersistentContext(path.join(root,'profile'),{headless:true,channel:'chromium',...(process.env.CHROMIUM_EXECUTABLE?{executablePath:process.env.CHROMIUM_EXECUTABLE}:{}),args:[`--disable-extensions-except=${ext}`,`--load-extension=${ext}`]});
    let [worker]=context.serviceWorkers();if(!worker)worker=await context.waitForEvent('serviceworker');
    const id=worker.url().split('/')[2];const popup=await context.newPage();await popup.goto(`chrome-extension://${id}/popup.html`);
    const errors=[];popup.on('pageerror',e=>errors.push(e.message));
    await popup.locator('#token').fill('extension-test-token');await popup.locator('#connect').click();await popup.waitForFunction(()=>document.querySelector('#status').textContent.includes('Connected'));
    await popup.locator('#url').fill(base+'/file.zip');await popup.locator('#send').click();await popup.waitForFunction(()=>document.querySelector('#status').textContent.includes('Added'));
    assert.equal((await state()).jobs.length,1);
    const mediaPage=await context.newPage();await mediaPage.goto(base+'/');
    await popup.waitForTimeout(500);
    await mediaPage.bringToFront();await popup.locator('#refresh').evaluate(el=>el.click());
    await popup.waitForFunction(()=>document.querySelector('.media-item strong')?.textContent==='clip.mp4');
    await popup.locator('.media-item button').click();await popup.waitForFunction(()=>document.querySelector('.media-item button').textContent==='✓ Sent');
    assert.ok((await state()).jobs.some(j=>j.url.endsWith('/clip.mp4')));
    await popup.locator('#capture').check();
    const until=async(fn,label)=>{for(let i=0;i<200;i++){const result=await fn();if(result)return result;await new Promise(r=>setTimeout(r,25));}throw new Error('Timed out: '+label);};
    await until(()=>worker.evaluate(async()=>!!(await chrome.storage.local.get('capture')).capture),'capture enabled');
    const downloads=()=>worker.evaluate(url=>chrome.downloads.search({url}),base+'/file.zip');
    const latestAfter=async id=>(await downloads()).filter(item=>item.id>id).sort((a,b)=>b.id-a.id)[0];
    await mediaPage.getByRole('link',{name:'Download file'}).click();
    const captured=await until(async()=>{const item=await latestAfter(-1);return item?.paused&&item;},'browser paused for folder selection');
    assert.equal(captured.state,'in_progress');assert.equal((await state()).jobs.length,2);
    await until(async()=>(await state()).jobs.length===3,'captured Relay job');
    await until(async()=>(await latestAfter(-1))?.state==='interrupted','browser download canceled after handoff');
    const canceled=await latestAfter(-1);assert.equal(fs.existsSync(canceled.filename),false);
    // A canceled native folder dialog must resume the original browser download.
    host.stdin.write('cancel-next\n');assert.equal((await lines.next()).value,'ok');
    await mediaPage.getByRole('link',{name:'Download file'}).click();
    const declined=await until(async()=>{const item=await latestAfter(captured.id);return item?.paused&&item;},'browser paused before canceling folder dialog');
    await until(async()=>(await latestAfter(captured.id))?.state==='complete','browser resumes after canceled handoff');
    assert.equal((await state()).jobs.length,3);
    // Authentication failure must also leave a working browser download, not a stranded pause.
    await worker.evaluate(()=>chrome.storage.local.set({token:'wrong-token'}));
    await mediaPage.getByRole('link',{name:'Download file'}).click();
    await until(async()=>(await latestAfter(declined.id))?.state==='complete','browser resumes after failed pairing');
    assert.equal((await state()).jobs.length,3);
    await worker.evaluate(()=>chrome.storage.local.set({token:'extension-test-token'}));
    assert.deepEqual(errors,[]);
    // Choosing a destination in the desktop app closes the browser popup. The
    // service worker must continue its request even after that popup disappears.
    await popup.locator('#url').fill(base+'/file.zip');await popup.locator('#send').click();await popup.close();
    for(let i=0;i<100&&(await state()).jobs.length<4;i++)await new Promise(r=>setTimeout(r,50));
    assert.equal((await state()).jobs.length,4);
    console.log('Chromium extension passed: pairing, media, capture pauses and cancels the browser copy, canceled/failed handoffs resume it, popup-close handoff.');
  }finally{if(context)await context.close();host.stdin.write('close\n');await exited;await new Promise(r=>fixture.close(r));fs.rmSync(root,{recursive:true,force:true});}
})().catch(e=>{console.error(e);process.exitCode=1;});
