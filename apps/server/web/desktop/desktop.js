// Browser transport only. Geometry and all desktop widgets are drawn by the Rust UI.
// Mutations retain the exact request through a lost reply; no optimistic project state.
import init, {start} from '/desktop/pkg/cutcraft_ui.js';
const PROTOCOL=6;
const model={snapshot:{protocol:PROTOCOL,incarnation:"",revision:0,materials:[]},geometry:{id:""},state:{},library:{},uploads:{},me:{},connected:false,pending:false,message:'',result:{},result_id:0,picture:'',picture_id:''};
try{model.preferences=JSON.parse(localStorage.getItem("cutcraft.desktop")||"{}");}catch{model.preferences={};}
let controls={},refreshing=false,live=null,uploadMode={},pictureFlight='';
const key=()=>crypto.randomUUID?.()||Array.from(crypto.getRandomValues(new Uint8Array(16)),x=>x.toString(16).padStart(2,'0')).join('');
function csrf(){return document.cookie.split(';').map(s=>s.trim()).find(s=>s.startsWith('cpi_csrf='))?.split('=').slice(1).join('=')||'';}
async function api(path,method='GET',body){
 const headers={'x-cutcraft-protocol':String(PROTOCOL),'x-csrf-token':decodeURIComponent(csrf())};
 if(body!==undefined && !(body instanceof FormData))headers['content-type']='application/json';
 const r=await fetch(path,{method,credentials:'same-origin',headers,body:body instanceof FormData?body:body===undefined?undefined:JSON.stringify(body)});
 const value=await r.json();
 if(!r.ok || value.answer==='refused'){const e=new Error(value.message||`Request failed (${r.status})`);e.code=value.code||value.error;e.status=r.status;throw e;}return value;
}
function request(id,params={}){return{id,params,key:key(),incarnation:model.snapshot.incarnation,revision:model.snapshot.revision};}
async function command(id,params={},fence={}){
 const body={...request(id,params),...fence};
 try{return await api('/api/v1/design/command','POST',body);}
 catch(e){if(e instanceof TypeError){model.message='Connection lost. Checking whether your change was applied…';const answer=await api(`/api/v1/design/lookup/${body.key}`);if(answer.answer==='applied')return answer;if(answer.answer==='refused')throw new Error(answer.message);throw new Error('The change was not applied. Review the current project and try again.');}throw e;}
}
async function refresh(){
 if(refreshing)return;refreshing=true;
 try{
  const me=await api('/api/v1/me');model.me={...me,login_required:me.mode==='users'&&!me.user};
  if(model.me.login_required || me.must_change || me.mode==='locked'){model.connected=true;live?.close();live=null;return;}
  const [snapshot,state]=await Promise.all([api('/api/v1/design/snapshot'),api('/api/v1/state')]);
  if(snapshot.protocol!==PROTOCOL)throw new Error('The server was updated. Reload this page.');
  const old=model.snapshot;
  if(old.incarnation && old.incarnation!==snapshot.incarnation){model.cut_preview={};model.result={};model.geometry={id:""};model.picture="";model.picture_id="";}
  model.snapshot=snapshot;model.state=state;model.connected=true;
  document.title=(snapshot.design?.name?`${snapshot.design.name} — `:'')+'CutCraft';
  if(snapshot.design?.geometry?.id && (snapshot.design.geometry.id!==old.design?.geometry?.id || !model.geometry.id)){
   const geometry=await api('/api/v1/design/geometry');if(model.snapshot.incarnation===snapshot.incarnation && model.snapshot.design?.geometry?.id===geometry.id)model.geometry=geometry;
  }
  const cutter=model.state.cutters?.find(c=>c.id===model.selectedCutter);
  if(['cut','test-cut'].includes(snapshot.ui.screen) && cutter?.view?.turn && model.cut_preview?.copy && cutter.view.turn.job.job_id===model.cut_preview.copy.id && cutter.view.activity==='idle'){
    try {
    const preview=await api(`/api/v1/cutters/${encodeURIComponent(cutter.id)}/preview?kind=${snapshot.ui.screen==='test-cut'?'test':'main'}`);
    model.cut_preview={...preview,copy:model.cut_preview?.copy};
    } catch(e) {model.cut_preview={copy:model.cut_preview.copy};model.message=e.message;}
  }
  if(!snapshot.design){model.geometry={id:""};model.picture='';model.picture_id='';}
  const art=snapshot.design?.art;
  if(art?.id!==model.picture_id){model.picture='';}
  if(art && art.id!==model.picture_id && pictureFlight!==art.id){pictureFlight=art.id;void picture(art).finally(()=>{if(pictureFlight===art.id)pictureFlight='';});}
  if(!live){live=new EventSource(`/api/v1/design/live?protocol=${PROTOCOL}`);live.addEventListener('snapshot',()=>refresh());live.onerror=()=>{model.connected=false;};}
 }catch(e){model.connected=false;model.message=e.message;}finally{refreshing=false;}
}
async function picture(art){
 try{const [x0,y0,x1,y1]=art.bounds.map(n=>n/art.scale);const width=1200,height=Math.max(1,Math.min(1600,Math.round(width*(y1-y0)/(x1-x0))));
  const q=new URLSearchParams({art:art.id,x0,y0,x1,y1,zoom_step:0,width,height});
  const r=await fetch(`/api/v1/design/picture?${q}`,{headers:{'x-cutcraft-protocol':String(PROTOCOL)}});if(!r.ok)return;
  const bytes=new Uint8Array(await r.arrayBuffer());let s='';for(let i=0;i<bytes.length;i+=8192)s+=String.fromCharCode(...bytes.subarray(i,i+8192));
  if(model.snapshot.design?.art?.id===art.id){model.picture=btoa(s);model.picture_id=art.id;}
 }catch{/* Preview failures do not replace the last complete picture. */}
}
function savedBody(params){return {...params,key:key(),incarnation:model.snapshot.incarnation,revision:model.snapshot.revision};}
async function open(body){try{await api('/api/v1/design/open','POST',savedBody(body));model.result={close_dialog:true};}catch(e){if(e.code==='unsaved-changes'&&confirm('The current design has unsaved changes. Open this file and keep the previous work as a draft?')){await api('/api/v1/design/open','POST',savedBody({...body,replace_unsaved:true}));}else throw e;}}
async function upload(file){if(!file)return;await execute(async()=>{const r=await fetch(`/api/v1/design/upload?name=${encodeURIComponent(file.name)}`,{method:'POST',headers:{'x-cutcraft-protocol':String(PROTOCOL),'x-csrf-token':decodeURIComponent(csrf())},body:file});const result=await r.json();if(!r.ok)throw new Error(result.message);const uploaded=result.design;if(uploadMode.update){await api('/api/v1/design/update','POST',savedBody({design:uploaded.id}));}else await open({design:uploaded.id});});}
async function execute(fn){if(model.pending)return;model.pending=true;model.message='';model.result={};try{await fn();model.result_id++;}catch(e){model.message=e.message;model.result={error_code:e.code||''};model.result_id++;}finally{model.pending=false;await refresh();}}
async function dispatch({action,params:p}){
 if(action==='cutter' && p.action==='stop'){model.message='Stop requested — waiting for the cutter';try{await api(`/api/v1/cutters/${encodeURIComponent(p.id)}/stop`,'POST',{});await refresh();}catch(e){model.message=`Stop is not confirmed: ${e.message}`;}return;}
 if(action==='select-cutter'){if(model.selectedCutter!==p.cutter){model.cut_preview={};}model.selectedCutter=p.cutter;return;}
 if(action==='preference'){Object.assign(model.preferences,p);localStorage.setItem('cutcraft.desktop',JSON.stringify(model.preferences));return;}
 if(action==='reset-folds'){model.preferences.folds={};localStorage.setItem('cutcraft.desktop',JSON.stringify(model.preferences));return;}
 if(action==='layout'){localStorage.setItem('cutcraft.layout',p.layout);location.assign('/station');return;}
 if(action==='fold'){model.preferences.folds||={};model.preferences.folds[p.group]=p.open;localStorage.setItem('cutcraft.desktop',JSON.stringify(model.preferences));return;}
 if(action==='upload'){uploadMode=p;document.querySelector('#file').click();return;}
 if(action==='link'){window.open(p.url,'_blank','noopener,noreferrer');return;}
 if(action==='settings'){window.open(`/station#/${p.section==='keys'?'settings':p.section}`,'_blank','noopener');return;}
 if(action==='highlight'){model.message='Show me: inspecting the affected artwork.';return;}
 await execute(async()=>{
  switch(action){
   case 'command':{const r=await command(p.id,p.params,{incarnation:p.incarnation,revision:p.revision});model.result={value:r.value,close_dialog:['materials.save','mats.save','materials.save_current'].includes(p.id)};break;}
   case 'save':{const {close_after,...save}=p;await api('/api/v1/design/save','POST',savedBody(save));if(close_after){await refresh();await command('design.close');}model.result={close_dialog:true};model.message='Saved to the library.';break;}
   case 'open':await open(p);break;
   case 'library':model.library=await api('/api/v1/library?path='+encodeURIComponent(p.path||''));break;
   case 'uploads':model.uploads=await api('/api/v1/design/designs');break;
   case 'login':await api('/api/v1/login','POST',{name:p.username,password:p.password});break;
   case 'password':await api('/api/v1/me/password','POST',p);break;
   case 'logout':await api('/api/v1/logout','POST',{});model.snapshot={protocol:PROTOCOL,incarnation:'',revision:0,materials:[]};model.picture='';model.picture_id='';model.geometry={id:''};model.cut_preview={};break;
   case 'download':{const r=await fetch(`/api/v1/design/download?kind=${encodeURIComponent(p.kind)}`,{headers:{'x-cutcraft-protocol':String(PROTOCOL)}});if(!r.ok){const e=await r.json();throw new Error(e.message);}const url=URL.createObjectURL(await r.blob());const a=document.createElement('a');a.href=url;a.download=p.kind==='project'?'design.cutcraft':`cutcraft.${p.kind==='preview-svg'?'svg':p.kind==='preview-png'?'png':p.kind==='plan'?'json':'gpgl'}`;a.click();setTimeout(()=>URL.revokeObjectURL(url),1000);break;}
   case 'prepare-cut':{model.selectedCutter=p.cutter;const current=model.state.cutters?.find(c=>c.id===p.cutter)?.view?.turn;const prior=model.cut_preview?.copy;let copy;if(prior && prior.project_revision===model.snapshot.design?.project_revision && current?.job.job_id===prior.id){copy=prior;}else{const r=await command('cut.copy',p.test?{test:true}:{});copy=r.value.copy;}if(current?.job.job_id!==copy.id){await api(`/api/v1/cutters/${encodeURIComponent(p.cutter)}/queue`,'POST',{job_id:copy.id,revision:copy.revision,key:key()});}model.cut_preview={copy};model.result={copy};break;}
   case 'start':{const body={...p.start,key:key()};const path=`/api/v1/cutters/${encodeURIComponent(p.cutter)}`;try{await api(`${path}/start`,'POST',body);}catch(e){if(e instanceof TypeError){const result=await api(`${path}/starts/${body.key}`);if(result.answer!=='started')throw new Error('Start is not confirmed. Check the cutter before trying again.');}else throw e;}break;}
   case 'cutter':await api(`/api/v1/cutters/${encodeURIComponent(p.id)}/${p.action}`,'POST',p.body||{});break;
  }
 });
}
let dispatchQueue=Promise.resolve();
window.CutCraft={state:()=>JSON.stringify(model),dispatch:s=>{const intent=JSON.parse(s);if(intent.action==='cutter'&&intent.params.action==='stop'){void dispatch(intent);}else{dispatchQueue=dispatchQueue.then(()=>dispatch(intent));}},inspect:s=>{controls=JSON.parse(s);},controls:()=>controls};
document.querySelector('#file').addEventListener('change',e=>{void upload(e.target.files[0]);e.target.value='';});
window.addEventListener('dragover',e=>e.preventDefault());window.addEventListener('drop',e=>{e.preventDefault();uploadMode={};void upload(e.dataTransfer.files[0]);});
try{await refresh();await init();await start(document.querySelector('#cutcraft'));document.querySelector('#loading').hidden=true;setInterval(refresh,1000);}catch(e){document.querySelector('#loading').textContent=`CutCraft could not start: ${e.message}. Reload to try again.`;}
