const api=globalThis.browser||chrome;
const endpoint='http://127.0.0.1:17864';
api.runtime.onInstalled.addListener(()=>{
  api.contextMenus.create({id:'relay-link',title:'Download with Relay',contexts:['link','video','audio']});
});
async function send(url,name){
  if(!/^https?:\/\//i.test(url))throw new Error('Only direct HTTP(S) links are supported.');
  const {token}=await api.storage.local.get('token');
  const res=await fetch(endpoint+'/downloads',{method:'POST',headers:{'Content-Type':'application/json',Authorization:`Bearer ${token||''}`},body:JSON.stringify({url,name}),signal:AbortSignal.timeout(120000)});
  const data=await res.json();if(!res.ok)throw new Error(data.error);return data;
}
async function report(error){
  await api.storage.local.set({lastError:String(error.message||error)});
  api.action.setBadgeText({text:'!'});api.action.setBadgeBackgroundColor({color:'#ca6b6b'});
}
// A popup closes when the desktop folder dialog takes focus. Keep its handoff in
// the background so choosing a folder does not cancel the request.
api.runtime.onMessage.addListener((message,_sender,respond)=>{
  if(message?.type!=='relay-download')return false;
  send(message.url,message.name).then(data=>respond({ok:true,...data}),error=>{report(error);respond({ok:false,error:error.message});});
  return true;
});
api.contextMenus.onClicked.addListener(info=>{if(info.menuItemId==='relay-link')send(info.linkUrl||info.srcUrl).catch(report);});
api.downloads.onCreated.addListener(async item=>{
  let paused=false,accepted=false;
  try{
    const {capture}=await api.storage.local.get('capture');
    const url=item.finalUrl||item.url;
    if(!capture || !/^https?:\/\//i.test(url) || item.paused || item.state!=='in_progress')return;
    // Folder selection can take longer than the entire browser download. Pause
    // first; if that is already too late, keep the browser copy without handing
    // the same file to Relay as well.
    await api.downloads.pause(item.id);paused=true;
    await send(url);accepted=true;
    await api.downloads.cancel(item.id);
  }catch(error){
    let message=error.message||String(error);
    if(paused && !accepted){
      try{await api.downloads.resume(item.id);}
      catch{message+=' Resume the original download from your browser downloads if it is still paused.';}
    }
    // Once Relay has accepted, a cancel failure must not resume a second copy.
    if(!paused)message='The download was kept in your browser because capture could not pause it. '+message;
    if(accepted)message='Relay accepted this download. Keep its browser copy paused and cancel it in your browser downloads. '+message;
    await report(new Error(message));
  }
});
// Serialize storage updates so parallel media requests cannot overwrite each other.
let writes=Promise.resolve();
api.webRequest.onHeadersReceived.addListener(info=>{
  if(info.tabId<0 || info.statusCode>=400)return;
  const type=(info.responseHeaders||[]).find(h=>h.name.toLowerCase()==='content-type')?.value||'';
  if(!/^(video\/(mp4|webm|quicktime)|audio\/(mpeg|mp4|ogg|wav|flac|aac))/i.test(type))return;
  const key=`media:${info.tabId}`;
  writes=writes.then(async()=>{
    const saved=await api.storage.local.get(key);const items=saved[key]||[];
    if(!items.some(m=>m.url===info.url))items.push({url:info.url,type:type.split(';')[0]});
    await api.storage.local.set({[key]:items.slice(-30)});
    api.action.setBadgeText({tabId:info.tabId,text:String(items.length)});
    api.action.setBadgeBackgroundColor({tabId:info.tabId,color:'#7061e8'});
  }).catch(()=>{});
},{urls:['http://*/*','https://*/*']},['responseHeaders']);
api.tabs.onRemoved.addListener(id=>api.storage.local.remove(`media:${id}`));
api.tabs.onUpdated.addListener((id,change)=>{if(change.status==='loading'){api.storage.local.remove(`media:${id}`);api.action.setBadgeText({tabId:id,text:''});}});
