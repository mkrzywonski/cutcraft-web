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
 console,URLSearchParams,FormData,TypeError,Uint8Array,AbortSignal,
 crypto:{randomUUID:()=> 'test-key'},
 localStorage:{getItem:()=>null,setItem(){}},
 document:{cookie:'',querySelector:()=>({addEventListener(){},remove(){}})},
 window:{addEventListener(){}},
 EventSource:class {addEventListener(){} close(){}},
 fetch:async path=>{
  calls.push(path);
  let value={};let ok=true;
  if(path.endsWith('/me'))value=meIdentity;
  else if(path.endsWith('/snapshot'))value={protocol:9,incarnation,revision:1,design:designView,ui:{screen:'test-cut'}};
  else if(path.endsWith('/history'))value={cutter:'practice',executions:[]};
  else if(path.endsWith('/state'))value={cutters:[{id:'practice',view:{activity,cutting,execution,turn:turn?{job:{job_id:turn}}:null,queue}}]};
  else if(path.includes('/preview?')){if(previewGate)await previewGate;value=refusePreview?{message:'Preview no longer available'}:{copy:{id:'my-copy'},execution,geometry:{id:'frozen'}};ok=!refusePreview;}
  else throw Error('Unexpected request '+path);
  return {ok,status:ok?200:404,json:async()=>value};
 }
});
vm.runInContext(source,context);
const realRefresh=vm.runInContext("refresh",context);
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
startStorage.set('cutcraft.pending-start.household','invalid JSON');
await vm.runInContext('refresh()',context);
assert.equal(vm.runInContext('model.start_recovery.unreadable',context),true);
assert.equal(vm.runInContext('model.connected',context),true,'Unreadable Start storage must not block normal project refresh');
assert.equal(vm.runInContext('model.snapshot.incarnation',context),incarnation);
startStorage.clear();await vm.runInContext('refresh()',context);
console.log('PASS: corrupt Start storage blocks only Start, not project refresh');

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

// Only an observed changed fence followed by a missing receipt can retire an unknown Start.
const resetRecovery=()=>{
 startStorage.clear();
 vm.runInContext(`clearPrivate();model.me={principal:'start-owner'};model.pending=false;model.connected=true;saveStart({owner:'start-owner',cutter:'practice',body:${originalBody}})`,context);
};
let reconcileStatus=404,reconcileGate=null;
const reconciliationCalls=[];
context.fetch=async(path,options={})=>{
 reconciliationCalls.push({path,method:options.method});
 assert.equal(options.method,'GET','Reconciliation never sends motion');
 if(reconcileGate)await reconcileGate;
 if(reconcileStatus==='offline')throw new TypeError('Network unavailable');
 return {ok:reconcileStatus===200,status:reconcileStatus,json:async()=>reconcileStatus===200?{answer:'started'}:{code:reconcileStatus===404?'start-not-found':'unauthorized',message:'Sign in again.'}};
};
const currentFence={boot:'boot',cutters:[{id:'practice',view:{action_epoch:4,mat:{generation:3}}}]};
resetRecovery();
await vm.runInContext(`reconcileStart(${JSON.stringify(currentFence)})`,context);
assert.equal(reconciliationCalls.length,0,'Unchanged fences retain uncertainty without automatic retries');
for(const changed of [
 {boot:'restarted',cutters:currentFence.cutters},
 {boot:'boot',cutters:[{id:'practice',view:{action_epoch:5,mat:{generation:3}}}]},
 {boot:'boot',cutters:[{id:'practice',view:{action_epoch:4,mat:{generation:null}}}]},
]){
 resetRecovery();await vm.runInContext(`reconcileStart(${JSON.stringify(changed)})`,context);
 assert.equal(startStorage.size,0,'A later missing receipt resolves a fenced-out request');
 assert.match(vm.runInContext('model.message',context),/can no longer run/);
}
const changedFence={...currentFence,boot:'restarted'};
resetRecovery();reconcileStatus=200;
await vm.runInContext(`reconcileStart(${JSON.stringify(changedFence)})`,context);
assert.equal(startStorage.size,0,'An accepted receipt also resolves the request after an epoch change');
assert.equal(vm.runInContext('model.message',context),'The original Start was confirmed.','Automatic reconciliation replaces the old uncertainty message');
resetRecovery();reconcileStatus='offline';
await assert.rejects(vm.runInContext(`reconcileStart(${JSON.stringify(changedFence)})`,context),/not confirmed/);
assert.equal(startStorage.size,1,'An unavailable lookup never resolves uncertainty');
for(const status of [401,403]){
 resetRecovery();reconcileStatus=status;
 await assert.rejects(vm.runInContext('lookupStart(model.start_recovery)',context),/Sign in again/);
 assert.equal(vm.runInContext('model.me.login_required',context),true,'Authentication failure requests sign-in');
 assert.equal(vm.runInContext('model.connected',context),true,'Authentication is not a network outage');
 assert.equal(startStorage.size,1,'Signing out retains the original request for its owner');
}
resetRecovery();reconcileStatus=404;
let releaseReconcile;reconcileGate=new Promise(resolve=>{releaseReconcile=resolve;});
const lateLookup=vm.runInContext(`reconcileStart(${JSON.stringify(changedFence)})`,context);
vm.runInContext("saveStart({...model.start_recovery,body:{...model.start_recovery.body,key:'newer-start'}})",context);
releaseReconcile();await lateLookup;reconcileGate=null;
assert.equal(vm.runInContext('model.start_recovery.body.key',context),'newer-start','A late lookup cannot clear a newer unresolved request');
assert.equal(JSON.parse([...startStorage.values()][0]).body.key,'newer-start');

