const $=selector=>document.querySelector(selector);
let state={jobs:[],settings:{}},view='all',filter='all',toastTimer;
const names={all:'All downloads',active:'In progress',scheduled:'Scheduled',completed:'Completed',paused:'Paused & failed',settings:'Settings & integration'};
const active=j=>['downloading','probing','assembling'].includes(j.status);
const escape=s=>String(s??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
function bytes(value){let n=Number(value)||0,i=0;const units=['B','KB','MB','GB','TB'];while(n>=1024&&i<4){n/=1024;i++;}return `${n.toFixed(i&&n<100?1:0)} ${units[i]}`;}
function statBytes(value,suffix=''){const [n,u]=bytes(value).split(' ');return `${n} <small>${u}${suffix}</small>`;}
function eta(j){if(!j.speed||!j.size)return '—';const t=Math.ceil((j.size-j.received)/j.speed);return t<60?`${t}s left`:t<3600?`${Math.ceil(t/60)}m left`:`${Math.floor(t/3600)}h ${Math.ceil(t%3600/60)}m left`;}
function toast(message){$('#toast').textContent=message;$('#toast').hidden=false;clearTimeout(toastTimer);toastTimer=setTimeout(()=>$('#toast').hidden=true,4200);}
async function call(fn){try{return await fn();}catch(e){toast(e.message.replace(/^Error invoking remote method '[^']+': Error: /,''));}}
function matches(j){
  if(view==='active'&&!active(j))return false;
  if(view==='scheduled'&&!(j.status==='queued'&&j.scheduledAt&&Date.parse(j.scheduledAt)>Date.now()))return false;
  if(view==='completed'&&j.status!=='completed')return false;
  if(view==='paused'&&!['paused','error'].includes(j.status))return false;
  if(!['all','active','scheduled','completed','paused','settings'].includes(view)&&j.category!==view)return false;
  if(filter==='active'&&!active(j))return false;
  if(filter==='queued'&&j.status!=='queued')return false;
  if(filter==='completed'&&j.status!=='completed')return false;
  const q=$('#search').value.toLowerCase();if(q&&!`${j.name} ${j.url} ${j.queue}`.toLowerCase().includes(q))return false;
  return !$('#queue-filter').value||j.queue===$('#queue-filter').value;
}
function row(j){
  const pct=j.size?Math.min(100,j.received/j.size*100):j.status==='completed'?100:0;
  const scheduled=j.status==='queued'&&j.scheduledAt&&Date.parse(j.scheduledAt)>Date.now();
  const labels={queued:scheduled?'Scheduled':'Queued',probing:'Connecting',downloading:'Downloading',assembling:'Finishing up',completed:'Completed',paused:'Paused',error:'Needs attention'};
  const icons={Videos:'▶',Music:'♫',Archives:'ZIP',Documents:'DOC',Images:'IMG',Programs:'APP',Other:'FILE'};
  let host;try{host=new URL(j.url).hostname;}catch{host='';}
  const error=j.error?escape(j.error):'';
  const date=scheduled?new Date(j.scheduledAt).toLocaleString():null;
  const statusMeta=error||date||`${escape(j.queue)} · ${escape(host)}`;
  let controls='';
  if(active(j)||j.status==='queued')controls+=`<button class="icon-button" data-action="pause" title="Pause download" aria-label="Pause ${escape(j.name)}">Ⅱ</button>`;
  if(['paused','error'].includes(j.status))controls+=`<button class="icon-button" data-action="resume" title="Resume download" aria-label="Resume ${escape(j.name)}">▷</button>`;
  if(j.status==='completed')controls+=`<button class="icon-button" data-action="show" title="Show in folder" aria-label="Show ${escape(j.name)} in folder">↗</button>`;
  if(j.status==='queued')controls+=`<button class="icon-button" data-action="up" title="Move up in queue" aria-label="Move up in queue">↑</button>`;
  controls+=`<button class="icon-button remove" data-action="remove" title="Remove from list (keeps completed file)" aria-label="Remove ${escape(j.name)}">×</button>`;
  return `<tr data-id="${j.id}"><td><div class="file-cell"><div class="file-icon ${escape(j.category)}">${icons[j.category]||'FILE'}</div><div class="file-text"><div class="file-name" title="${escape(j.name)}">${escape(j.name)}</div><div class="file-meta" title="${statusMeta}">${statusMeta}</div></div></div></td><td><div class="progress-heading"><span class="status-label ${j.status}">${labels[j.status]||escape(j.status)}</span><span>${pct.toFixed(0)}%</span></div><div class="progress-track ${j.status}"><div class="progress-fill" data-progress="${pct.toFixed(2)}"></div></div><div class="segments">${(j.parts||[]).slice(0,24).map(p=>`<i class="${p.end!==null&&p.done>=p.end-p.start+1?'done':''}"></i>`).join('')}</div></td><td><div class="cell-primary">${j.size===null?'Unknown':bytes(j.size)}</div><div class="cell-secondary">${bytes(j.received)} saved</div></td><td><div class="cell-primary">${j.speed?bytes(j.speed)+'/s':'—'}</div><div class="cell-secondary">${eta(j)}</div></td><td><div class="row-actions">${controls}</div></td></tr>`;
}
function render(){
  const jobs=state.jobs, running=jobs.filter(active), completed=jobs.filter(j=>j.status==='completed');
  $('#all-count').textContent=jobs.length;$('#active-count').textContent=running.length;$('#tab-count').textContent=jobs.length;
  $('#stat-speed').innerHTML=statBytes(running.reduce((s,j)=>s+j.speed,0),'/s');$('#stat-active').innerHTML=`${running.length} <small>files</small>`;
  $('#stat-total').innerHTML=statBytes(completed.reduce((s,j)=>s+j.received,0));$('#total-foot').textContent=`${completed.length} files completed`;
  $('#stat-segments').innerHTML=`${state.settings.connections} <small>connections</small>`;
  $('#active-foot').textContent=`${running.reduce((s,j)=>s+j.connections,0)} active connections`;
  $('#speed-foot').textContent=running.length?'Transfers in motion':'Ready when you are';
  $('#bandwidth-footer').textContent=state.settings.bandwidthKB?`Bandwidth capped at ${bytes(state.settings.bandwidthKB*1024)}/s`:'Unlimited bandwidth';
  const selectedQueue=$('#queue-filter').value;const queues=[...new Set(jobs.map(j=>j.queue))];
  $('#queue-filter').innerHTML='<option value="">All queues</option>'+queues.map(q=>`<option value="${escape(q)}">${escape(q)}</option>`).join('');
  $('#queue-filter').value=queues.includes(selectedQueue)?selectedQueue:'';
  $('#queue-names').innerHTML=queues.map(q=>`<option value="${escape(q)}">`).join('');
  let list=jobs.filter(matches);const sort=$('#sort').value;
  if(sort==='newest')list.reverse();else if(sort==='size')list.sort((a,b)=>(b.size||0)-(a.size||0));else if(sort==='name')list.sort((a,b)=>a.name.localeCompare(b.name));
  $('#downloads').innerHTML=list.map(row).join('');
  // Inline widths are set through the DOM to keep CSP free of unsafe-inline.
  document.querySelectorAll('[data-progress]').forEach(el=>el.style.setProperty('--progress',`${el.dataset.progress}%`));
  $('#empty').hidden=list.length>0;
  if(!list.length){$('#empty h2').textContent=jobs.length?'Nothing here just yet':'Your next download starts here';$('#empty p').innerHTML=jobs.length?'Try a different filter or add a new download.':'Drop in a link. Relay takes care of the rest.<br>Fast transfers, saved progress, a tidier downloads folder.';$('#empty-add').textContent=jobs.length?'＋ Add a download':'＋ Add your first download';}
  $('#visible-count').textContent=`${list.length} of ${jobs.length} downloads`;
  $('#save-path').textContent=state.settings.downloadDir;$('#bridge-status').textContent=state.bridgeStatus;
}
function setView(next){view=next;filter='all';$('#search').value='';$('#queue-filter').value='';document.querySelectorAll('[data-view]').forEach(el=>el.classList.toggle('selected',el.dataset.view===view));document.querySelectorAll('[data-filter]').forEach(el=>el.classList.toggle('selected',el.dataset.filter===filter));$('#breadcrumb').textContent=names[view]||view;$('#page-title').textContent=view==='all'?'Your downloads.':view==='settings'?'A little more you.':(names[view]||view)+'.';$('#page-subtitle').textContent=view==='settings'?'Fine-tune your downloads. Connect your browser.':'Everything you need. Moving at your speed.';$('#downloads-view').hidden=view==='settings';$('#settings-view').hidden=view!=='settings';if(view==='settings'){for(const key of ['connections','simultaneous','bandwidthKB'])$('#settings-form').elements[key].value=state.settings[key];$('#settings-form').elements.organize.checked=state.settings.organize;}render();}
function addDialog(){const form=$('#add-form');form.reset();form.elements.connections.value=state.settings.connections;form.elements.destination.value=state.settings.downloadDir;$('#add-error').textContent='';$('#add-dialog').showModal();form.elements.url.focus();}
$('#new-download').onclick=addDialog;$('#empty-add').onclick=addDialog;
$('#close-dialog').onclick=$('#cancel-dialog').onclick=()=>$('#add-dialog').close();
$('#paste-url').onclick=()=>call(async()=>{$('#add-form').elements.url.value=(await window.relay.clipboard()).trim();});
$('#browse-download-folder').onclick=()=>call(async()=>{const input=$('#add-form').elements.destination;const selected=await window.relay.chooseDownloadFolder(input.value);if(selected)input.value=selected;});
$('#add-form').onsubmit=async event=>{event.preventDefault();const form=event.target;const submit=form.querySelector('[type=submit]');submit.disabled=true;try{await window.relay.add({url:form.elements.url.value.trim(),name:form.elements.name.value.trim(),destination:form.elements.destination.value.trim(),queue:form.elements.queue.value.trim(),connections:Number(form.elements.connections.value),paused:form.elements.paused.checked,scheduledAt:form.elements.scheduledAt.value?new Date(form.elements.scheduledAt.value).toISOString():null});$('#add-dialog').close();toast('Download added to your queue.');}catch(e){$('#add-error').textContent=e.message;}finally{submit.disabled=false;}};
document.querySelectorAll('[data-view]').forEach(el=>el.onclick=()=>setView(el.dataset.view));
document.querySelectorAll('[data-filter]').forEach(el=>el.onclick=()=>{filter=el.dataset.filter;document.querySelectorAll('[data-filter]').forEach(t=>t.classList.toggle('selected',t===el));render();});
$('#search').oninput=render;$('#sort').onchange=render;$('#queue-filter').onchange=render;
$('#downloads').onclick=event=>{const button=event.target.closest('[data-action]');if(!button)return;const id=button.closest('[data-id]').dataset.id;button.disabled=true;call(()=>button.dataset.action==='show'?window.relay.showFile(id):window.relay.action(id,button.dataset.action));};
$('#pause-all').onclick=()=>call(async()=>{for(const j of state.jobs.filter(j=>active(j)||j.status==='queued'))await window.relay.action(j.id,'pause');toast('All downloads paused.');});
$('#resume-all').onclick=()=>call(async()=>{for(const j of state.jobs.filter(j=>['paused','error'].includes(j.status)))await window.relay.action(j.id,'resume');toast('Paused downloads returned to the queue.');});
$('#folder').onclick=()=>call(()=>window.relay.openFolder());$('#choose-folder').onclick=()=>call(()=>window.relay.chooseFolder());
$('#setup-extension').onclick=()=>setView('settings');$('#extension-folder').onclick=()=>call(()=>window.relay.extensionFolder());$('#copy-token').onclick=()=>call(async()=>{await window.relay.copyToken();toast('Pairing token copied. Paste it into the browser extension.');});
$('#settings-form').onsubmit=event=>{event.preventDefault();const form=event.target;call(async()=>{await window.relay.settings({connections:Number(form.elements.connections.value),simultaneous:Number(form.elements.simultaneous.value),bandwidthKB:Number(form.elements.bandwidthKB.value),organize:form.elements.organize.checked});toast('Preferences saved.');});};
document.addEventListener('keydown',e=>{if((e.ctrlKey||e.metaKey)&&e.key.toLowerCase()==='n'){e.preventDefault();if(!$('#add-dialog').open)addDialog();}if((e.ctrlKey||e.metaKey)&&e.key.toLowerCase()==='k'){e.preventDefault();if(view==='settings')setView('all');$('#search').focus();}});
window.relay.onUpdate(next=>{state=next;render();});call(async()=>{state=await window.relay.state();render();});
