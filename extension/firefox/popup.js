const api=globalThis.browser||chrome;
const $=s=>document.querySelector(s);
const endpoint='http://127.0.0.1:17864';
function status(text){$('#status').textContent=text;}
async function remote(route,body){
  if(body){const reply=await api.runtime.sendMessage({type:'relay-download',...body});if(!reply?.ok)throw new Error(reply?.error||'Download handoff failed.');return reply;}
  const {token}=await api.storage.local.get('token');
  const res=await fetch(endpoint+route,{method:body?'POST':'GET',headers:{Authorization:`Bearer ${token||''}`,...(body?{'Content-Type':'application/json'}:{})},...(body?{body:JSON.stringify(body)}:{}),signal:AbortSignal.timeout(body?120000:5000)});
  const data=await res.json();if(!res.ok)throw new Error(data.error);return data;
}
async function send(url,button){
  button.disabled=true;
  status('Choose a download folder in the Relay desktop app.');
  try{if(!/^https?:\/\//i.test(url))throw new Error('Paste a direct HTTP(S) URL.');await remote('/downloads',{url});status('Added to your Relay download queue.');button.textContent='✓ Sent';}
  catch(e){status(e.message.includes('fetch')?'Could not connect. Open Relay and check the pairing token.':e.message);}
  finally{button.disabled=false;}
}
async function refresh(){
  $('#media').replaceChildren();
  const [tab]=await api.tabs.query({active:true,currentWindow:true});if(!tab)return;
  const key=`media:${tab.id}`;const saved=await api.storage.local.get(key);let media=saved[key]||[];
  try{
    const results=await api.scripting.executeScript({target:{tabId:tab.id},func:()=>Array.from(document.querySelectorAll('video,audio,video source,audio source')).map(el=>({url:el.currentSrc||el.src,type:el.type||el.tagName.toLowerCase()})).filter(m=>/^https?:\/\//i.test(m.url))});
    media=[...media,...(results[0]?.result||[])];
  }catch{}
  media=media.filter((m,i,all)=>all.findIndex(x=>x.url===m.url)===i);
  if(!media.length){const el=document.createElement('p');el.className='empty';el.textContent='No direct media detected yet.';$('#media').append(el);}
  for(const m of media){
    const row=document.createElement('div');row.className='media-item';const detail=document.createElement('div');const name=document.createElement('strong');
    try{name.textContent=decodeURIComponent(new URL(m.url).pathname.split('/').pop())||'Media file';}catch{name.textContent='Media file';}
    name.title=m.url;const type=document.createElement('small');type.textContent=m.type;detail.append(name,type);const button=document.createElement('button');button.textContent='↓';button.title='Download this media';button.onclick=()=>send(m.url,button);row.append(detail,button);$('#media').append(row);
  }
}
$('#connect').onclick=async()=>{await api.storage.local.set({token:$('#token').value.trim()});try{await remote('/health');status('Connected to Relay. Ready to download.');await api.storage.local.remove('lastError');api.action.setBadgeText({text:''});}catch{status('Could not connect. Open Relay and check your token.');}};
$('#capture').onchange=()=>api.storage.local.set({capture:$('#capture').checked});
$('#send').onclick=()=>send($('#url').value.trim(),$('#send'));$('#refresh').onclick=refresh;
(async()=>{const saved=await api.storage.local.get(['token','capture','lastError']);$('#token').value=saved.token||'';$('#capture').checked=!!saved.capture;if(saved.lastError)status(saved.lastError);else if(saved.token){try{await remote('/health');status('Connected to Relay. Ready to download.');}catch{status('Relay is offline. Open the desktop app to connect.');}}await refresh();})();