for(const status of [503,401]){
 resetRecovery();reconcileStatus=status;
 reconcileGate=new Promise(resolve=>{releaseReconcile=resolve;});
 const oldLookup=vm.runInContext(`reconcileStart(${JSON.stringify(changedFence)})`,context);
 vm.runInContext("saveStart({...model.start_recovery,body:{...model.start_recovery.body,key:'newer-start'}})",context);
 releaseReconcile();await assert.rejects(oldLookup);reconcileGate=null;
 assert.equal(vm.runInContext('model.connected',context),true,'A late failure cannot disconnect a newer recovery context');
 assert.equal(vm.runInContext('model.start_recovery.body.key',context),'newer-start');
 assert.notEqual(vm.runInContext('model.me.login_required',context),true);
}
console.log('PASS: changed-fence recovery is read-only, preserves unknown outcomes and newer requests, and distinguishes authentication');


// A stalled receipt endpoint must not stall geometry, live events or later refreshes.
context.refresh=realRefresh;
resetRecovery();
let releaseReceipt;
let receiptGate=new Promise(resolve=>{releaseReceipt=resolve;});
let receiptCount=0,geometryCount=0;
context.fetch=async(path,options={})=>{
 assert.equal(options.method,'GET','Background receipt recovery is read-only');
 let value;
 if(path.endsWith('/me'))value={mode:'open',principal:'start-owner'};
 else if(path.endsWith('/snapshot'))value={protocol:9,incarnation:'fresh-project',revision:1,design:{geometry:{id:'fresh-geometry'}},ui:{screen:'side-by-side'}};
 else if(path.endsWith('/state'))value=changedFence;
 else if(path.endsWith('/geometry')){geometryCount++;value={id:'fresh-geometry'};}
 else if(path.includes('/starts/')){
  receiptCount++;
  assert.ok(options.signal,'Receipt reads have a bounded lifetime');
  await receiptGate;
  return {ok:false,status:503,json:async()=>({message:'Receipt service unavailable'})};
 }else throw Error('Unexpected refresh request '+path);
 return {ok:true,status:200,json:async()=>value};
};
await vm.runInContext('refresh()',context);
assert.equal(vm.runInContext('model.geometry.id',context),'fresh-geometry');
assert.ok(vm.runInContext('live',context),'Live updates are connected before receipt recovery');
assert.equal(vm.runInContext('refreshing',context),false);
assert.equal(geometryCount,1);
assert.equal(receiptCount,1);
await vm.runInContext('refresh()',context);
assert.equal(receiptCount,1,'Refreshes do not multiply a stalled receipt lookup');
releaseReceipt();await vm.runInContext('recoveryFlight',context);
assert.equal(startStorage.size,1,'A failed receipt service does not resolve Start');
assert.match(vm.runInContext('model.message',context),/not confirmed/);
await vm.runInContext('refresh()',context);
await vm.runInContext('recoveryFlight',context);
assert.equal(receiptCount,2,'A later refresh can retry the read after failure');
assert.equal(vm.runInContext('model.geometry.id',context),'fresh-geometry');
assert.ok(vm.runInContext('live',context));
console.log('PASS: hanging/failed receipt recovery cannot block geometry, live connection or later refreshes');

resetRecovery();
const abortLookup=new AbortController();
context.AbortSignal={timeout:ms=>{assert.equal(ms,10000);return abortLookup.signal;}};
context.fetch=async(_path,options)=>new Promise((_resolve,reject)=>options.signal.addEventListener('abort',()=>reject(options.signal.reason),{once:true}));
const timedLookup=vm.runInContext('lookupStart(model.start_recovery)',context);
abortLookup.abort(new DOMException('Receipt lookup timed out','TimeoutError'));
await assert.rejects(timedLookup,/not confirmed/);
assert.equal(startStorage.size,1);
assert.equal(vm.runInContext('model.start_recovery.lookup_failed',context),true);
console.log('PASS: receipt timeout retains uncertainty and never sends motion');

