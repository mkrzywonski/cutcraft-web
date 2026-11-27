// Real mouse/keyboard acceptance against a fresh practice-only service. No dispatcher shortcuts.
// The fixture is authored; captures stay in the caller's scratch directory.
import fs from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
const [url,debugPort,fixture,artifacts]=process.argv.slice(2);
if(!url||!debugPort||!fixture||!artifacts)throw Error('Usage: desktop-browser-check.mjs URL DEBUG_PORT SVG ARTIFACT_DIRECTORY');
const tab=await (await fetch(`http://127.0.0.1:${debugPort}/json/new?about:blank`,{method:'PUT'})).json();
const ws=new WebSocket(tab.webSocketDebuggerUrl);await new Promise(r=>ws.addEventListener('open',r,{once:true}));
let next=0;const waiting=new Map();
ws.addEventListener('message',e=>{const m=JSON.parse(e.data);if(!m.id)return;const p=waiting.get(m.id);waiting.delete(m.id);m.error?p.reject(Error(JSON.stringify(m.error))):p.resolve(m.result);});
const assetResponses=[];
const fontResponses=[];
ws.addEventListener('message',e=>{const m=JSON.parse(e.data);if(m.method==='Network.responseReceived' && m.params.response.url.includes('/api/v1/fonts'))fontResponses.push({url:m.params.response.url,status:m.params.response.status});});
ws.addEventListener('message',e=>{const m=JSON.parse(e.data);if(m.method==='Network.responseReceived' && m.params.response.url.includes('/desktop/assets/'))assetResponses.push(m.params.response);});
function cdp(method,params={}){return new Promise((resolve,reject)=>{let id=++next;waiting.set(id,{resolve,reject});ws.send(JSON.stringify({id,method,params}));});}
async function page(expression){const r=await cdp('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(r.exceptionDetails)throw Error(JSON.stringify(r.exceptionDetails));return r.result.value;}
async function state(){return await page('JSON.parse(CutCraft.state())');}
async function until(fn,why,timeout=15000){const end=Date.now()+timeout;while(Date.now()<end){if(await fn())return;await sleep(100);}const failureShot=await cdp('Page.captureScreenshot',{format:'png'});await fs.writeFile(`${artifacts}/failure.png`,Buffer.from(failureShot.data,'base64'));await fs.writeFile(`${artifacts}/failure-state.json`,JSON.stringify({state:await state(),fontResponses,controls:await page('CutCraft.controls()')}));throw Error(why+': '+JSON.stringify((await state()).message));}
async function click(label){if(label!=='Stop now')await until(async()=>!(await state()).pending,'Ready for '+label);await until(async()=>!!await page(`CutCraft.controls()[${JSON.stringify(label)}]`),'Control rendered: '+label);let r=await page(`CutCraft.controls()[${JSON.stringify(label)}]`);if(!r)throw Error('No control '+label);const x=(r[0]+r[2])/2,y=(r[1]+r[3])/2;if(y<80&&label==='Width')throw Error('Bad field');await cdp('Input.dispatchMouseEvent',{type:'mousePressed',x,y,button:'left',clickCount:1});await cdp('Input.dispatchMouseEvent',{type:'mouseReleased',x,y,button:'left',clickCount:1});await sleep(100);}
async function type(label,value){await click(label);await cdp('Input.dispatchKeyEvent',{type:'keyDown',key:'a',code:'KeyA',modifiers:2,windowsVirtualKeyCode:65});await cdp('Input.dispatchKeyEvent',{type:'keyUp',key:'a',code:'KeyA',modifiers:2,windowsVirtualKeyCode:65});await cdp('Input.insertText',{text:value});await cdp('Input.dispatchKeyEvent',{type:'keyDown',key:'Enter',code:'Enter',windowsVirtualKeyCode:13});await cdp('Input.dispatchKeyEvent',{type:'keyUp',key:'Enter',code:'Enter',windowsVirtualKeyCode:13});}
await cdp('Runtime.enable');
await cdp('Page.enable');
await cdp('Network.enable');
await cdp('Emulation.setDeviceMetricsOverride',{width:1280,height:900,deviceScaleFactor:1,mobile:false});
await cdp('Page.navigate',{url:url+'/desktop'});
await until(async()=>await page('!!window.CutCraft && Object.keys(CutCraft.controls()).length>0'),'Desktop ready');
const wasmResponse=assetResponses.find(r=>/cutcraft_ui_bg\.[0-9a-f]{64}\.wasm$/.test(r.url));
if(!wasmResponse)throw Error('Desktop did not load the content-addressed wasm');
for(const response of assetResponses){
 const headers=Object.fromEntries(Object.entries(response.headers).map(([k,v])=>[k.toLowerCase(),v]));
 if(headers['content-encoding']!=='gzip' || !headers['cache-control']?.includes('immutable'))throw Error('Desktop asset is not compressed and immutable: '+response.url);
}
if(assetResponses.length!==4)throw Error('Expected CSS, runtime, bindings and wasm through hashed routes');
await fs.writeFile(`${artifacts}/asset-transfer.json`,JSON.stringify(assetResponses,null,2));
console.log('PASS: real browser loads all four content-addressed, gzip-compressed immutable desktop assets');
await cdp('Network.emulateNetworkConditions',{offline:true,latency:0,downloadThroughput:-1,uploadThroughput:-1});
await click('Menu Help');await click('Manual');
await until(async()=>!!await page('CutCraft.controls()["Close dialog"]'),'Bundled manual opens offline');
const manual=await cdp('Page.captureScreenshot',{format:'png'});await fs.writeFile(`${artifacts}/manual-offline.png`,Buffer.from(manual.data,'base64'));
await click('Close dialog');
await cdp('Network.emulateNetworkConditions',{offline:false,latency:0,downloadThroughput:-1,uploadThroughput:-1});
await until(async()=>(await state()).connected,'Reconnect after offline help');
await click('Menu Help');await click('About CutCraft');await click('User manual');
await until(async()=>!!await page('CutCraft.controls()["Close dialog"]'),'About opens bundled manual');await click('Close dialog');
console.log('PASS: bundled Manual opens offline and from About without an external browser page');
await click('Menu File');await sleep(150);
const menu=await cdp('Page.captureScreenshot',{format:'png'});await fs.writeFile(`${artifacts}/file-menu-shortcuts.png`,Buffer.from(menu.data,'base64'));
await cdp('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});await cdp('Input.dispatchKeyEvent',{type:'keyUp',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
await click('Menu Edit');await click('Materials…');await click('New…');
await type('Preset name','Browser default fixture');await click('Default for new designs');await click('Save changes');
await until(async()=>(await state()).snapshot.ui.defaults.material==='Browser default fixture' && !(await state()).pending,'Save default material from editor');
await click('Menu Edit');await click('Materials…');
await until(async()=>!!await page('CutCraft.controls()["Default preset Browser default fixture"]'),'Default tag');
const rows=await page('Object.entries(CutCraft.controls()).filter(([key])=>key.startsWith("Preset ")).sort((a,b)=>a[1][1]-b[1][1])');
if(rows[0][0]!=='Preset Browser default fixture')throw Error('Default preset is not listed first');
await click('Preset Browser default fixture');
if(await page('!!CutCraft.controls()["Save changes"]'))throw Error('Single selection opened the editor');
await click('Edit…');
const agentHit=await page('(()=>{const input=document.querySelector("#cutcraft + input[type=text]"),r=input.getBoundingClientRect();return document.elementFromPoint(r.x+.5,r.y+.5)?.id;})()');
if(agentHit!=='cutcraft')throw Error('Invisible keyboard input intercepts canvas clicks');
await type('Preset name','Browser renamed default');
const renamed=await cdp('Page.captureScreenshot',{format:'png'});await fs.writeFile(`${artifacts}/rename-draft.png`,Buffer.from(renamed.data,'base64'));
await click('Save changes');
await until(async()=>(await state()).snapshot.ui.defaults.material==='Browser renamed default' && !(await state()).pending,'Rename follows default');
await click('Menu Edit');await click('Materials…');await click('Preset Browser renamed default');await click('Delete');await click('Delete preset');
await until(async()=>!(await state()).snapshot.ui.defaults.material && !(await state()).pending,'Deleting default clears its identity');
if(await page('!!CutCraft.controls()["Close dialog"]'))await click('Close dialog');
console.log('PASS: preset default checkbox, first-row tag, select without editing, rename and delete fallback');
// Original 1 KiB test font: validates the actual child worker, shared archive and live rescan.
await cdp('Browser.setDownloadBehavior',{behavior:'allow',downloadPath:artifacts});
const fontFixture=fileURLToPath(new URL('../crates/import/tests/fixtures/cutcraft-test-font.ttf',import.meta.url));
const fontDesign=`${artifacts}/authored-font.svg`;
await fs.writeFile(fontDesign,'<svg xmlns="http://www.w3.org/2000/svg" width="40mm" height="30mm" viewBox="0 0 40 30"><text x="5" y="20" font-family="CutCraft Fixture" font-size="15">A</text></svg>');
const fontDoc=await cdp('DOM.getDocument');
const designPicker=await cdp('DOM.querySelector',{nodeId:fontDoc.root.nodeId,selector:'#file'});
await cdp('DOM.setFileInputFiles',{nodeId:designPicker.nodeId,files:[fontDesign]});
await until(async()=>!(await state()).pending && (await state()).snapshot.ui.report?.pending?.some(i=>i.choices?.some(c=>c.decision==='font_substitute')),'Authored missing font issue');
await click('Upload a font…');
await until(async()=>(await state()).fonts?.fonts?.length && !(await state()).pending,'Font catalog',60000);
async function chooseFont(label,file){
 await click(label);
 const document=await cdp('DOM.getDocument');
 const input=await cdp('DOM.querySelector',{nodeId:document.root.nodeId,selector:'#font-file'});
 if(!input.nodeId)throw Error('Font upload did not open its picker');
 await cdp('DOM.setFileInputFiles',{nodeId:input.nodeId,files:[file]});
}
await chooseFont('Upload font…',fontFixture);
await until(async()=>{const s=await state();return !s.pending && s.fonts?.installed?.some(f=>f.faces.some(face=>face.family==='CutCraft Fixture'));},'Installed font available to the live worker',60000);
const installedFont=(await state()).fonts.installed[0];
await click('Close dialog');await click('Run again');
await until(async()=>!(await state()).pending && (await state()).snapshot.design?.cut_ready,'Uploaded font resolves missing font after Run again');
await click('Menu File');await click('Download project…');
await until(async()=>{try{return (await fs.stat(`${artifacts}/design.cutcraft`)).size>0;}catch{return false;}},'Download project with prepared letter shapes');
await click('Menu Edit');await click('Fonts…');
await until(async()=>!(await state()).pending && (await state()).fonts?.project_fonts?.some(f=>f.family==='CutCraft Fixture' && f.available),'Project font is available');
await sleep(300);

await click('Download project font CutCraft Fixture Regular');
await until(async()=>{try{return (await fs.readFile(`${artifacts}/CutCraft Fixture-Regular.ttf`)).equals(await fs.readFile(fontFixture));}catch{return false;}},'Project font download preserves bytes and uses family/style filename');
await click('Download library font cutcraft-test-font.ttf');
await until(async()=>{try{return (await fs.readFile(`${artifacts}/cutcraft-test-font.ttf`)).equals(await fs.readFile(fontFixture));}catch{return false;}},'Font download is byte-exact');
const fontShot=await cdp('Page.captureScreenshot',{format:'png'});await fs.writeFile(`${artifacts}/fonts-library.png`,Buffer.from(fontShot.data,'base64'));
const replacementFont=`${artifacts}/replacement.ttf`;
await fs.writeFile(replacementFont,Buffer.concat([await fs.readFile(fontFixture),Buffer.from([0])]));
await chooseFont('Replace cutcraft-test-font.ttf…',replacementFont);
await until(async()=>{const s=await state();return !s.pending && s.fonts?.installed?.length===1 && s.fonts.installed[0].id!==installedFont.id;},'Explicit font replacement',60000);
await click('Remove replacement.ttf');await click('Remove font');
await until(async()=>{const s=await state();return !s.pending && s.fonts?.installed?.length===0 && s.fonts.loading===false && s.fonts.project_fonts.some(f=>f.family==='CutCraft Fixture' && !f.available);},'Removal rescans the live worker',60000);
await click('Menu File');await click('Close design');await click('Close design');
await until(async()=>!(await state()).snapshot.design && !(await state()).pending,'Close authored font design');
const savedFontDocument=await cdp('DOM.getDocument');
const savedFontPicker=await cdp('DOM.querySelector',{nodeId:savedFontDocument.root.nodeId,selector:'#file'});
await cdp('DOM.setFileInputFiles',{nodeId:savedFontPicker.nodeId,files:[`${artifacts}/design.cutcraft`]});
await until(async()=>!(await state()).pending && (await state()).snapshot.design?.cut_ready,'Saved project cuts without the removed font');
await click('Menu File');await click('Close design');await click('Close design');
await until(async()=>!(await state()).snapshot.design && !(await state()).pending,'Close saved font project');
console.log('PASS: missing-font upload and reprepare, project font listing, exact download, replacement, removal and saved letter shapes');
const doc=await cdp('DOM.getDocument');
const input=await cdp('DOM.querySelector',{nodeId:doc.root.nodeId,selector:'#file'});
const background=fileURLToPath(new URL('../examples/issues/08-background-rectangle.svg',import.meta.url));
await cdp('DOM.setFileInputFiles',{nodeId:input.nodeId,files:[background]});
await until(async()=>{const s=await state();return s.snapshot.ui.report?.pending?.length && Object.keys(s.geometry.source_bounds_mm||{}).length && !s.pending;},'Background issue and source bounds');
const issueState=await state();
await cdp('Browser.setDownloadBehavior',{behavior:'allow',downloadPath:artifacts});
await click('Menu File');await click('Download original design…');
await until(async()=>{try{return (await fs.readFile(`${artifacts}/original.svg`)).equals(await fs.readFile(background));}catch{return false;}},'Original download matches uploaded bytes before resolving issues');
if((await state()).snapshot.revision!==issueState.snapshot.revision)throw Error('Original download changed the project');
console.log('PASS: original design downloads unchanged while preparation still has an issue');
await click('Menu Help');await click('Problem report…');await click('Download problem report');
await until(async()=>{try{return JSON.parse(await fs.readFile(`${artifacts}/cutcraft-cut-report.json`,'utf8')).engine.name==='VectorCraft';}catch{return false;}},'Problem report downloads before the first cut');
const diagnosticReport=JSON.parse(await fs.readFile(`${artifacts}/cutcraft-cut-report.json`,'utf8'));
if(!diagnosticReport.platform.os || diagnosticReport.device.simulated!==true || diagnosticReport.executions.length!==0)throw Error('Problem report missing platform/cutter context');
await fs.rename(`${artifacts}/cutcraft-cut-report.json`,`${artifacts}/problem-report-before-cut.json`);
await click('Close dialog');
console.log('PASS: Help downloads a bounded diagnostic report before any cut has run');


await click('Overlay');await until(async()=>(await state()).snapshot.ui.screen==='overlay' && (await state()).picture,'Overlay renders artwork');
await sleep(250);
const contrast=await cdp('Page.captureScreenshot',{format:'png'});await fs.writeFile(`${artifacts}/overlay-contrast.png`,Buffer.from(contrast.data,'base64'));
await click('Side by side');await until(async()=>(await state()).snapshot.ui.screen==='side-by-side' && !(await state()).pending,'Return from Overlay');
const showRevision=(await state()).snapshot.revision;
const backgroundId=issueState.snapshot.ui.report.pending[0].source_ids[0];
if(issueState.geometry.lines.some(line=>line.source_ids.includes(backgroundId)))throw Error('Unanswered background unexpectedly has cut lines');
await click('Show me');
await until(async()=>!!await page('CutCraft.controls()["Shown source 0"] && CutCraft.controls()["Shown source 1"]'),'Source-only object highlighted in both panes');
await until(async()=>!!(await state()).picture,'Source artwork rendered for Show me');await sleep(250);
const shown=await cdp('Page.captureScreenshot',{format:'png'});await fs.writeFile(`${artifacts}/show-me-withheld-background.png`,Buffer.from(shown.data,'base64'));
if((await state()).snapshot.revision!==showRevision)throw Error('Show me changed the project');
await click('Menu File');await click('Close design');await until(async()=>!!await page('CutCraft.controls()["Save first"]'),'Close confirmation');await click('Close design');
await until(async()=>!(await state()).snapshot.design && !(await state()).pending,'Close issue fixture');
console.log('PASS: Show me locates a withheld source object without creating cut geometry or an edit');
// Advanced preparation edits the same recipe as MCP; conversion off must block cutting.
const thickLine=fileURLToPath(new URL('../examples/issues/21-thick-line.svg',import.meta.url));
await cdp('DOM.setFileInputFiles',{nodeId:input.nodeId,files:[thickLine]});
await until(async()=>(await state()).snapshot.design?.cut_ready && !(await state()).pending,'Open thick-line fixture');
await click('Advanced preparation');
await click('Turn thick lines into shapes');
await until(async()=>{const s=await state();return !s.pending && s.snapshot.ui.report?.diagnostics?.some(d=>d.code==='stroke.conversion_disabled') && !s.snapshot.design.cut_ready;},'Disabled stroke conversion blocks cutting');
const advanced=await cdp('Page.captureScreenshot',{format:'png'});await fs.writeFile(`${artifacts}/advanced-preparation.png`,Buffer.from(advanced.data,'base64'));
await click('Turn thick lines into shapes');
await until(async()=>{const s=await state();return !s.pending && s.snapshot.design.cut_ready && s.snapshot.ui.report.recipe.decisions.filter(d=>d.decision==='preparation_step').length===1;},'Re-enabling conversion replaces the old choice');
await click('Clear answers');
await until(async()=>{const s=await state();return !s.pending && s.snapshot.ui.report.recipe.decisions.length===0;},'Clear answers restores defaults');
await click('Advanced preparation');
await click('Menu File');await click('Close design');
await until(async()=>!!await page('CutCraft.controls()["Save first"]'),'Close preparation confirmation');await click('Close design');
await until(async()=>!(await state()).snapshot.design && !(await state()).pending,'Close preparation fixture');
console.log('PASS: real Advanced preparation checkbox blocks/restores cut readiness and Clear answers resets the recipe');
await cdp('DOM.setFileInputFiles',{nodeId:input.nodeId,files:[fixture]});
await until(async()=>(await state()).snapshot.design?.cut_ready && !(await state()).pending,'Open authored design');
const inspectLabel=(await state()).snapshot.ui.rows[0].label;
await click('Cut '+inspectLabel);
await until(async()=>!(await state()).pending && !(await state()).snapshot.ui.rows[0].included,'Exclude authored row');
const inspectionRevision=(await state()).snapshot.revision;
await until(async()=>!!await page(`CutCraft.controls()[${JSON.stringify('Inspect '+inspectLabel)}]`),'Row inspection target');
const inspectRect=await page(`CutCraft.controls()[${JSON.stringify('Inspect '+inspectLabel)}]`);
await cdp('Input.dispatchTouchEvent',{type:'touchStart',touchPoints:[{x:(inspectRect[0]+inspectRect[2])/2,y:(inspectRect[1]+inspectRect[3])/2}]});
await cdp('Input.dispatchTouchEvent',{type:'touchEnd',touchPoints:[]});
await sleep(300);
if((await state()).snapshot.revision!==inspectionRevision || (await state()).snapshot.ui.rows[0].included)throw Error('Touch inspection changed inclusion');
const inspected=await cdp('Page.captureScreenshot',{format:'png'});await fs.writeFile(`${artifacts}/touch-row-inspection.png`,Buffer.from(inspected.data,'base64'));
await cdp('Input.dispatchKeyEvent',{type:'keyDown',key:'Tab',code:'Tab',windowsVirtualKeyCode:9});
await cdp('Input.dispatchKeyEvent',{type:'keyUp',key:'Tab',code:'Tab',windowsVirtualKeyCode:9});
if((await state()).snapshot.revision!==inspectionRevision)throw Error('Keyboard focus changed inclusion');
await click('Cut '+inspectLabel);
await until(async()=>!(await state()).pending && (await state()).snapshot.design?.cut_ready,'Restore authored row inclusion');
console.log('PASS: touch row inspection and keyboard focus never change inclusion or project revision');

await sleep(300);
await click('Menu Edit'); await click('Mats…'); await click('New…');
await until(async()=>!!await page('CutCraft.controls()["Save changes"]'),'Mat editor opened');
await sleep(200);
const matEditor=await cdp('Page.captureScreenshot',{format:'png'});
await fs.writeFile(`${artifacts}/mat-editor.png`,Buffer.from(matEditor.data,'base64'));
await click('Close dialog');
await until(async()=>!!(await state()).picture_key,'Initial source region');
await sleep(500);
const imageBeforeZoom=(await state()).picture_key;
await click('Zoom in');
await until(async()=>{const s=await state();return s.picture_key!==imageBeforeZoom && JSON.parse(s.picture_key)[1].zoom_step>JSON.parse(imageBeforeZoom)[1].zoom_step;},'Zoom produces a sharper region render');
if(!(await state()).picture)throw Error('Zoom cleared source artwork');
await click('Fit in window');
console.log('PASS: actual zoom requests and displays a new source region without clearing the artwork');
if(!process.env.CUTCRAFT_PRESENTATION_ONLY){
await type('Width','1 in');
await until(async()=>Math.abs((await state()).snapshot.ui.size_mm[0]-25.4)<.001,'Resize width');
await type('Force','12');
await until(async()=>(await state()).snapshot.ui.sheet.material.force===12,'Set force');
await click('Save');
await until(async()=>!!(await page('CutCraft.controls()["Save project"]')),'First Save opens Save As');
await click('Save project');
await until(async()=>!!(await state()).snapshot.design.saved && !(await state()).snapshot.design.unsaved,'Save project');
const viewRevision=(await state()).snapshot.revision;
const previewRect=await page('CutCraft.controls()["Preview canvas"]');
const panX=previewRect[0]+(previewRect[2]-previewRect[0])/4,panY=(previewRect[1]+previewRect[3])/2;
await cdp('Input.dispatchMouseEvent',{type:'mouseMoved',x:panX,y:panY});
await cdp('Input.dispatchMouseEvent',{type:'mousePressed',x:panX,y:panY,button:'middle',clickCount:1});
await cdp('Input.dispatchMouseEvent',{type:'mouseMoved',x:panX+200,y:panY+50,button:'middle',buttons:4});
await cdp('Input.dispatchMouseEvent',{type:'mouseReleased',x:panX+200,y:panY+50,button:'middle',clickCount:1});
await sleep(200);
const panned=await cdp('Page.captureScreenshot',{format:'png'});
await fs.writeFile(`${artifacts}/comparison-panned.png`,Buffer.from(panned.data,'base64'));
await click('Fit in window');
if((await state()).snapshot.revision!==viewRevision || (await state()).snapshot.design.unsaved)throw Error('Panning modified the project');
console.log('PASS: middle-button comparison pan and fit preserve the project; panned capture saved');
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
async function scrollPanelTo(label){
 await cdp('Input.dispatchMouseEvent',{type:'mouseMoved',x:1120,y:650});
 for(let n=0;n<16;n++){
  const r=await page(`CutCraft.controls()[${JSON.stringify(label)}]`);
  if(r && r[1]>180 && r[3]<780)return;
  await cdp('Input.dispatchMouseEvent',{type:'mouseWheel',x:1120,y:650,deltaX:0,deltaY:r&&r[1]<180?-250:250});await sleep(150);
 }
 throw Error('Could not scroll to '+label);
}
await scrollPanelTo('Cutting a piece of material'); await click('Cutting a piece of material');
await until(async()=>(await state()).snapshot.ui.sheet.piece_mm?.[0]===250,'Enable material piece');
await click('Move media');
await until(async()=>!!await page('CutCraft.controls()["Resize e"]'),'Media edge handle');
const mediaBefore=(await state()).snapshot.ui;
const edge=await page('CutCraft.controls()["Resize e"]');
const ex=(edge[0]+edge[2])/2,ey=(edge[1]+edge[3])/2;
await cdp('Input.dispatchMouseEvent',{type:'mouseMoved',x:ex,y:ey});
await cdp('Input.dispatchMouseEvent',{type:'mousePressed',x:ex,y:ey,button:'left',clickCount:1});
await cdp('Input.dispatchMouseEvent',{type:'mouseMoved',x:ex+25,y:ey+40,button:'left',buttons:1});
await cdp('Input.dispatchMouseEvent',{type:'mouseReleased',x:ex+25,y:ey+40,button:'left',clickCount:1});
await until(async()=>(await state()).snapshot.ui.sheet.piece_mm[0]>250,'Resize media east edge');
const mediaAfter=(await state()).snapshot.ui;
if(mediaAfter.sheet.piece_mm[1]!==250 || JSON.stringify(mediaAfter.sheet.piece_at_mm??[0,0])!==JSON.stringify(mediaBefore.sheet.piece_at_mm??[0,0]))throw Error('An east-edge drag moved the opposite edge or changed height');
if(mediaAfter.history.undo.length!==mediaBefore.history.undo.length+1)throw Error('A gesture must add exactly one Undo');
await click('Undo');await until(async()=>(await state()).snapshot.ui.sheet.piece_mm[0]===250,'Undo media edge drag');
await scrollPanelTo('Cutting a piece of material');await click('Cutting a piece of material');
await until(async()=>!(await state()).snapshot.ui.sheet.piece_mm,'Forget material piece');
await click('Move design');
console.log('PASS: media edge drag changes one dimension, keeps its opposite edge and creates one Undo');
// All linked corners preserve the opposite corner, through one shared command/Undo.
for(const [handle,dx,dy,fixed] of [['nw',10,10,[2,3]],['ne',-10,10,[0,3]],['sw',10,-10,[2,1]],['se',-10,-10,[0,1]]]){
 await sleep(250);
 const before=await state(),b=before.geometry.art_box_mm;
 const r=await page(`CutCraft.controls()["Resize ${handle}"]`),x=(r[0]+r[2])/2,y=(r[1]+r[3])/2;
 await cdp('Input.dispatchMouseEvent',{type:'mouseMoved',x,y});
 await cdp('Input.dispatchMouseEvent',{type:'mousePressed',x,y,button:'left',clickCount:1});
 await cdp('Input.dispatchMouseEvent',{type:'mouseMoved',x:x+dx,y:y+dy,button:'left',buttons:1});
 await cdp('Input.dispatchMouseEvent',{type:'mouseReleased',x:x+dx,y:y+dy,button:'left',clickCount:1});
 await until(async()=>{const s=await state();return !s.pending&&s.geometry.id!==before.geometry.id&&s.snapshot.ui.size_mm[0]<25.4;},'Resize '+handle);
 const after=await state();
 for(const axis of fixed)if(Math.abs(after.geometry.art_box_mm[axis]-b[axis])>.001)throw Error(handle+' moved its fixed corner');
 if(after.snapshot.ui.history.undo.length!==before.snapshot.ui.history.undo.length+1)throw Error(handle+' produced more than one Undo');
 await click('Undo');await until(async()=>{const s=await state();return !s.pending&&Math.abs(s.snapshot.ui.size_mm[0]-25.4)<.001&&s.geometry.art_box_mm.every((v,i)=>Math.abs(v-b[i])<.001);},'Undo '+handle);
 await until(async()=>{const shown=await page(`CutCraft.controls()["Resize ${handle}"]`);return shown?.every((v,i)=>Math.abs(v-r[i])<.25);},'Undo drawn on canvas '+handle);
}
await sleep(250);
const dimensionBefore=(await state()).geometry.art_box_mm;
const dimension=await page('CutCraft.controls()["Width dimension"]');
const dimensionX=(dimension[0]+dimension[2])/2,dimensionY=(dimension[1]+dimension[3])/2;
await cdp('Input.dispatchMouseEvent',{type:'mouseMoved',x:dimensionX,y:dimensionY});
for(const clickCount of [1,2]){
 await cdp('Input.dispatchMouseEvent',{type:'mousePressed',x:dimensionX,y:dimensionY,button:'left',clickCount});
 await cdp('Input.dispatchMouseEvent',{type:'mouseReleased',x:dimensionX,y:dimensionY,button:'left',clickCount});
 await sleep(50);
}
await type('Dimension','1.5 in');
await until(async()=>{const s=await state();return !s.pending&&Math.abs(s.snapshot.ui.size_mm[0]-38.1)<.001;},'Typed canvas dimension');
const dimensionAfter=(await state()).geometry.art_box_mm;
for(const axis of [0,1])if(Math.abs(dimensionBefore[axis]-dimensionAfter[axis])>.001)throw Error('Typed dimension moved the top-left corner');
await click('Undo');await until(async()=>Math.abs((await state()).snapshot.ui.size_mm[0]-25.4)<.001,'Undo typed dimension');
console.log('PASS: all four design corners use one Undo, preserve their anchors, and typed dimensions preserve top-left');

if(!process.env.CUTCRAFT_MEDIA_ONLY){
await click('Test Cut');await until(async()=>(await state()).snapshot.ui.screen==='test-cut','Test screen');
if((await state()).state.cutters[0].view.turn)throw Error('Toolbar Test Cut prepared a job before explicit review');
await until(async()=>!!await page('CutCraft.controls()["Preview canvas"]'),'Unprepared cut retains its mat preview');
const unprepared=await cdp('Page.captureScreenshot',{format:'png'});
await fs.writeFile(`${artifacts}/test-before-prepare.png`,Buffer.from(unprepared.data,'base64'));

await click('Cut pattern');await until(async()=>(await state()).cut_preview?.geometry,'Prepare test preview');
const preparedTurn=(await state()).state.cutters[0].view.turn.id;
await cdp('Page.reload');
await until(async()=>await page('!!window.CutCraft && Object.keys(CutCraft.controls()).length>0'),'Prepared cut reload ready');
await until(async()=>(await state()).cut_preview?.geometry,'Recover prepared preview after reload');
if((await state()).state.cutters[0].view.turn.id!==preparedTurn || (await state()).state.cutters[0].view.queue.length)throw Error('Reload duplicated the prepared turn');
await click('Load virtual mat');
await until(async()=>(await state()).state.cutters[0].view.mat.state==='fresh'&&(await state()).state.cutters[0].view.mat_loaded,'Fresh virtual mat');
await click('Test Cut');await until(async()=>(await state()).snapshot.ui.screen==='mat','Exit cancels loaded review');
if(!(await state()).state.cutters[0].view.turn.review_cancelled || (await state()).state.cutters[0].view.mat.state!=='fresh')throw Error('Exit must cancel review while preserving fresh material');
await click('Test Cut');await until(async()=>(await state()).snapshot.ui.screen==='test-cut','Return to cancelled review');
await click('Start');
if((await state()).state.cutters[0].view.execution)throw Error('Cancelled review must not enable Start');
await click('Prepare cut again');
await until(async()=>!(await state()).state.cutters[0].view.turn.review_cancelled && !(await state()).pending,'Explicitly prepare cancelled review again');
console.log('PASS: Exit cancels review without unloading; Start waits for explicit re-preparation');

await click('Start');await until(async()=>(await state()).state.cutters[0].view.turn?.tests===1,'Test completed');
await until(async()=>(await state()).state.cutters[0].view.mat_loaded,'Mat ready after first test');
await cdp('Page.reload');
await until(async()=>await page('!!window.CutCraft && Object.keys(CutCraft.controls()).length>0'),'Completed test reload ready');
await until(async()=>(await state()).cut_preview?.geometry,'Recover next pattern after reload');
if((await state()).state.cutters[0].view.turn.taken.length!==1)throw Error('Reload lost the completed test reservation');
await sleep(200);
const occupiedShot=await cdp('Page.captureScreenshot',{format:'png'});
await fs.writeFile(`${artifacts}/occupied-test.png`,Buffer.from(occupiedShot.data,'base64'));
const firstCopy=(await state()).cut_preview.copy.id;
await click('Test Cut');await until(async()=>(await state()).snapshot.ui.screen==='mat','Exit test to adjust settings');
if(!(await state()).state.cutters[0].view.turn.review_cancelled || (await state()).state.cutters[0].view.turn.taken.length!==1)throw Error('Exit discarded the occupied test area');

await type('Force','17');await until(async()=>(await state()).snapshot.ui.sheet.material.force===17,'Adjust force after test');
await click('Test Cut');await until(async()=>(await state()).snapshot.ui.screen==='test-cut','Return to test');
await click('Prepare updated cut');
await until(async()=>(await state()).cut_preview?.copy?.id!==firstCopy && (await state()).cut_preview?.material?.force===17,'Replace prepared settings');
if((await state()).state.cutters[0].view.turn.id!==preparedTurn || (await state()).state.cutters[0].view.turn.taken.length!==1 || (await state()).state.cutters[0].view.queue.length)throw Error('Settings replacement lost the turn or test reservation');
await click('Try another pattern');await click('Start');
await until(async()=>(await state()).state.cutters[0].view.turn?.tests===2,'Second test completed');
if((await state()).state.cutters[0].view.turn.taken.length!==2)throw Error('Test reservations lost');
await click('Cut');await until(async()=>(await state()).snapshot.ui.screen==='cut','Design cut screen');
await until(async()=>(await state()).state.cutters[0].view.mat_loaded,'Same loaded mat ready for design');
await click('Start');await until(async()=>(await state()).state.cutters[0].view.mat.state==='used','Main cut completed',45000);
console.log('PASS: toolbar navigation only, explicit prepare/Start, two tests and design on one loaded mat');
await until(async()=>{const h=(await state()).cut_history;return h?.executions?.[0]?.kind==='main' && h.executions[0].status==='Done';},'Durable Done outcome reaches the cut screen');
await click('Cut history…');await click('Download cut report');
await until(async()=>{try{return JSON.parse(await fs.readFile(`${artifacts}/cutcraft-cut-report.json`,'utf8')).executions[0].status==='Done';}catch{return false;}},'Download recorded cut outcomes');
await click('Close dialog');

await until(async()=>!(await state()).cut_preview?.copy,'Finished turn clears prepared preview');
await click('Test Cut');await until(async()=>(await state()).snapshot.ui.screen==='test-cut','New test after completed design');
await click('Cut pattern');await until(async()=>(await state()).cut_preview?.geometry,'Prepare another turn');
await until(async()=>(await state()).state.cutters[0].view.mat_loaded,'Used mat settled before unloading');
await sleep(200);
await click('Unload virtual mat');await until(async()=>(await state()).state.cutters[0].view.mat.state==='none','Unload used mat');
await sleep(200);
await click('Load virtual mat');await until(async()=>(await state()).state.cutters[0].view.mat.state==='fresh' && (await state()).state.cutters[0].view.mat_loaded,'Load fresh mat');
await click('Start');await until(async()=>(await state()).state.cutters[0].view.activity==='testing','Test running for modal Stop');
const stoppedExecution=(await state()).state.cutters[0].view.execution;
const activeCopy=(await state()).cut_preview.copy.id;
await until(async()=>(await state()).cut_preview?.motion?.passes?.length>0,'Sender pass ranges reach the desktop');
const runningShot=await cdp('Page.captureScreenshot',{format:'png'});
await fs.writeFile(`${artifacts}/estimated-progress.png`,Buffer.from(runningShot.data,'base64'));

await cdp('Page.reload');
await until(async()=>await page('!!window.CutCraft && Object.keys(CutCraft.controls()).length>0'),'Active cut reload ready');
await until(async()=>(await state()).cut_preview?.geometry && (await state()).cut_preview?.execution===stoppedExecution,'Recover exact active execution preview');
if((await state()).cut_preview.copy.id!==activeCopy)throw Error('Active reload changed the immutable cut copy');
await click('Menu Help');await click('Troubleshooting');
await until(async()=>!!(await page('CutCraft.controls()["Close dialog"]')),'Troubleshooting modal is open');
await until(async()=>!!(await page('CutCraft.controls()["Stop now"]')),'Stop remains visible through dialog');
await cdp('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
await cdp('Input.dispatchKeyEvent',{type:'keyUp',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
await until(async()=>['stopping','confirming-stop','idle'].includes((await state()).state.cutters[0].view.activity),'Modal Escape stops cut');
await until(async()=>(await state()).state.cutters[0].view.activity==='idle','Stop confirmed',45000);
await until(async()=>{const record=JSON.parse(await fs.readFile(`${artifacts}/data/private/executions/${stoppedExecution}.json`,'utf8'));return record.outcome?.ended==='stopped';},'Durable record confirms Stop, not completion');
if((await state()).state.cutters[0].view.mat.state!=='used')throw Error('Stopped test must require fresh loading');
console.log('PASS: prepare after finished cut and Escape Stop through a help dialog; fresh loading required');
await until(async()=>(await state()).cut_history?.executions?.[0]?.status==='Stopped','Durable Stopped outcome reaches the cut screen');

await cdp('Page.reload');
await until(async()=>await page('!!window.CutCraft && Object.keys(CutCraft.controls()).length>0'),'Reload ready');
await until(async()=>(await state()).snapshot.ui.sheet?.material?.force===17,'Material setting survives reload');
if((await state()).state.cutters[0].view.activity!=='idle')throw Error('Reload started a cut');
await until(async()=>(await state()).cut_history?.executions?.[0]?.status==='Stopped','Reload retains Stopped rather than inferring Done from idle');
console.log('PASS: durable Done/Stopped outcomes, private cut report download and outcome recovery after reload');

for(const [width,height] of [[1280,800],[1920,1080]]){
 await cdp('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:false});await sleep(300);
 const shot=await cdp('Page.captureScreenshot',{format:'png'});
 await fs.writeFile(`${artifacts}/desktop-${width}x${height}.png`,Buffer.from(shot.data,'base64'));
}
console.log('PASS: reload preserves settings without restarting motion; desktop screenshots captured');
}
}
// Exercise the overflow controls with the largest supported interface text.
await click((await state()).snapshot.ui.screen==='test-cut'?'Test Cut':'Cut Preview');
await until(async()=>(await state()).snapshot.ui.screen==='mat','Exit test screen');
await click('Menu Edit'); await click('Preferences…');
await type('Text size (10–24 points)','24');
await until(async()=>(await state()).snapshot.ui.font_size===24,'Largest text size');
await click('Close dialog');
await cdp('Emulation.setDeviceMetricsOverride',{width:600,height:900,deviceScaleFactor:1,mobile:false});
await sleep(500);
await fs.writeFile(`${artifacts}/narrow-controls.json`,JSON.stringify(await page('({controls:CutCraft.controls(),canvas:document.querySelector("canvas").getBoundingClientRect().toJSON(),innerWidth})')));
const diagnostic=await cdp('Page.captureScreenshot',{format:'png'});await fs.writeFile(`${artifacts}/narrow-before.png`,Buffer.from(diagnostic.data,'base64'));
await until(async()=>!!await page('CutCraft.controls()["Choose view"]'),'Compact desktop view selector');
await click('Choose view'); await click('Overlay');
await until(async()=>(await state()).snapshot.ui.screen==='overlay','Overflow view navigation');
await click('More tools'); await click('Fit in window');
await cdp('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
await cdp('Input.dispatchKeyEvent',{type:'keyUp',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
await sleep(200);
const toolbar=await page('CutCraft.controls()');
for(const label of ['Open','Choose view','More tools','Test Cut','Cut']){
 const r=toolbar[label];
 if(!r || r[0]<0 || r[2]>600 || Math.abs(r[1]-toolbar.Open[1])>2)throw Error('Toolbar overflow: '+label+JSON.stringify(r));
}
const narrow=await cdp('Page.captureScreenshot',{format:'png'});
await fs.writeFile(`${artifacts}/desktop-600-font24.png`,Buffer.from(narrow.data,'base64'));
await click('More tools'); await click('Save');
await until(async()=>!(await state()).pending,'Overflow Save finishes');
console.log('PASS: 600px desktop with 24pt text keeps primary controls visible and overflow views/zoom/Save usable');
await sleep(1500);
await page(`performance.clearResourceTimings();window.canvasTraffic={updates:0,pictures:0,frames:0};for(const [name,count] of [['state_update','updates'],['picture','pictures'],['inspect','frames']]){const original=CutCraft[name];CutCraft[name]=(...args)=>{canvasTraffic[count]++;return original(...args);};}`);
await sleep(10000);
const idleReads=await page("performance.getEntriesByType('resource').filter(e=>new URL(e.name).pathname==='/api/v1/design/snapshot').length");
await fs.writeFile(`${artifacts}/idle-refresh.json`,JSON.stringify({seconds:10,snapshot_reads:idleReads,canvas:await page('canvasTraffic')}));
if(idleReads>14)throw Error('Snapshot reads still feed a live refresh loop: '+idleReads);
console.log(`PASS: idle snapshot reads are bounded (${idleReads} in 10 seconds)`);
const traffic=await page('canvasTraffic');
if(traffic.updates>50 || traffic.frames>100 || traffic.pictures!==0)throw Error('Idle canvas still transfers/redraws unnecessarily: '+JSON.stringify(traffic));
console.log('PASS: idle canvas traffic '+JSON.stringify(traffic));
ws.close();
