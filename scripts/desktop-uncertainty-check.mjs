// Uncertain Start (UI-08, CUT-04) and library save conflicts (SAVE-01) through the real desktop
// against a fresh practice-only service. The network faults are injected by the browser
// (DevTools Fetch), so the server and simulator are unmodified. The fixture is authored;
// captures stay in the caller's scratch directory.
import fs from 'node:fs/promises';
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
const [url,debugPort,fixture,artifacts]=process.argv.slice(2);
if(!url||!debugPort||!fixture||!artifacts)throw Error('Usage: desktop-uncertainty-check.mjs URL DEBUG_PORT SVG ARTIFACT_DIRECTORY');
const failures=[];
function check(ok,what){if(ok)console.log('  ok   '+what);else{console.log('  FAIL '+what);failures.push(what);}}

// One page in one browser context, driven only by real input events.
async function tab(context){
 const created=context?await browser('Target.createTarget',{url:'about:blank',browserContextId:context}):await (await fetch(`http://127.0.0.1:${debugPort}/json/new?about:blank`,{method:'PUT'})).json();
 const id=created.targetId||created.id;
 const ws=new WebSocket(`ws://127.0.0.1:${debugPort}/devtools/page/${id}`);await new Promise(r=>ws.addEventListener('open',r,{once:true}));
 let next=0;const waiting=new Map(),listeners=[];
 ws.addEventListener('message',e=>{const m=JSON.parse(e.data);if(m.id){const p=waiting.get(m.id);waiting.delete(m.id);m.error?p.reject(Error(JSON.stringify(m.error))):p.resolve(m.result);return;}for(const l of listeners)l(m);});
 const t={
  cdp:(method,params={})=>new Promise((resolve,reject)=>{const id=++next;waiting.set(id,{resolve,reject});ws.send(JSON.stringify({id,method,params}));}),
  on:fn=>listeners.push(fn),
  async page(expression){const r=await t.cdp('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(r.exceptionDetails)throw Error(JSON.stringify(r.exceptionDetails));return r.result.value;},
  state:async()=>await t.page('JSON.parse(CutCraft.state())'),
  async until(fn,why,timeout=15000){const end=Date.now()+timeout;while(Date.now()<end){if(await fn())return;await sleep(100);}throw Error(why+': '+JSON.stringify((await t.state()).message));},
  control:label=>t.page(`CutCraft.controls()[${JSON.stringify(label)}]`),
  async click(label){await t.until(async()=>!!await t.control(label),'Control rendered: '+label);const r=await t.control(label);const x=(r[0]+r[2])/2,y=(r[1]+r[3])/2;await t.cdp('Input.dispatchMouseEvent',{type:'mouseMoved',x,y});await t.cdp('Input.dispatchMouseEvent',{type:'mousePressed',x,y,button:'left',clickCount:1});await t.cdp('Input.dispatchMouseEvent',{type:'mouseReleased',x,y,button:'left',clickCount:1});await sleep(100);},
  async type(label,value){await t.click(label);await t.cdp('Input.dispatchKeyEvent',{type:'keyDown',key:'a',code:'KeyA',modifiers:2,windowsVirtualKeyCode:65});await t.cdp('Input.dispatchKeyEvent',{type:'keyUp',key:'a',code:'KeyA',modifiers:2,windowsVirtualKeyCode:65});await t.cdp('Input.insertText',{text:value});await t.cdp('Input.dispatchKeyEvent',{type:'keyDown',key:'Enter',code:'Enter',windowsVirtualKeyCode:13});await t.cdp('Input.dispatchKeyEvent',{type:'keyUp',key:'Enter',code:'Enter',windowsVirtualKeyCode:13});},
  async shot(name){const s=await t.cdp('Page.captureScreenshot',{format:'png'});await fs.writeFile(`${artifacts}/${name}.png`,Buffer.from(s.data,'base64'));},
  async desktop(){await t.cdp('Page.navigate',{url:url+'/desktop'});await t.ready();},
  async ready(){await t.until(async()=>await t.page('!!window.CutCraft && Object.keys(CutCraft.controls()).length>0'),'Desktop ready');},
  async reload(){await t.cdp('Page.reload');await sleep(300);await t.ready();},
  async open(file){const doc=await t.cdp('DOM.getDocument');const input=await t.cdp('DOM.querySelector',{nodeId:doc.root.nodeId,selector:'#file'});await t.cdp('DOM.setFileInputFiles',{nodeId:input.nodeId,files:[file]});await t.until(async()=>{const s=await t.state();return s.snapshot.design?.cut_ready&&!s.pending;},'Open authored design');await t.settle();},
  // Wait until the project revision has stopped moving (follow-up view commands after open).
  async settle(){let last=-1;for(let i=0;i<40;i++){const s=await t.state();if(!s.pending&&s.snapshot.revision===last)return;last=s.snapshot.revision;await sleep(250);}},
  // Same-origin JSON call from inside the page, with the desktop's protocol and CSRF headers.
  api:(path,method='GET',body)=>t.page(`(async()=>{const c=document.cookie.split(';').map(s=>s.trim()).find(s=>s.startsWith('cpi_csrf='))?.split('=').slice(1).join('=')||'';const r=await fetch(${JSON.stringify(path)},{method:${JSON.stringify(method)},credentials:'same-origin',headers:{'content-type':'application/json','x-cutcraft-protocol':'8','x-csrf-token':decodeURIComponent(c)},body:${body===undefined?'undefined':JSON.stringify(JSON.stringify(body))}});return {status:r.status,body:await r.json().catch(()=>null)};})()`),
 };
 await t.cdp('Runtime.enable');await t.cdp('Page.enable');await t.cdp('Network.enable');
 await t.cdp('Emulation.setDeviceMetricsOverride',{width:1280,height:900,deviceScaleFactor:1,mobile:false});
 return t;
}
let browserWs,browserNext=0;const browserWaiting=new Map();
async function browser(method,params={}){
 if(!browserWs){const v=await (await fetch(`http://127.0.0.1:${debugPort}/json/version`)).json();browserWs=new WebSocket(v.webSocketDebuggerUrl);await new Promise(r=>browserWs.addEventListener('open',r,{once:true}));browserWs.addEventListener('message',e=>{const m=JSON.parse(e.data);const p=browserWaiting.get(m.id);if(!p)return;browserWaiting.delete(m.id);m.error?p.reject(Error(JSON.stringify(m.error))):p.resolve(m.result);});}
 return new Promise((resolve,reject)=>{const id=++browserNext;browserWaiting.set(id,{resolve,reject});browserWs.send(JSON.stringify({id,method,params}));});
}

// ---------------------------------------------------------------------------------------------
console.log('Uncertain Start');
const a=await tab();
const sent=[];
a.on(m=>{if(m.method==='Network.requestWillBeSent'){const u=new URL(m.params.request.url);if(/\/api\/v1\/cutters\/[^/]+\/starts?(\/|$)/.test(u.pathname))sent.push({method:m.params.request.method,path:u.pathname,body:m.params.request.postData});}});
const posts=()=>sent.filter(r=>r.method==='POST'&&r.path.endsWith('/start')).length;
const lookups=()=>sent.filter(r=>r.method==='GET'&&r.path.includes('/starts/')).length;
// fault: 'response' drops the reply after the server handled it; 'request' drops it before.
let fault=null;
a.on(async m=>{if(m.method!=='Fetch.requestPaused')return;const p=m.params,u=new URL(p.request.url);
 const start=p.request.method==='POST'&&u.pathname.endsWith('/start'),lookup=u.pathname.includes('/starts/');
 const drop=fault&&((start&&(fault==='request'?p.responseStatusCode===undefined:fault==='response'?p.responseStatusCode!==undefined:p.responseStatusCode===undefined))||(lookup&&fault==='offline'&&p.responseStatusCode===undefined));
 if(drop)await a.cdp('Fetch.failRequest',{requestId:p.requestId,errorReason:'ConnectionReset'});
 else if(p.responseStatusCode!==undefined)await a.cdp('Fetch.continueResponse',{requestId:p.requestId});
 else await a.cdp('Fetch.continueRequest',{requestId:p.requestId});});
await a.cdp('Fetch.enable',{patterns:[{urlPattern:'*/api/v1/cutters/*',requestStage:'Request'},{urlPattern:'*/api/v1/cutters/*/start',requestStage:'Response'}]});
await a.desktop();await a.open(fixture);
await a.click('Test Cut');await a.until(async()=>(await a.state()).snapshot.ui.screen==='test-cut','Test screen');
await a.click('Cut pattern');await a.until(async()=>(await a.state()).cut_preview?.geometry,'Prepare test preview');
await a.click('Load virtual mat');await a.until(async()=>{const v=(await a.state()).state.cutters[0].view;return v.mat.state==='fresh'&&v.mat_loaded;},'Fresh virtual mat');
const view=async()=>(await a.state()).state.cutters[0].view;
// After a finished test the desktop asks for review first ("Try another pattern").
const readyToStart=async()=>{await a.until(async()=>{const v=await view();return v.activity==='idle'&&v.mat_loaded&&!(await a.state()).pending;},'Ready to start',45000);await sleep(300);if(await a.control('Try another pattern')){await a.click('Try another pattern');await sleep(200);}};

if(!process.env.CUTCRAFT_SAVE_ONLY){
console.log(' A. Start reached the cutter but its reply was lost');
fault='response';
let p0=posts(),l0=lookups();
await a.click('Start');
await a.until(async()=>(await view()).turn?.tests===1,'Test completed after lost reply',45000);
fault=null;
check(posts()-p0===1,'Start was sent once, never retried');
check(lookups()-l0===1,'the desktop looked up the Start by its key');
const msgA=(await a.state()).message;
check(!/not confirmed|failed|error/i.test(msgA),`no failure shown for a confirmed Start (message: ${JSON.stringify(msgA)})`);
await readyToStart();

console.log(' B. Start never reached the cutter');
const testsBefore=(await view()).turn.tests,executionBefore=(await view()).execution;
fault='request';p0=posts();l0=lookups();
await a.click('Start');
await a.until(async()=>!!(await a.state()).message && !(await a.state()).pending,'Start failure reported');
fault=null;
const msgB=(await a.state()).message;
await sleep(3000);
check((await view()).activity==='idle'&&(await view()).turn.tests===testsBefore&&(await view()).execution===executionBefore,'nothing started');
check(posts()-p0===1,'Start was not retried');
check(lookups()-l0===1,'the desktop looked up the Start by its key');
check(/not confirmed|check the cutter/i.test(msgB),`the message says Start is not confirmed (message: ${JSON.stringify(msgB)})`);
await a.shot('uncertain-start-not-sent');
const originalUnknown=sent.filter(r=>r.method==='POST').at(-1).body;
const beforeDisabled=posts();await a.click('Start');await sleep(300);
check(posts()===beforeDisabled,'Start is disabled while the original request is unresolved');

console.log(' C. Reconciliation is unavailable (connection down)');
fault='offline';p0=posts();l0=lookups();
await a.click('Retry original Start');
await a.until(async()=>!!(await a.state()).message && !(await a.state()).pending,'Offline Start reported');
const msgC=(await a.state()).message;
await a.shot('uncertain-start-offline');
fault=null;
await sleep(3000);
check(posts()-p0===0,'Failed lookup and reconnection do not resend Start');
check(/not confirmed|check the cutter/i.test(msgC),`the message says Start is not confirmed (message: ${JSON.stringify(msgC)})`);
const beforeReload=posts();
await a.reload();await sleep(3000);
check(posts()===beforeReload&&(await view()).activity==='idle'&&(await view()).turn.tests===testsBefore,'reload after an uncertain Start sends nothing and starts nothing');

console.log(' D. A deliberate retry afterwards uses the original request');
await readyToStart();
p0=posts();
await a.click('Retry original Start');
await a.until(async()=>(await view()).turn?.tests===testsBefore+1,'Deliberate Start completes',45000);
check(posts()-p0===1,'one deliberate retry, one cut');
check(sent.filter(r=>r.method==='POST').at(-1).body===originalUnknown,'retry after reload reuses the exact original body and key');
console.log(' E. A fresh Start and its lookup are both lost');
await readyToStart();
fault='offline';p0=posts();l0=lookups();
await a.click('Start');
await a.until(async()=>!!(await a.state()).message && !(await a.state()).pending,'Offline fresh Start reported');
check(posts()-p0===1&&lookups()-l0===1,'one Start attempt and one lookup while offline');
check(/not confirmed/i.test((await a.state()).message),'offline Start has clear recovery guidance');
check(!(await a.state()).connected,'failed reconciliation does not claim Connected');
const offlineBody=sent.filter(r=>r.method==='POST').at(-1).body;
fault=null;
await a.reload();await sleep(1500);
check(posts()-p0===1,'reload never replays the offline Start');
await readyToStart();
await a.click('Retry original Start');
await a.until(async()=>(await view()).turn?.tests===testsBefore+2,'Offline Start recovered deliberately',45000);
check(sent.filter(r=>r.method==='POST').at(-1).body===offlineBody,'offline recovery preserves the original request');

}
await a.cdp('Fetch.disable');

// ---------------------------------------------------------------------------------------------
console.log('Save conflict');
const password='authored-check-password';
const first=await a.api('/api/v1/users/first','POST',{name:'alex',password});
if(first.status!==200)throw Error('Create first user: '+JSON.stringify(first));
const second=await a.api('/api/v1/users','POST',{name:'sam',password,role:'user'});
if(second.status!==200)throw Error('Create second user: '+JSON.stringify(second));
async function person(name){const ctx=(await browser('Target.createBrowserContext',{disposeOnDetach:true})).browserContextId;const t=await tab(ctx);await t.cdp('Page.navigate',{url:url+'/'});await sleep(500);const r=await t.api('/api/v1/login','POST',{name,password});if(r.status!==200)throw Error(`Log in ${name}: `+JSON.stringify(r));if(name!=='alex'){const c=await t.api('/api/v1/me/password','POST',{old:password,new:password+'-own'});if(c.status!==200)throw Error(`New password for ${name}: `+JSON.stringify(c));const again=await t.api('/api/v1/login','POST',{name,password:password+'-own'});if(again.status!==200)throw Error(`Log in ${name} again: `+JSON.stringify(again));}await t.desktop();return t;}
const alex=await person('alex'),sam=await person('sam');

await alex.open(fixture);
await alex.click('Save');await alex.until(async()=>!!await alex.control('Save project'),'First Save opens Save As');
await alex.click('Save project');
await alex.until(async()=>{const d=(await alex.state()).snapshot.design;return d.saved&&!d.unsaved;},'Alex saves to the library');
const job=(await alex.state()).snapshot.design.saved.job;
const revision1=(await alex.state()).snapshot.design.saved.revision;

const opened=await sam.api('/api/v1/design/open','POST',{project:{job},key:'sam-open-'+Date.now(),incarnation:(await sam.state()).snapshot.incarnation,revision:(await sam.state()).snapshot.revision});
if(opened.status!==200)throw Error('Sam opens the library project: '+JSON.stringify(opened));
await sam.until(async()=>(await sam.state()).snapshot.design?.saved?.job===job,'Sam has the library project open');

await alex.type('Force','12');await alex.until(async()=>(await alex.state()).snapshot.ui.sheet.material.force===12,'Alex edits');
await alex.click('Save');
await alex.until(async()=>{const d=(await alex.state()).snapshot.design;return d.saved.revision>revision1&&!d.unsaved;},'Alex saves a new revision');
const revision2=(await alex.state()).snapshot.design.saved.revision;

await sam.type('Force','15');await sam.until(async()=>(await sam.state()).snapshot.ui.sheet.material.force===15,'Sam edits');
await sam.click('Save');
await sam.until(async()=>!(await sam.state()).pending&&!!(await sam.state()).message,'Sam\'s Save answered');
await sleep(300);
await sam.shot('save-conflict');
const samState=await sam.state();
check(samState.result?.error_code==='saved-elsewhere','Save is refused as saved elsewhere');
check(!!await sam.control('Reload saved project')&&!!await sam.control('Save As a new project'),'the conflict offers Reload and Save As');
check(samState.snapshot.design.unsaved&&samState.snapshot.ui.sheet.material.force===15,'Sam\'s unsaved work is kept');
const library=await alex.api(`/api/v1/jobs/${encodeURIComponent(job)}`);
const current=library.body?.details?.current_revision;
check(current===revision2,`the library keeps Alex's revision ${revision2} (now ${JSON.stringify(current)})`);

await sam.click('Save As a new project');
await sam.until(async()=>!!await sam.control('Save project'),'Save As dialog');
await sleep(500);
// The Save button must lie inside the visible dialog: a click outside a modal dismisses it.
const visible=await sam.page(`(()=>{const c=CutCraft.controls(),s=c['Save project'],n=c['Name'];return {save:s,name:n};})()`);
await sam.shot('save-as-from-conflict');
await sam.click('Save project');
await sleep(1500);
const viaConflict=(await sam.state()).snapshot.design;
check(viaConflict.saved?.job!==job&&!viaConflict.unsaved,`Save As from the conflict dialog saves a new project (Save button at ${JSON.stringify(visible.save)}; see save-as-from-conflict.png)`);
if(viaConflict.saved?.job===job){
 // File → Save As… in the same page is clipped the same way; check that, then continue
 // after a reload (fresh dialog sizing) so the remaining checks still run.
 if(await sam.control('Close dialog'))await sam.click('Close dialog');
 await sam.click('Menu File');await sam.click('Save As…');
 await sam.until(async()=>!!await sam.control('Save project'),'File Save As dialog');await sleep(500);
 await sam.shot('file-save-as-after-conflict');
 await sam.click('Save project');await sleep(1500);
 const viaFile=(await sam.state()).snapshot.design;
 check(viaFile.saved?.job!==job&&!viaFile.unsaved,'File → Save As… in the same page saves a new project (see file-save-as-after-conflict.png)');
 if(await sam.control('Close dialog'))await sam.click('Close dialog');
 await sam.reload();
 await sam.click('Menu File');await sam.click('Save As…');
 await sam.until(async()=>!!await sam.control('Save project'),'Save As after reload');await sleep(300);
 await sam.click('Save project');
}
await sam.until(async()=>{const d=(await sam.state()).snapshot.design;return d.saved&&d.saved.job!==job&&!d.unsaved;},'Sam saves a new project');
check((await sam.state()).snapshot.design.saved.job!==job,'Save As creates a separate project');

console.log(' Reload after a conflict');
await alex.type('Force','13');await alex.until(async()=>(await alex.state()).snapshot.ui.sheet.material.force===13,'Alex edits again');
await alex.click('Save');await alex.until(async()=>{const d=(await alex.state()).snapshot.design;return d.saved.revision>revision2&&!d.unsaved;},'Alex saves revision 3');
const reopen=await sam.api('/api/v1/design/open','POST',{project:{job,revision:revision2},key:'sam-reopen-'+Date.now(),incarnation:(await sam.state()).snapshot.incarnation,revision:(await sam.state()).snapshot.revision,replace_unsaved:true});
if(reopen.status!==200)throw Error('Sam reopens revision 2: '+JSON.stringify(reopen));
await sam.until(async()=>(await sam.state()).snapshot.design?.saved?.job===job,'Sam on the shared project');
await sam.type('Force','16');await sam.until(async()=>(await sam.state()).snapshot.ui.sheet.material.force===16,'Sam edits the stale revision');
await sam.click('Save');
await sam.until(async()=>!!await sam.control('Reload saved project'),'Second conflict');
await sam.click('Reload saved project');
await sam.until(async()=>{const s=await sam.state();return !s.pending&&s.snapshot.design?.saved?.job===job&&!s.snapshot.design.unsaved;},'Reload replaces the stale work',20000);
const reloaded=await sam.state();
check(reloaded.snapshot.ui.sheet.material.force===13,`Reload shows the newest saved revision (force ${reloaded.snapshot.ui.sheet.material.force})`);
check(!(await sam.control('Reload saved project')),'the conflict dialog closes after Reload');
await sam.shot('save-conflict-reloaded');

console.log(failures.length?`\n${failures.length} check(s) failed:\n- `+failures.join('\n- '):process.env.CUTCRAFT_SAVE_ONLY?'\nAll save-conflict checks passed (Start fault cases skipped).':'\nAll uncertainty and conflict checks passed.');
process.exitCode=failures.length?1:0;
browserWs?.close();
process.exit();
