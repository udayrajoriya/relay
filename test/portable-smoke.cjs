// Portable launchers do not forward Electron's debugger stdout. Attach via the profile's CDP port file instead.
const {chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const {spawn}=require('node:child_process');
const fs=require('node:fs');
const os=require('node:os');
const path=require('node:path');
const assert=require('node:assert/strict');
const {version}=require('../package.json');
const executable=process.env.RELAY_EXECUTABLE||path.resolve(__dirname,process.platform==='linux'?'../dist/linux-unpacked/relay':`../dist/Relay-${version}-Windows.exe`);
const wait=ms=>new Promise(r=>setTimeout(r,ms));
(async()=>{
  const profile=fs.mkdtempSync(path.join(os.tmpdir(),'relay-portable-'));
  const child=spawn(executable,[`--user-data-dir=${profile}`,'--remote-debugging-port=0'],{stdio:'ignore'});
  let browser,page;
  try{
    const portFile=path.join(profile,'DevToolsActivePort');
    for(let i=0;i<300&&!fs.existsSync(portFile);i++)await wait(100);
    assert.ok(fs.existsSync(portFile),'Packaged app did not start its renderer');
    const port=fs.readFileSync(portFile,'utf8').split(/\r?\n/)[0];
    browser=await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
    page=browser.contexts()[0].pages()[0];await page.waitForFunction(()=>window.relay&&document.querySelector('#stat-segments').textContent.includes('6'));
    await page.getByRole('button',{name:'New download'}).click();
    await page.locator('#add-form [name=url]').fill('https://example.com/portable-test.zip');
    const destination=path.join(profile,'chosen downloads');await page.locator('#add-form [name=destination]').fill(destination);
    await page.locator('#add-form [name=paused]').check();
    await page.getByRole('button',{name:'Start download'}).click();
    await page.getByRole('button',{name:'Resume portable-test.zip',exact:true}).waitFor();
    const state=await page.evaluate(()=>window.relay.state());assert.equal(state.jobs[0].status,'paused');assert.equal(state.jobs[0].destination,destination);
    assert.ok(fs.existsSync(path.join(state.extensionDir,'chromium','manifest.json')));
    assert.ok(fs.existsSync(path.join(state.extensionDir,'firefox','manifest.json')));
    await page.getByRole('button',{name:'Remove portable-test.zip',exact:true}).click();
    for(let i=0;i<100&&(await page.evaluate(()=>window.relay.state())).jobs.length;i++)await wait(50);
    assert.equal((await page.evaluate(()=>window.relay.state())).jobs.length,0);
    await page.close();page=null;
    console.log('Packaged executable passed: launch, renderer, IPC, queue, bundled browser extensions, clean exit.');
  }finally{
    if(page)await page.close().catch(()=>{});if(browser)await browser.close();
    for(let i=0;i<100&&child.exitCode===null;i++)await wait(100);
    if(child.exitCode===null)child.kill();
    fs.rmSync(profile,{recursive:true,force:true});
  }
})().catch(e=>{console.error(e);process.exitCode=1;});
