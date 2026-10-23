// Portable launchers do not forward Electron's debugger stdout. Attach via the profile's CDP port file instead.
const {chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const {spawn}=require('node:child_process');
const fs=require('node:fs');
const os=require('node:os');
const path=require('node:path');
const assert=require('node:assert/strict');
const wait=ms=>new Promise(r=>setTimeout(r,ms));
(async()=>{
  const profile=fs.mkdtempSync(path.join(os.tmpdir(),'relay-portable-'));
  const child=spawn(path.resolve(__dirname,'../dist/Relay-0.1.0-Windows.exe'),[`--user-data-dir=${profile}`,'--remote-debugging-port=0'],{stdio:'ignore'});
  let browser,page;
  try{
    const portFile=path.join(profile,'DevToolsActivePort');
    for(let i=0;i<300&&!fs.existsSync(portFile);i++)await wait(100);
    assert.ok(fs.existsSync(portFile),'Portable app did not start its renderer');
    const port=fs.readFileSync(portFile,'utf8').split(/\r?\n/)[0];
    browser=await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
    page=browser.contexts()[0].pages()[0];await page.waitForFunction(()=>window.relay&&document.querySelector('#stat-segments').textContent.includes('6'));
    await page.getByRole('button',{name:'New download'}).click();
    await page.locator('#add-form [name=url]').fill('https://example.com/portable-test.zip');
    await page.locator('#add-form [name=paused]').check();
    await page.getByRole('button',{name:'Start download'}).click();
    await page.getByRole('button',{name:'Resume portable-test.zip',exact:true}).waitFor();
    const state=await page.evaluate(()=>window.relay.state());assert.equal(state.jobs[0].status,'paused');
    assert.ok(fs.existsSync(path.join(state.extensionDir,'chromium','manifest.json')));
    assert.ok(fs.existsSync(path.join(state.extensionDir,'firefox','manifest.json')));
    await page.getByRole('button',{name:'Remove portable-test.zip',exact:true}).click();
    await page.waitForFunction(async()=>!(await window.relay.state()).jobs.length);
    await page.close();page=null;
    console.log('Portable executable passed: launch, renderer, IPC, queue, bundled browser extensions, clean exit.');
  }finally{
    if(page)await page.close().catch(()=>{});if(browser)await browser.close();
    for(let i=0;i<100&&child.exitCode===null;i++)await wait(100);
    if(child.exitCode===null)child.kill();
    fs.rmSync(profile,{recursive:true,force:true});
  }
})().catch(e=>{console.error(e);process.exitCode=1;});
