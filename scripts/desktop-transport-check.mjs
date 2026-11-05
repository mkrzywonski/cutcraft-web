// Transport regressions: public browser state and fake HTTP; never real cutter traffic.
import {readFile} from 'node:fs/promises';
import vm from 'node:vm';
import assert from 'node:assert/strict';
const source=(await readFile(new URL('../apps/server/web/desktop/desktop.js',import.meta.url),'utf8'))
 .replace(/^import .*;$/m,'').split('try{await refresh();await init();')[0];
const calls=[];
let turn='another-copy', refusePreview=false, incarnation='session', previewGate=null, queue=[{job:{job_id:'my-copy'}}];
const context=vm.createContext({
 console,URLSearchParams,FormData,TypeError,Uint8Array,
 crypto:{randomUUID:()=> 'test-key'},
 localStorage:{getItem:()=>null,setItem(){}},
 document:{cookie:'',querySelector:()=>({addEventListener(){}})},
 window:{addEventListener(){}},
 EventSource:class {addEventListener(){} close(){}},
 fetch:async path=>{
  calls.push(path);
  let value={};let ok=true;
  if(path.endsWith('/me'))value={mode:'open'};
  else if(path.endsWith('/snapshot'))value={protocol:7,incarnation,revision:1,ui:{screen:'test-cut'}};
  else if(path.endsWith('/state'))value={cutters:[{id:'practice',view:{activity:'idle',turn:turn?{job:{job_id:turn}}:null,queue}}]};
  else if(path.includes('/preview?')){if(previewGate)await previewGate;value=refusePreview?{message:'Preview no longer available'}:{copy:{id:'my-copy'},geometry:{id:'frozen'}};ok=!refusePreview;}
  else throw Error('Unexpected request '+path);
  return {ok,status:ok?200:404,json:async()=>value};
 }
});
vm.runInContext(source,context);
await vm.runInContext("model.selectedCutter='practice';model.cut_preview={copy:{id:'my-copy'}};refresh()",context);
assert.equal(calls.some(p=>p.includes('/preview?')),false,'Waiting behind another person must not request their preview');
assert.equal(vm.runInContext('model.connected',context),true);
turn='my-copy';
await vm.runInContext('refresh()',context);
assert.equal(vm.runInContext('model.cut_preview.geometry.id',context),'frozen');
refusePreview=true;
await vm.runInContext('refresh()',context);
assert.equal(vm.runInContext('model.connected',context),true,'Preview refusal must not mark the whole service offline');
assert.equal(vm.runInContext('model.cut_preview.geometry',context),undefined,'Refused preview must not leave stale geometry');
await vm.runInContext('model.result={close_dialog:true};execute(async()=>{})',context);
assert.equal(vm.runInContext('model.result.close_dialog',context),undefined,'A later action must not replay an old dialog-close result');
console.log('PASS: waiting-copy isolation, preview refusal, and independent action results');

incarnation='other-session';
await vm.runInContext("model.cut_preview={copy:{id:'my-copy'},geometry:{id:'private'}};refresh()",context);
assert.equal(vm.runInContext('model.cut_preview.copy',context),undefined,'A different session must discard the previous private preview');
console.log('PASS: session changes discard private preview context');

refusePreview=false;
await vm.runInContext('model.cut_preview={};refresh()',context);
assert.equal(vm.runInContext('model.cut_preview.copy.id',context),'my-copy','Reload recovers the server-owned copy without issuing a command');
assert.equal(calls.some(p=>p.endsWith('/queue') || p.endsWith('/start')),false);
console.log('PASS: read-only prepared cut recovery');

let releasePreview;
previewGate=new Promise(resolve=>{releasePreview=resolve;});
const refreshing=vm.runInContext('model.cut_preview={};refresh()',context);
await new Promise(resolve=>setImmediate(resolve));
vm.runInContext("model.selectedCutter='another-cutter';model.cut_preview={}",context);
releasePreview();await refreshing;
assert.equal(vm.runInContext('model.cut_preview.geometry',context),undefined,'A late response must not replace the newly selected cutter preview');
console.log('PASS: switching cutters discards an in-flight preview');

previewGate=null; turn=null; queue=[];
await vm.runInContext("model.selectedCutter='practice';model.cut_preview={copy:{id:'my-copy'},geometry:{id:'finished'}};refresh()",context);
assert.equal(vm.runInContext('model.cut_preview.copy',context),undefined,'A finished turn must release the prepared copy so another cut can be prepared');
console.log('PASS: completed cuts release the desktop preparation state');

turn='my-copy';refusePreview=true;
previewGate=new Promise(resolve=>{releasePreview=resolve;});
const oldPreview=vm.runInContext("model.cut_preview={copy:{id:'my-copy'}};refresh()",context);
await new Promise(resolve=>setImmediate(resolve));
vm.runInContext("model.cut_preview={copy:{id:'replacement'},geometry:{id:'new-preview'}}",context);
releasePreview();await oldPreview;
assert.equal(vm.runInContext('model.cut_preview.geometry.id',context),'new-preview','A refused old preview must not erase a newer prepared copy');
console.log('PASS: late preview refusal preserves replacement geometry');
