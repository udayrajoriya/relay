const {app,BrowserWindow,ipcMain,dialog,shell,clipboard,Notification} = require('electron');
const path=require('node:path');
const fs=require('node:fs');
const {randomBytes}=require('node:crypto');
const {Engine}=require('./engine');
const {startBridge}=require('./bridge');
let win,engine,bridge,token,quitting=false,bridgeStatus='Starting';
if (!app.requestSingleInstanceLock()) app.quit();
else {
  app.on('second-instance',()=>{if(win){win.show();win.focus();}});
  app.whenReady().then(async()=>{
    const dataDir=app.getPath('userData');
    fs.mkdirSync(dataDir,{recursive:true});
    const tokenFile=path.join(dataDir,'browser-token');
    token=fs.existsSync(tokenFile)?fs.readFileSync(tokenFile,'utf8'):randomBytes(32).toString('hex');
    fs.writeFileSync(tokenFile,token);
    try {engine=new Engine({dataDir,downloadDir:path.join(app.getPath('downloads'),'Relay')});}
    catch(e){dialog.showErrorBox('Relay could not load its download history',`${e.message}\nYour state file is in ${dataDir}. Preserve it before attempting recovery.`);app.quit();return;}
    let browserFolderSelection=Promise.resolve();
    const selectBrowserDestination=isConnected=>{
      const select=async()=>{
        if(!isConnected())return null;
        if(!win || win.isDestroyed())throw new Error('Relay is still starting. Try again in a moment.');
        if(win.isMinimized())win.restore();win.show();win.focus();
        const result=await dialog.showOpenDialog(win,{title:'Choose a folder for this browser download',defaultPath:engine.settings.downloadDir,properties:['openDirectory','createDirectory']});
        return result.canceled?null:result.filePaths[0];
      };
      const pending=browserFolderSelection.then(select,select);browserFolderSelection=pending.catch(()=>{});return pending;
    };
    try {bridge=await startBridge(engine,token,17864,{selectDestination:selectBrowserDestination});bridgeStatus='Listening on 127.0.0.1:17864';}
    catch(e){bridgeStatus=`Unavailable: ${e.message}`;}
    const extensionDir=app.isPackaged?path.join(process.resourcesPath,'extension'):path.join(__dirname,'..','extension');
    ipcMain.handle('state',()=>({...engine.snapshot(),bridgeStatus,extensionDir}));
    ipcMain.handle('add',(_,input)=>engine.add(input));
    ipcMain.handle('action',(_,id,action)=>engine.action(id,action));
    ipcMain.handle('settings',(_,input)=>{const {downloadDir,...allowed}=input;engine.updateSettings(allowed);});
    ipcMain.handle('choose-folder',async()=>{const result=await dialog.showOpenDialog(win,{properties:['openDirectory','createDirectory']});if(!result.canceled)engine.updateSettings({downloadDir:result.filePaths[0]});});
    ipcMain.handle('choose-download-folder',async(_,current)=>{
      const result=await dialog.showOpenDialog(win,{title:'Choose a folder for this download',defaultPath:typeof current==='string'&&path.isAbsolute(current)?current:engine.settings.downloadDir,properties:['openDirectory','createDirectory']});
      return result.canceled?null:result.filePaths[0];
    });
    ipcMain.handle('show-file',(_,id)=>{const j=engine.get(id);if(j.filePath && fs.existsSync(j.filePath))shell.showItemInFolder(j.filePath);else throw new Error('The completed file is no longer at its saved location.');});
    ipcMain.handle('open-folder',async()=>{fs.mkdirSync(engine.settings.downloadDir,{recursive:true});const error=await shell.openPath(engine.settings.downloadDir);if(error)throw new Error(error);});
    ipcMain.handle('copy-token',()=>clipboard.writeText(token));
    ipcMain.handle('extension-folder',()=>shell.openPath(extensionDir));
    ipcMain.handle('clipboard',()=>clipboard.readText());
    win=new BrowserWindow({width:1380,height:960,minWidth:1050,minHeight:700,icon:path.join(__dirname,'assets','icon.png'),backgroundColor:'#f5f6f8',title:'Relay — Download Manager',autoHideMenuBar:true,webPreferences:{preload:path.join(__dirname,'preload.js'),contextIsolation:true,nodeIntegration:false,sandbox:true}});
    win.webContents.setWindowOpenHandler(()=>({action:'deny'}));
    win.webContents.on('will-navigate',event=>event.preventDefault());
    win.loadFile(path.join(__dirname,'ui','index.html'));
    engine.on('change',state=>{if(win && !win.isDestroyed())win.webContents.send('update',{...state,bridgeStatus,extensionDir});});
    engine.on('complete',j=>{if(Notification.isSupported())new Notification({title:'Download complete',body:j.name}).show();});
    app.on('activate',()=>win?.show());
  });
  app.on('window-all-closed',()=>app.quit());
  app.on('before-quit',event=>{
    if(quitting || !engine)return;
    event.preventDefault();quitting=true;
    if(bridge)bridge.close();
    engine.close().finally(()=>app.quit());
  });
}
