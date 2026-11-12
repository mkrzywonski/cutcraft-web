// Transport regressions: public browser state and fake HTTP; never real cutter traffic.
import {readFile} from 'node:fs/promises';
import vm from 'node:vm';
import assert from 'node:assert/strict';
const source=(await readFile(new URL('../apps/server/web/desktop/desktop.js',import.meta.url),'utf8'))
 .replace(/^import .*;$/m,'').split('try{await refresh();await init();')[0];
const calls=[];
let turn='another-copy', refusePreview=false, incarnation='session', previewGate=null, queue=[{job:{job_id:'my-copy'}}], activity='idle', cutting=null, execution=null, designView=null, meIdentity={mode:'open',principal:'household'};
const startStorage=new Map();
const context=vm.createContext({
 sessionStorage:{getItem:key=>startStorage.get(key)??null,setItem:(key,value)=>startStorage.set(key,value),removeItem:key=>startStorage.delete(key)},
 console,URLSearchParams,FormData,TypeError,Uint8Array,
 crypto:{randomUUID:()=> 'test-key'},
 localStorage:{getItem:()=>null,setItem(){}},
 document:{cookie:'',querySelector:()=>({addEventListener(){}})},
 window:{addEventListener(){}},
 EventSource:class {addEventListener(){} close(){}},
 fetch:async path=>{
  calls.push(path);
  let value={};let ok=true;
  if(path.endsWith('/me'))value=meIdentity;
  else if(path.endsWith('/snapshot'))value={protocol:8,incarnation,revision:1,design:designView,ui:{screen:'test-cut'}};
  else if(path.endsWith('/state'))value={cutters:[{id:'practice',view:{activity,cutting,execution,turn:turn?{job:{job_id:turn}}:null,queue}}]};
  else if(path.includes('/preview?')){if(previewGate)await previewGate;value=refusePreview?{message:'Preview no longer available'}:{copy:{id:'my-copy'},execution,geometry:{id:'frozen'}};ok=!refusePreview;}
  else throw Error('Unexpected request '+path);
  return {ok,status:ok?200:404,json:async()=>value};
 }
});
vm.runInContext(source,context);
vm.runInContext("model.me={mode:'open',principal:'household'}",context);
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

previewGate=null;refusePreview=false;activity='testing';cutting={job_id:'my-copy'};execution='active-one';
turn='my-copy';
await vm.runInContext('model.cut_preview={};refresh()',context);
assert.equal(vm.runInContext('model.cut_preview.geometry.id',context),'frozen','Reload during motion must recover the owned frozen preview');
assert.equal(calls.some(p=>p.endsWith('/queue') || p.endsWith('/start')),false,'Recovery must stay read-only');
console.log('PASS: active-cut reload recovers frozen geometry without motion');
const fetchedBefore=calls.filter(p=>p.includes('/preview?')).length;
await vm.runInContext('refresh();',context);
assert.equal(calls.filter(p=>p.includes('/preview?')).length,fetchedBefore,'The same active execution reuses its frozen geometry');
execution='active-two';
await vm.runInContext('refresh();',context);
assert.equal(calls.filter(p=>p.includes('/preview?')).length,fetchedBefore+1,'A different execution refreshes the preview even for the same copy');
assert.equal(vm.runInContext('model.cut_preview.execution',context),'active-two');
console.log('PASS: active previews are cached by execution and immutable copy');


designView={source_sha256:'same-source',art:{id:'new-render',bounds:[0,0,20,20],scale:1}};
await vm.runInContext("model.snapshot.design={source_sha256:'same-source'};model.picture='old-pixels';model.picture_id='old-render';refresh()",context);
assert.equal(vm.runInContext('model.picture',context),'old-pixels','Keep the same source artwork while its replacement render is pending or fails');
designView={...designView,source_sha256:'different-source',art:{...designView.art,id:'different-render'}};
await vm.runInContext('refresh()',context);
assert.equal(vm.runInContext('model.picture',context),'','A different source must not display the previous artwork');
console.log('PASS: retain only same-source artwork during re-rendering');

