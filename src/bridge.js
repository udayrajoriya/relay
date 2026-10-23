const http = require('node:http');
const { timingSafeEqual } = require('node:crypto');
const {validURL}=require('./engine');

function startBridge(engine, token, port=17864, {selectDestination}={}) {
  const server=http.createServer(async(req,res)=>{
    res.setHeader('Content-Type','application/json');
    res.setHeader('Cache-Control','no-store');
    const send=(status,data)=>{res.writeHead(status);res.end(JSON.stringify(data));};
    const auth=Buffer.from(req.headers.authorization || '');const expected=Buffer.from(`Bearer ${token}`);
    if(auth.length!==expected.length || !timingSafeEqual(auth,expected)) return send(401,{error:'Pair the extension using the token in Relay settings.'});
    if(req.method==='GET' && req.url==='/health') return send(200,{app:'Relay',version:'0.1.0'});
    if(req.method!=='POST' || req.url!=='/downloads') return send(404,{error:'Not found.'});
    let body='';
    try {
      for await(const chunk of req) {body+=chunk;if(body.length>16384){send(413,{error:'Request too large.'});req.destroy();return;}}
      const input=JSON.parse(body);
      const url=validURL(input.url);
      const destination=selectDestination?await selectDestination(()=>!res.destroyed):undefined;
      // A browser timeout or a canceled folder dialog must not leave a stray download.
      if(res.destroyed)return;
      if(destination===null)return send(409,{error:'Download canceled in Relay. The browser download was kept.'});
      const id=engine.add({url,name:input.name,queue:'Browser',destination});
      send(201,{id});
    } catch(e) {send(400,{error:e.message});}
  });
  server.requestTimeout=10000;
  return new Promise((resolve,reject)=>{server.once('error',reject);server.listen(port,'127.0.0.1',()=>resolve(server));});
}
module.exports={startBridge};
