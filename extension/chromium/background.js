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
  const {capture}=await api.storage.local.get('capture');
  if(!capture || !/^https?:\/\//i.test(item.finalUrl||item.url))return;
  // The browser keeps its download if the desktop handoff fails.
  try {await send(item.finalUrl||item.url);await api.downloads.cancel(item.id);}catch(e){report(e);}
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