context.AbortSignal=AbortSignal;
// History reads must never delay refresh or reveal the previous account's outcomes.
let releaseHistory;
const savedFetch=context.fetch;
context.fetch=async(path,...rest)=>path.endsWith('/history')
 ? await new Promise(resolve=>{releaseHistory=()=>resolve({ok:true,json:async()=>({cutter:'practice',executions:[{started:1,status:'Done',execution:'private-old-account'}]})});})
 : savedFetch(path,...rest);
vm.runInContext("model.me.principal='owner-before';model.selectedCutter='practice';model.snapshot.ui={screen:'cut'};historyFlight=null;queueCutHistory()",context);
assert.ok(releaseHistory);
vm.runInContext("clearPrivate();model.me.principal='owner-after'",context);
releaseHistory();await new Promise(r=>setTimeout(r,0));
assert.equal(vm.runInContext("JSON.stringify(model.cut_history)",context),'{}');
context.fetch=savedFetch;
console.log('PASS: late cut history cannot reveal the previous account after switching users');

context.fetch=async()=>await new Promise(resolve=>{releaseHistory=()=>resolve({ok:true,json:async()=>({cutter:'practice',executions:[{started:1,status:'Done'}]})});});
vm.runInContext("model.me.principal='same-owner';model.snapshot.ui={screen:'cut'};queueCutHistory()",context);
vm.runInContext("clearPrivate();model.me.login_required=true",context);
releaseHistory();await new Promise(r=>setTimeout(r,0));
assert.equal(vm.runInContext("JSON.stringify(model.cut_history)",context),'{}','Authentication loss retires history reads even before the principal field changes');
context.fetch=savedFetch;
console.log('PASS: authentication loss rejects late history even when the account id has not yet changed');

// Exit cancels only this account's review, before changing screens.
const exitRequests=[];
context.fetch=async(path,options)=>{
 exitRequests.push({path,body:JSON.parse(options.body)});
 return {ok:true,status:200,json:async()=>({answer:'review-cancelled'})};
};
vm.runInContext(`model.me={principal:'exit-owner'};model.selectedCutter='practice';model.state={boot:'exit-boot',cutters:[{id:'practice',view:{action_epoch:12,turn:{id:'own-turn',actor:{principal:'exit-owner'}},queue:[]}}]}`,context);
await vm.runInContext('cancelOwnReview()',context);
assert.equal(exitRequests.length,1);
assert.equal(exitRequests[0].body.turn,'own-turn');
assert.equal(exitRequests[0].body.action_epoch,12);
vm.runInContext("model.state.cutters[0].view.turn.actor.principal='someone-else'",context);
await vm.runInContext('cancelOwnReview()',context);
assert.equal(exitRequests.length,1,'Exit must never cancel another account’s review');
vm.runInContext("model.cut_preview={copy:{id:'queued-copy'}};model.state.cutters[0].view.queue=[{turn:'queued-turn',actor:{principal:'exit-owner'},job:{job_id:'queued-copy'}}]",context);
await vm.runInContext('cancelOwnReview()',context);
assert.equal(exitRequests[1].body.turn,'queued-turn');
context.fetch=async()=>({ok:false,status:409,json:async()=>({message:'Stop the cut first.'})});
await assert.rejects(vm.runInContext('cancelOwnReview()',context),/Stop the cut first/);
console.log('PASS: Exit cancels only an owned current/queued review and propagates refusal');

// Cancellation bypasses the edit queue but targets only the displayed preparation.
const cancelRequests=[];let finishCancellation;
vm.runInContext("refresh=async()=>{};clearPrivate();model.me={principal:'preparer'};model.connected=true;model.pending=true;model.snapshot.incarnation='prepare-session';model.snapshot.busy_key='preparing-now'",context);
context.fetch=async(path,options)=>{
 cancelRequests.push({path,body:JSON.parse(options.body)});
 return await new Promise(resolve=>{finishCancellation=()=>resolve({ok:true,status:200,json:async()=>({answer:'applied',value:{cancel_requested:true}})});});
};
const cancellation=vm.runInContext("dispatch({action:'cancel-preparation',params:{incarnation:'prepare-session',key:'preparing-now'}})",context);
assert.equal(cancelRequests.length,1,'A pending edit must not swallow cancellation');
assert.equal(cancelRequests[0].body.id,'design.cancel');
assert.equal(cancelRequests[0].body.params.key,'preparing-now');
await vm.runInContext("dispatch({action:'cancel-preparation',params:{incarnation:'prepare-session',key:'preparing-now'}})",context);
assert.equal(cancelRequests.length,1,'Duplicate clicks are coalesced');
finishCancellation();await cancellation;
assert.equal(vm.runInContext('model.pending',context),true,'Cancel must not release the original edit lock');
assert.match(vm.runInContext('model.message',context),/Cancellation requested/);
await vm.runInContext("dispatch({action:'cancel-preparation',params:{incarnation:'older-session',key:'preparing-now'}})",context);
await vm.runInContext("dispatch({action:'cancel-preparation',params:{incarnation:'prepare-session',key:'older-request'}})",context);
assert.equal(cancelRequests.length,1,'Stale UI cannot cancel another preparation');
const lateCancellation=vm.runInContext("dispatch({action:'cancel-preparation',params:{incarnation:'prepare-session',key:'preparing-now'}})",context);
vm.runInContext("clearPrivate();model.me.principal='new-user';model.message='New account'",context);
finishCancellation();await lateCancellation;
assert.equal(vm.runInContext('model.message',context),'New account');
assert.equal(vm.runInContext('model.cancelling',context),false);
console.log('PASS: preparation cancellation bypasses pending edits, fences its target, and retires cross-account replies');

