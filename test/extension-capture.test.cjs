const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const vm=require('node:vm');
const event=()=>({addListener(fn){this.listener=fn;}});
const tick=()=>new Promise(resolve=>setImmediate(resolve));
function harness(browser,{capture=true,failPause=false,failCancel=false,failResume=false}={}){
  const calls=[],errors=[];
  const state={paused:false,status:'in_progress'};
  let finish;
  const response=new Promise(resolve=>finish=resolve);
  const api={
    runtime:{onInstalled:event(),onMessage:event()},
    contextMenus:{create(){},onClicked:event()},
    storage:{local:{get:async()=>({capture,token:'test-token'}),set:async data=>errors.push(data.lastError)}},
    action:{setBadgeText(){},setBadgeBackgroundColor(){}},
    tabs:{onRemoved:event(),onUpdated:event()},webRequest:{onHeadersReceived:event()},
    downloads:{onCreated:event(),
      pause:async()=>{calls.push('pause');if(failPause)throw new Error('Not active');state.paused=true;},
      resume:async()=>{calls.push('resume');if(failResume)throw new Error('Resume failed');state.paused=false;},
      cancel:async()=>{calls.push('cancel');if(failCancel)throw new Error('Cancel failed');state.status='interrupted';state.paused=false;},
    },
  };
  const globals={AbortSignal,fetch:async()=>{calls.push('handoff');return response;}};
  globals[browser==='firefox'?'browser':'chrome']=api;
  vm.runInNewContext(fs.readFileSync(path.join(__dirname,`../extension/${browser}/background.js`),'utf8'),globals);
  return {calls,errors,state,run:item=>api.downloads.onCreated.listener({id:12,url:'https://example.org/file.zip',state:'in_progress',paused:false,...item}),accept:()=>finish({ok:true,json:async()=>({id:'relay-job'})}),reject:()=>finish({ok:false,json:async()=>({error:'Download canceled in Relay.'})})};
}
for(const browser of ['chromium','firefox']){
  test(`${browser}: browser stays paused while Relay waits for a folder, then is canceled`,async()=>{
    const h=harness(browser);const pending=h.run();await tick();
    assert.deepEqual(h.calls,['pause','handoff']);assert.equal(h.state.paused,true);
    h.accept();await pending;assert.deepEqual(h.calls,['pause','handoff','cancel']);assert.equal(h.state.status,'interrupted');
  });
  test(`${browser}: rejected handoff resumes the browser without canceling it`,async()=>{
    const h=harness(browser);const pending=h.run();await tick();h.reject();await pending;
    assert.deepEqual(h.calls,['pause','handoff','resume']);assert.equal(h.state.paused,false);assert.equal(h.state.status,'in_progress');
  });
  test(`${browser}: a download that cannot be paused is not also sent to Relay`,async()=>{
    const h=harness(browser,{failPause:true});await h.run();
    assert.deepEqual(h.calls,['pause']);assert.equal(h.state.paused,false);
  });
  test(`${browser}: disabled capture and unsupported URLs leave browser downloads untouched`,async()=>{
    const disabled=harness(browser,{capture:false});await disabled.run();assert.deepEqual(disabled.calls,[]);
    const unsupported=harness(browser);await unsupported.run({url:'blob:https://example.org/file'});assert.deepEqual(unsupported.calls,[]);
  });
  test(`${browser}: already completed or user-paused downloads are left alone`,async()=>{
    const complete=harness(browser);await complete.run({state:'complete'});assert.deepEqual(complete.calls,[]);
    const paused=harness(browser);await paused.run({paused:true});assert.deepEqual(paused.calls,[]);
  });
  test(`${browser}: cancellation errors after acceptance never resume a duplicate download`,async()=>{
    const h=harness(browser,{failCancel:true});const pending=h.run();await tick();h.accept();await pending;
    assert.deepEqual(h.calls,['pause','handoff','cancel']);assert.equal(h.state.paused,true);assert.match(h.errors[0],/Cancel failed/);
  });
  test(`${browser}: a failed browser resume is reported without losing the original error`,async()=>{
    const h=harness(browser,{failResume:true});const pending=h.run();await tick();h.reject();await pending;
    assert.match(h.errors[0],/Download canceled in Relay/);assert.match(h.errors[0],/resume/i);
  });
}