meIdentity={mode:'users',user:'Same name',principal:'new-account'};
await vm.runInContext("model.me={mode:'users',user:'Same name',principal:'old-account'};model.picture='private-pixels';model.library={private:true};refresh()",context);
assert.equal(vm.runInContext('model.picture',context),'');
assert.equal(vm.runInContext('model.library.private',context),undefined,'A replacement account with the same name must not inherit cached library data');
meIdentity={mode:'users',user:null};
await vm.runInContext("model.picture='private-pixels';model.snapshot.design={name:'private'};refresh()",context);
assert.equal(vm.runInContext('model.picture',context),'');
assert.equal(vm.runInContext('model.snapshot.design',context),undefined,'Authentication loss clears private presentation state');
console.log('PASS: account changes and authentication loss clear private caches');

let releaseQueue;
context.queueGate=new Promise(resolve=>{releaseQueue=resolve;});
vm.runInContext("model.me={principal:'old-account'};model.snapshot.incarnation='old-session';dispatchQueue=queueGate;window.CutCraft.dispatch(JSON.stringify({action:'save',params:{}}));model.me.principal='new-account'",context);
releaseQueue();await vm.runInContext('dispatchQueue',context);
assert.equal(calls.some(p=>p.endsWith('/design/save')),false,'A queued Save from another account must not run in the new account');
console.log('PASS: queued private actions do not cross account boundaries');

// One render at a time; only the latest requested region may replace the cached image.
context.btoa=s=>Buffer.from(s,'binary').toString('base64');
let releaseImage1,releaseImage2;
const imageGate1=new Promise(resolve=>{releaseImage1=resolve;});
const imageGate2=new Promise(resolve=>{releaseImage2=resolve;});
const imageRequests=[];
context.fetch=async path=>{imageRequests.push(path);await (imageRequests.length===1?imageGate1:imageGate2);return {ok:true,arrayBuffer:async()=>new Uint8Array([1,2,3]).buffer};};
vm.runInContext("clearPrivate();model.me={principal:'owner'};model.snapshot={incarnation:'region-session',design:{source_sha256:'source',art:{id:'region-art',scale:1,bounds:[0,0,100,100]}}};model.picture='retained';requestPicture(model.snapshot.design.art,{art:'region-art',region:[0,0,100,100],width:100,height:100,zoom_step:0})",context);
vm.runInContext("requestPicture(model.snapshot.design.art,{art:'region-art',region:[25,25,75,75],width:200,height:200,zoom_step:4})",context);
assert.equal(imageRequests.length,1,'Viewport changes must coalesce behind the current render');
releaseImage1();await new Promise(resolve=>setImmediate(resolve));
assert.equal(imageRequests.length,2);
assert.equal(vm.runInContext('model.picture',context),'retained','An obsolete region must not replace the displayed image');
releaseImage2();await new Promise(resolve=>setImmediate(resolve));
assert.deepEqual(JSON.parse(vm.runInContext('JSON.stringify(model.picture_region)',context)),[25,25,75,75]);
assert.equal(vm.runInContext('model.picture',context),'AQID');
assert.match(imageRequests[1],/width=200/);
let releasePrivateImage;
const privateGate=new Promise(resolve=>{releasePrivateImage=resolve;});
context.fetch=async()=>{await privateGate;return {ok:true,arrayBuffer:async()=>new Uint8Array([4,5,6]).buffer};};
vm.runInContext("requestPicture(model.snapshot.design.art,{art:'region-art',region:[50,50,100,100],width:200,height:200,zoom_step:4});clearPrivate();model.me={principal:'other-owner'}",context);
releasePrivateImage();await new Promise(resolve=>setImmediate(resolve));
assert.equal(vm.runInContext('model.picture',context),'','A late render must not repopulate private caches after an identity change');
console.log('PASS: coalesced region renders retain the image and reject obsolete or cross-account replies');

