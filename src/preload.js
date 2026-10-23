const {contextBridge,ipcRenderer}=require('electron');
contextBridge.exposeInMainWorld('relay',{
  state:()=>ipcRenderer.invoke('state'),
  add:input=>ipcRenderer.invoke('add',input),
  action:(id,action)=>ipcRenderer.invoke('action',id,action),
  settings:input=>ipcRenderer.invoke('settings',input),
  chooseFolder:()=>ipcRenderer.invoke('choose-folder'),
  showFile:id=>ipcRenderer.invoke('show-file',id),
  openFolder:()=>ipcRenderer.invoke('open-folder'),
  copyToken:()=>ipcRenderer.invoke('copy-token'),
  extensionFolder:()=>ipcRenderer.invoke('extension-folder'),
  clipboard:()=>ipcRenderer.invoke('clipboard'),
  onUpdate:callback=>{const listener=(_,state)=>callback(state);ipcRenderer.on('update',listener);return()=>ipcRenderer.removeListener('update',listener);}
});
