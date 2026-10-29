// Real mouse/keyboard acceptance against a fresh practice-only service. No dispatcher shortcuts.
// The fixture is authored; captures stay in the caller's scratch directory.
import fs from 'node:fs/promises';
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
const [url,debugPort,fixture,artifacts]=process.argv.slice(2);
if(!url||!debugPort||!fixture||!artifacts)throw Error('Usage: desktop-browser-check.mjs URL DEBUG_PORT SVG ARTIFACT_DIRECTORY');
const tab=await (await fetch(`http://127.0.0.1:${debugPort}/json/new?about:blank`,{method:'PUT'})).json();
const ws=new WebSocket(tab.webSocketDebuggerUrl);await new Promise(r=>ws.addEventListener('open',r,{once:true}));
let next=0;const waiting=new Map();
ws.addEventListener('message',e=>{const m=JSON.parse(e.data);if(!m.id)return;const p=waiting.get(m.id);waiting.delete(m.id);m.error?p.reject(Error(JSON.stringify(m.error))):p.resolve(m.result);});
function cdp(method,params={}){return new Promise((resolve,reject)=>{let id=++next;waiting.set(id,{resolve,reject});ws.send(JSON.stringify({id,method,params}));});}
async function page(expression){const r=await cdp('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(r.exceptionDetails)throw Error(JSON.stringify(r.exceptionDetails));return r.result.value;}
async function state(){return await page('JSON.parse(CutCraft.state())');}
async function until(fn,why,timeout=15000){const end=Date.now()+timeout;while(Date.now()<end){if(await fn())return;await sleep(100);}throw Error(why+': '+JSON.stringify((await state()).message));}
async function click(label){let r=await page(`CutCraft.controls()[${JSON.stringify(label)}]`);if(!r)throw Error('No control '+label);const x=(r[0]+r[2])/2,y=(r[1]+r[3])/2;if(y<80&&label==='Width')throw Error('Bad field');await cdp('Input.dispatchMouseEvent',{type:'mousePressed',x,y,button:'left',clickCount:1});await cdp('Input.dispatchMouseEvent',{type:'mouseReleased',x,y,button:'left',clickCount:1});await sleep(100);}
async function type(label,value){await click(label);await cdp('Input.dispatchKeyEvent',{type:'keyDown',key:'a',code:'KeyA',modifiers:2,windowsVirtualKeyCode:65});await cdp('Input.dispatchKeyEvent',{type:'keyUp',key:'a',code:'KeyA',modifiers:2,windowsVirtualKeyCode:65});await cdp('Input.insertText',{text:value});await cdp('Input.dispatchKeyEvent',{type:'keyDown',key:'Enter',code:'Enter',windowsVirtualKeyCode:13});await cdp('Input.dispatchKeyEvent',{type:'keyUp',key:'Enter',code:'Enter',windowsVirtualKeyCode:13});}
await cdp('Runtime.enable');
await cdp('Page.enable');
await cdp('Emulation.setDeviceMetricsOverride',{width:1280,height:900,deviceScaleFactor:1,mobile:false});
await cdp('Page.navigate',{url:url+'/desktop'});
await until(async()=>await page('!!window.CutCraft && Object.keys(CutCraft.controls()).length>0'),'Desktop ready');
const doc=await cdp('DOM.getDocument');
const input=await cdp('DOM.querySelector',{nodeId:doc.root.nodeId,selector:'#file'});
await cdp('DOM.setFileInputFiles',{nodeId:input.nodeId,files:[fixture]});
await until(async()=>(await state()).snapshot.design?.cut_ready && !(await state()).pending,'Open authored design');
await sleep(300);
await type('Width','1 in');
await until(async()=>Math.abs((await state()).snapshot.ui.size_mm[0]-25.4)<.001,'Resize width');
await type('Force','12');
await until(async()=>(await state()).snapshot.ui.sheet.material.force===12,'Set force');
await click('Save');
await until(async()=>!!(await page('CutCraft.controls()["Save project"]')),'First Save opens Save As');
await click('Save project');
await until(async()=>!!(await state()).snapshot.design.saved && !(await state()).snapshot.design.unsaved,'Save project');
await click('Cut Preview');
await until(async()=>(await state()).snapshot.ui.screen==='mat','Cut Preview navigation');
if((await state()).snapshot.design.unsaved)throw Error('Changing views dirtied the saved project');
await sleep(300);
const r=await page('CutCraft.controls()["Resize se"]');
if(!r)throw Error('Missing resize handle');
const x=(r[0]+r[2])/2,y=(r[1]+r[3])/2;
await cdp('Input.dispatchMouseEvent',{type:'mouseMoved',x,y});
await cdp('Input.dispatchMouseEvent',{type:'mousePressed',x,y,button:'left',clickCount:1});
await cdp('Input.dispatchMouseEvent',{type:'mouseMoved',x:x+25,y:y+25,button:'left',buttons:1});
await cdp('Input.dispatchMouseEvent',{type:'mouseReleased',x:x+25,y:y+25,button:'left',clickCount:1});
await until(async()=>(await state()).snapshot.ui.size_mm[0]>25.5,'Drag resize');
await click('Undo');await until(async()=>Math.abs((await state()).snapshot.ui.size_mm[0]-25.4)<.001,'Undo drag');
console.log('PASS: open, typed units, material edit, Save As, clean view navigation, drag, Undo');
await click('Test Cut');await until(async()=>(await state()).snapshot.ui.screen==='test-cut','Test screen');
if((await state()).state.cutters[0].view.turn)throw Error('Toolbar Test Cut prepared a job before explicit review');
await click('Cut pattern');await until(async()=>(await state()).cut_preview?.geometry,'Prepare test preview');
await click('Load virtual mat');
await until(async()=>(await state()).state.cutters[0].view.mat.state==='fresh'&&(await state()).state.cutters[0].view.mat_loaded,'Fresh virtual mat');
await click('Start');await until(async()=>(await state()).state.cutters[0].view.turn?.tests===1,'Test completed');
await until(async()=>(await state()).state.cutters[0].view.mat_loaded,'Mat ready after first test');
await click('Try another pattern');await click('Start');
await until(async()=>(await state()).state.cutters[0].view.turn?.tests===2,'Second test completed');
if((await state()).state.cutters[0].view.turn.taken.length!==2)throw Error('Test reservations lost');
await click('Cut');await until(async()=>(await state()).snapshot.ui.screen==='cut','Design cut screen');
await until(async()=>(await state()).state.cutters[0].view.mat_loaded,'Same loaded mat ready for design');
await click('Start');await until(async()=>(await state()).state.cutters[0].view.mat.state==='used','Main cut completed',45000);
console.log('PASS: toolbar navigation only, explicit prepare/Start, two tests and design on one loaded mat');
await cdp('Page.reload');
await until(async()=>await page('!!window.CutCraft && Object.keys(CutCraft.controls()).length>0'),'Reload ready');
await until(async()=>(await state()).snapshot.ui.sheet?.material?.force===12,'Material setting survives reload');
if((await state()).state.cutters[0].view.activity!=='idle')throw Error('Reload started a cut');
for(const [width,height] of [[1280,800],[1920,1080]]){
 await cdp('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:false});await sleep(300);
 const shot=await cdp('Page.captureScreenshot',{format:'png'});
 await fs.writeFile(`${artifacts}/desktop-${width}x${height}.png`,Buffer.from(shot.data,'base64'));
}
console.log('PASS: reload preserves settings without restarting motion; desktop screenshots captured');
ws.close();
