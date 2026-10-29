// Transport regressions: public browser state and fake HTTP; never real cutter traffic.
import {readFile} from 'node:fs/promises';
import vm from 'node:vm';
import assert from 'node:assert/strict';
const source=(await readFile(new URL('../apps/server/web/desktop/desktop.js',import.meta.url),'utf8'))
 .replace(/^import .*;$/m,'').split('try{await refresh();await init();')[0];
const calls=[];
let turn='another-copy', refusePreview=false, incarnation='session';
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
  else if(path.endsWith('/snapshot'))value={protocol:6,incarnation,revision:1,ui:{screen:'test-cut'}};
  else if(path.endsWith('/state'))value={cutters:[{id:'practice',view:{activity:'idle',turn:{job:{job_id:turn}}}}]};
  else if(path.includes('/preview?')){value=refusePreview?{message:'Preview no longer available'}:{geometry:{id:'frozen'}};ok=!refusePreview;}
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
