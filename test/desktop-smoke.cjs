// Run with Node 22+ and PLAYWRIGHT_MODULE pointing to an installed playwright package.
const {_electron}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const os=require('node:os');
const http=require('node:http');
const {createHash}=require('node:crypto');
(async()=>{
  const userData=fs.mkdtempSync(path.join(os.tmpdir(),'relay-desktop-'));
  const fixture=Buffer.alloc(2*1024*1024,67);
  const server=http.createServer((req,res)=>{
    const m=/bytes=(\d+)-(\d+)/.exec(req.headers.range||'');const start=m?+m[1]:0,end=m?+m[2]:fixture.length-1;
    const headers={'Content-Length':end-start+1,ETag:'"smoke"','Content-Type':'application/octet-stream'};if(m)headers['Content-Range']=`bytes ${start}-${end}/${fixture.length}`;
    res.writeHead(m?206:200,headers);let pos=start;const timer=setInterval(()=>{if(pos>end){clearInterval(timer);res.end();return;}const next=Math.min(pos+8192,end+1);res.write(fixture.subarray(pos,next));pos=next;},8);res.on('close',()=>clearInterval(timer));
  });
  await new Promise(r=>server.listen(0,'127.0.0.1',r));
  let app;
  try{
    app=await _electron.launch({executablePath:process.env.RELAY_EXECUTABLE||path.resolve(__dirname,'../node_modules/electron/dist/electron.exe'),args:[...(process.env.RELAY_EXECUTABLE?[]:[path.resolve(__dirname,'..')]),`--user-data-dir=${userData}`]});
    const page=await app.firstWindow();const errors=[];page.on('pageerror',e=>errors.push(e.message));
    await page.waitForFunction(()=>document.querySelector('#stat-segments').textContent.includes('6'));
    // Keep test output in the isolated profile.
    await app.evaluate(async({dialog},folder)=>{dialog.showOpenDialog=async()=>({canceled:false,filePaths:[folder]});},path.join(userData,'downloads'));
    await page.getByRole('button',{name:'Settings & integration'}).click();await page.getByRole('button',{name:'Change',exact:true}).click();
    await page.getByRole('button',{name:'All downloads'}).click();
    await page.screenshot({path:path.resolve(__dirname,'../preview-empty.png')});
    await page.getByRole('button',{name:'New download'}).click();
    await page.locator('#add-form [name=url]').fill(`http://127.0.0.1:${server.address().port}/fixture.zip`);
    await page.locator('#add-form [name=name]').fill('Desktop test.zip');
    await page.locator('#add-form [name=paused]').check();
    await page.getByRole('button',{name:'Start download'}).click();
    await page.getByRole('button',{name:'Resume Desktop test.zip',exact:true}).click();
    await page.waitForFunction(()=>document.querySelector('.status-label')?.textContent==='Downloading');
    await page.screenshot({path:path.resolve(__dirname,'../preview-active.png')});
    await page.getByRole('button',{name:'Pause Desktop test.zip',exact:true}).click();
    await page.getByRole('button',{name:'Resume Desktop test.zip',exact:true}).click();
    await page.waitForFunction(()=>document.querySelector('.status-label')?.textContent==='Completed');
    const state=await page.evaluate(()=>window.relay.state());assert.equal(state.jobs.length,1);assert.equal(state.jobs[0].status,'completed');
    const digest=b=>createHash('sha256').update(b).digest('hex');assert.equal(digest(fs.readFileSync(state.jobs[0].filePath)),digest(fixture));
    await page.locator('#search').fill('does-not-exist');await page.waitForFunction(()=>!document.querySelector('#empty').hidden);await page.locator('#search').fill('');
    await page.getByRole('button',{name:'Set up integration'}).click();await page.locator('#settings-form [name=bandwidthKB]').fill('512');await page.getByRole('button',{name:'Save preferences'}).click();
    await page.screenshot({path:path.resolve(__dirname,'../preview-settings.png')});
    const updated=await page.evaluate(()=>window.relay.state());assert.equal(updated.settings.bandwidthKB,512);assert.equal(await page.evaluate(()=>typeof window.require),'undefined');
    assert.deepEqual(errors,[]);console.log('Desktop smoke passed: add, pause, resume, hash, search, settings, renderer isolation.');
  }finally{if(app)await app.close();await new Promise(r=>server.close(r));fs.rmSync(userData,{recursive:true,force:true});}
})().catch(e=>{console.error(e);process.exitCode=1;});