// An unknown Start survives reload and never becomes a fresh request or automatic retry.
const startCalls=[];let lookupAnswer='missing',postAnswer='lost',startId=0;
context.crypto.randomUUID=()=>`start-key-${++startId}`;
context.fetch=async(path,options={})=>{
 startCalls.push({path,method:options.method,body:options.body});
 if(path.endsWith('/start')){
  assert.equal(startStorage.size,1,'Remember the exact request before sending motion');
  if(postAnswer==='lost')throw new TypeError('Network unavailable');
  return {ok:postAnswer==='accepted',status:postAnswer==='accepted'?200:409,json:async()=>postAnswer==='accepted'?{answer:'started'}:{error:'stale-action',message:'Review the changed cutter state.'}};
 }
 if(path.includes('/starts/')){
  if(lookupAnswer==='offline')throw new TypeError('Network unavailable');
  return {ok:lookupAnswer==='accepted',status:lookupAnswer==='accepted'?200:404,json:async()=>lookupAnswer==='accepted'?{answer:'started'}:{error:'start-not-found',message:'No accepted Start with this key is recorded.'}};
 }
 throw Error('Unexpected Start test request '+path);
};
vm.runInContext("refresh=async()=>{};clearPrivate();model.me={principal:'start-owner'};model.pending=false",context);
const originalIntent="{action:'start',params:{cutter:'practice',start:{boot:'boot',turn:'turn',job_id:'copy',revision:2,generation:3,action_epoch:4,kind:'test'}}}";
await vm.runInContext(`dispatch(${originalIntent})`,context);
assert.match(vm.runInContext('model.message',context),/Start is not confirmed/);
assert.equal(startCalls.length,2);
const originalBody=startCalls[0].body;
await vm.runInContext(`dispatch(${originalIntent})`,context);
assert.equal(startCalls.length,2,'An unresolved Start cannot mint a second key');
vm.runInContext("clearPrivate();restoreStart()",context);
assert.equal(vm.runInContext('model.start_recovery.body.key',context),'start-key-1','Reload restores the original key');
vm.runInContext("clearPrivate();model.me={principal:'another-owner'};restoreStart()",context);
assert.equal(vm.runInContext('model.start_recovery',context),null,'Another account cannot inherit the request');
vm.runInContext("model.me={principal:'start-owner'};restoreStart()",context);
lookupAnswer='offline';
await vm.runInContext("dispatch({action:'retry-start',params:{}})",context);
assert.equal(startCalls.filter(c=>c.method==='POST').length,1,'Failed reconciliation must not send motion');
lookupAnswer='missing';postAnswer='accepted';
await vm.runInContext("dispatch({action:'retry-start',params:{}})",context);
assert.equal(startCalls.at(-2).method,'GET','A deliberate retry first looks up its result');
assert.equal(startCalls.at(-1).body,originalBody,'The retry reuses every original field and key');
assert.equal(startStorage.size,0);
assert.equal(vm.runInContext('model.start_recovery',context),null);
postAnswer='lost';lookupAnswer='accepted';
await vm.runInContext(`dispatch(${originalIntent})`,context);
assert.equal(startStorage.size,0,'A lost reply with an accepted lookup clears recovery');
assert.equal(vm.runInContext('model.message',context),'');
postAnswer='refused';
await vm.runInContext(`dispatch(${originalIntent})`,context);
assert.equal(startStorage.size,0,'An explicit refusal resolves the request');
assert.match(vm.runInContext('model.message',context),/Review the changed cutter state/);
startStorage.set('cutcraft.pending-start.start-owner','broken json');
const beforeBroken=startCalls.length;
await vm.runInContext(`dispatch(${originalIntent})`,context);
assert.equal(startCalls.length,beforeBroken,'Unreadable recovery state cannot authorize a new Start');
console.log('PASS: durable unknown Start, account isolation, no new key, and exact deliberate retry after lookup');
startStorage.clear();
vm.runInContext(`model.me={principal:'start-owner'};saveStart({owner:'start-owner',cutter:'practice',body:${originalBody}})`,context);
const beforeAccountChange=startCalls.length;
context.fetch=async()=>{
 vm.runInContext("clearPrivate();model.me={principal:'changed-during-lookup'}",context);
 return {ok:false,status:404,json:async()=>({error:'start-not-found'})};
};
await vm.runInContext("dispatch({action:'retry-start',params:{}})",context);
assert.equal(startCalls.length,beforeAccountChange,'An account change during reconciliation cannot send the original request');
assert.equal(vm.runInContext('model.start_recovery',context),null);
assert.equal(startStorage.size,1,'The original account retains its recovery record');
const saveSession=context.sessionStorage.setItem;
context.sessionStorage.setItem=()=>{throw Error('Session storage is blocked');};
context.fetch=async()=>{throw Error('Must not send before remembering Start');};
await vm.runInContext(`dispatch(${originalIntent})`,context);
assert.match(vm.runInContext('model.message',context),/Nothing was sent/);
context.sessionStorage.setItem=saveSession;
console.log('PASS: account changes during reconciliation and unavailable session storage cannot send Start');