let finishReport,reportDownloads=0;
context.Blob=Blob;
context.URL={createObjectURL:()=> 'blob:report',revokeObjectURL:()=>{}};
context.setTimeout=fn=>{fn();};
context.document.createElement=()=>({click:()=>{reportDownloads++;}});
vm.runInContext("clearPrivate();model.me={principal:'report-owner'};model.pending=false;model.selectedCutter='practice'",context);
context.fetch=async(path)=>{
 assert.equal(path,'/api/v1/cutters/practice/report');
 return await new Promise(resolve=>{finishReport=()=>resolve({ok:true,status:200,json:async()=>({schema:1,executions:[]})});});
};
const reportDownload=vm.runInContext("dispatch({action:'download-cut-report',params:{}})",context);
finishReport();await reportDownload;
assert.equal(reportDownloads,1,'A report is available before any execution');
const staleReportDownload=vm.runInContext("dispatch({action:'download-cut-report',params:{}})",context);
vm.runInContext("clearPrivate();model.me.principal='another-account'",context);
finishReport();await staleReportDownload;
assert.equal(reportDownloads,1,'Late private report must never download after account change');
console.log('PASS: reports are fetched through the authorized endpoint and never downloaded after an account change');

// A font catalog includes the current design's font names; late replies are private.
let finishFonts;
vm.runInContext("clearPrivate();model.me={principal:'font-owner'};model.pending=false",context);
context.fetch=async()=>await new Promise(resolve=>{finishFonts=()=>resolve({ok:true,json:async()=>({fonts:[],project_fonts:[{family:'Private project font'}]})});});
const staleFonts=vm.runInContext('refreshFonts()',context);
vm.runInContext("clearPrivate();model.me.principal='next-font-owner'",context);
finishFonts();await staleFonts;
assert.equal(vm.runInContext('JSON.stringify(model.fonts)',context),'{}');
context.fetch=async()=>await new Promise(resolve=>{finishFonts=()=>resolve({ok:true,blob:async()=>new Blob(['font'])});});
const staleFontDownload=vm.runInContext("dispatch({action:'font-download',params:{id:'opaque-face'}})",context);
vm.runInContext("clearPrivate();model.me.principal='last-font-owner'",context);
finishFonts();await staleFontDownload;
assert.equal(reportDownloads,1,'Late font download cannot cross accounts');
let picker;let fontUploads=0;
context.document.createElement=()=>picker={click(){},remove(){this.removed=true;},files:[{name:'private.ttf',size:1024}]};
context.document.body={append(){}};
context.document.querySelector=()=>picker;
context.fetch=async()=>{fontUploads++;throw Error('Unexpected upload');};
await vm.runInContext("dispatch({action:'font-upload',params:{}})",context);
vm.runInContext("clearPrivate();model.me.principal='changed-at-picker'",context);
assert.equal(picker.removed,true,'Account change removes the stale file picker');
await picker.onchange();
assert.equal(fontUploads,0,'A selected file from a retired account is never uploaded');
console.log('PASS: font catalogs, downloads and file pickers retire on account changes');

vm.runInContext(`fontCatalog={fonts:Array.from({length:12000},(_,i)=>({family:'Family '+i,style:'Regular'})),installed:[],project_fonts:[]};filterFonts('')`,context);
assert.equal(vm.runInContext('model.fonts.fonts.length',context),100);
assert.equal(vm.runInContext('model.fonts.matches',context),12000);
vm.runInContext("filterFonts('Family 11999')",context);
assert.equal(vm.runInContext('model.fonts.fonts[0].family',context),'Family 11999');
assert.equal(vm.runInContext('model.fonts.matches',context),1);
vm.runInContext('clearPrivate()',context);
assert.equal(vm.runInContext('fontCatalog',context),null);
console.log('PASS: font search covers the full catalog while bounding per-frame UI data');
