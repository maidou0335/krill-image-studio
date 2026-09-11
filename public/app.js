const $=s=>document.querySelector(s),$$=s=>[...document.querySelectorAll(s)];
const isTauri=Boolean(window.__TAURI__?.core?.invoke);
const STORE_KEY="krill-image-studio-settings-v1",HISTORY_KEY="krill-image-studio-history-v1";
const defaults={baseUrl:"",apiKey:"",selectedModel:"",availableModels:[],responseFormat:"url",outputDir:"浏览器下载目录"};
const loadSettings=()=>({...defaults,...JSON.parse(localStorage.getItem(STORE_KEY)||"{}")});
const saveBrowserSettings=s=>localStorage.setItem(STORE_KEY,JSON.stringify(s));
const normalizeBase=value=>{let url=String(value||"").trim().replace(/\/$/,"");url=url.replace(/\/images\/(generations|edits)$/i,"");if(!url)throw new Error("请填写 API 地址");return url};
const authHeaders=key=>({Authorization:`Bearer ${key}`});
const apiFetch=async(url,options={})=>{let response;try{response=await fetch(url,options)}catch(e){throw new Error("浏览器无法连接接口。该供应商可能未允许跨域访问（CORS），请换用支持网页调用的接口，或继续使用桌面版。")}const text=await response.text();let value;try{value=text?JSON.parse(text):{}}catch{value={message:text}}if(!response.ok)throw new Error(value?.error?.message||value?.message||`接口返回 ${response.status}`);return value};
const dataUrlFromBlob=blob=>new Promise((resolve,reject)=>{const reader=new FileReader();reader.onload=()=>resolve(reader.result);reader.onerror=reject;reader.readAsDataURL(blob)});
const db=()=>new Promise((resolve,reject)=>{const request=indexedDB.open("krill-image-studio",1);request.onupgradeneeded=()=>request.result.createObjectStore("images");request.onsuccess=()=>resolve(request.result);request.onerror=()=>reject(request.error)});
async function putImage(id,data){const database=await db();await new Promise((resolve,reject)=>{const tx=database.transaction("images","readwrite");tx.objectStore("images").put(data,id);tx.oncomplete=resolve;tx.onerror=()=>reject(tx.error)});database.close()}
async function getImage(id){const database=await db();const result=await new Promise((resolve,reject)=>{const tx=database.transaction("images","readonly");const request=tx.objectStore("images").get(id);request.onsuccess=()=>resolve(request.result);request.onerror=()=>reject(request.error)});database.close();if(!result)throw new Error("本地图片记录已不存在");return result}
const b64ToDataUrl=(b64,mime="image/png")=>`data:${mime};base64,${b64}`;
async function resolveImage(value,base,key){
  for(let i=0;i<225;i++){
    const b64=value?.data?.[0]?.b64_json||value?.b64_json;if(b64)return b64ToDataUrl(b64);
    const url=value?.data?.[0]?.url||value?.url;if(url){const response=await fetch(url);if(!response.ok)throw new Error(`图片链接下载返回 ${response.status}`);return dataUrlFromBlob(await response.blob())}
    const id=value?.id||value?.task_id||value?.data?.id;if(!id)throw new Error("接口没有返回图片 URL、Base64 或任务 ID");
    await new Promise(resolve=>setTimeout(resolve,4000));value=await apiFetch(`${base}/images/${id}`,{headers:authHeaders(key)});
  }
  throw new Error("等待图片任务超过15分钟");
}
async function browserInvoke(cmd,args={}){
  const s=loadSettings();
  if(cmd==="get_status")return {...s,connected:Boolean(s.apiKey),apiKey:undefined};
  if(cmd==="save_settings"){const next={...s,baseUrl:normalizeBase(args.baseUrl),selectedModel:args.selectedModel||"",responseFormat:args.responseFormat||"url",outputDir:args.outputDir||"浏览器下载目录"};if(args.apiKey?.trim())next.apiKey=args.apiKey.trim();saveBrowserSettings(next);return {ok:true}}
  if(cmd==="fetch_models"){
    const started=performance.now(),baseUrl=normalizeBase(args.baseUrl),apiKey=args.apiKey?.trim()||s.apiKey;if(!apiKey)throw new Error("请填写 API Key");
    const value=await apiFetch(`${baseUrl}/models`,{headers:authHeaders(apiKey)});const raw=Array.isArray(value)?value:(value.data||value.models||[]);const models=[...new Set(raw.map(x=>typeof x==="string"?x:x?.id).filter(Boolean))].sort((a,b)=>{const image=x=>/(image|dall|flux|seedream|imagen|recraft)/i.test(x)?0:1;return image(a)-image(b)||a.localeCompare(b)});if(!models.length)throw new Error("/models 没有返回可识别的模型");const selectedModel=models.includes(s.selectedModel)?s.selectedModel:models[0];saveBrowserSettings({...s,baseUrl,apiKey,availableModels:models,selectedModel});return {models,selectedModel,baseUrl,elapsedMs:performance.now()-started};
  }
  if(cmd==="save_selected_model"){saveBrowserSettings({...s,selectedModel:args.model});return}
  if(cmd==="get_history")return JSON.parse(localStorage.getItem(HISTORY_KEY)||"[]");
  if(cmd==="read_image")return getImage(args.path);
  if(cmd==="generate_image"||cmd==="edit_image"){
    const input=args.input,key=s.apiKey;if(!key)throw new Error("请先填写 API Key 并获取模型");const started=performance.now();let endpoint,options;
    if(cmd==="generate_image"){endpoint=`${s.baseUrl}/images/generations`;options={method:"POST",headers:{...authHeaders(key),"Content-Type":"application/json"},body:JSON.stringify({model:input.model,prompt:input.prompt,size:input.size,quality:input.quality,response_format:s.responseFormat})}}
    else{endpoint=`${s.baseUrl}/images/edits`;const form=new FormData();form.append("model",input.model);form.append("prompt",input.prompt);form.append("size",input.size);form.append("quality",input.quality);form.append("response_format",s.responseFormat);const toBlob=async data=>(await fetch(data)).blob();form.append("image",await toBlob(input.imageData),input.imageName||"image.png");if(input.maskData)form.append("mask",await toBlob(input.maskData),input.maskName||"mask.png");options={method:"POST",headers:authHeaders(key),body:form}}
    const value=await apiFetch(endpoint,options),data=await resolveImage(value,s.baseUrl,key),id=`img-${Date.now()}-${Math.random().toString(36).slice(2)}`;await putImage(id,data);const item={id,path:id,mode:cmd==="edit_image"?"edit":"generate",prompt:input.prompt,size:input.size,quality:input.quality,model:input.model,filename:input.filename,elapsedSeconds:(performance.now()-started)/1000,createdAt:new Date().toISOString()};const history=[item,...JSON.parse(localStorage.getItem(HISTORY_KEY)||"[]")].slice(0,100);localStorage.setItem(HISTORY_KEY,JSON.stringify(history));return item;
  }
  throw new Error(`浏览器版暂不支持 ${cmd}`);
}
const invoke=(cmd,args={})=>isTauri?window.__TAURI__.core.invoke(cmd,args):browserInvoke(cmd,args);
const state={mode:"generate",source:null,mask:null,current:null,history:[],models:[],settings:null};

function escapeHtml(value=""){return String(value).replace(/[&<>'"]/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;","'":"&#39;",'"':"&quot;"}[c]))}
function error(message=""){const box=$("#error");box.hidden=!message;box.textContent=message}
function setMode(mode){state.mode=mode;$$('.mode-tab').forEach(x=>x.classList.toggle('active',x.dataset.mode===mode));$("#editInputs").hidden=mode!=="edit";$("#generate span").textContent=mode==="edit"?"生成修改图片":"生成图片";error()}
function fileData(file){return new Promise((resolve,reject)=>{if(file.size>25*1024*1024)return reject(new Error("单张图片不能超过 25MB"));const reader=new FileReader();reader.onload=()=>resolve({name:file.name,data:reader.result});reader.onerror=()=>reject(new Error("读取图片失败"));reader.readAsDataURL(file)})}
function optionHtml(models,selected){return models.length?models.map(m=>`<option value="${escapeHtml(m)}" ${m===selected?'selected':''}>${escapeHtml(m)}</option>`).join(''):'<option value="">请先获取可用模型</option>'}
function renderModels(models,selected){state.models=models||[];$("#modelSelect").innerHTML=optionHtml(state.models,selected);$("#settingsModel").innerHTML=optionHtml(state.models,selected)}
async function refreshStatus(){
  const s=await invoke("get_status");state.settings=s;renderModels(s.availableModels||[],s.selectedModel||"");$("#baseUrl").value=s.baseUrl||"";$("#outputDir").value=s.outputDir||"";$("#responseFormat").value=s.responseFormat||"url";
  const c=$("#connection");c.classList.toggle("ok",s.connected);c.querySelector('b').textContent=s.connected?(s.selectedModel?`已连接 · ${s.selectedModel}`:"已连接 · 请获取模型"):"尚未配置";$("#generateHint").textContent=(s.responseFormat||"url")==="url"?"URL 返回 · 速度优先":"Base64 返回 · 兼容优先";
}
async function fetchModels(){
  const button=$("#fetchModels"),status=$("#fetchStatus");button.disabled=true;button.textContent="正在获取…";status.textContent="正在连接 /models";
  try{const result=await invoke("fetch_models",{baseUrl:$("#baseUrl").value,apiKey:$("#apiKey").value});renderModels(result.models,result.selectedModel);$("#baseUrl").value=result.baseUrl;status.textContent=`已获取 ${result.models.length} 个模型，用时 ${(result.elapsedMs/1000).toFixed(2)} 秒`;await refreshStatus()}catch(e){status.textContent=String(e)}finally{button.disabled=false;button.textContent="获取并保存可用模型"}
}
async function loadHistory(){state.history=await invoke("get_history");renderHistory()}
async function imageFor(item){return invoke("read_image",{path:item.path})}
function showImage(data,item){state.current=item;$("#resultImage").src=data;$("#canvas").classList.add("has-image");$("#canvasActions").hidden=false;$("#download").href=data;$("#download").download=item.filename;$("#resultMeta").textContent=`${item.model} · ${item.size} · ${item.quality} · ${Number(item.elapsedSeconds||0).toFixed(1)} 秒`}
function renderHistory(){
  const filter=$("#historyFilter").value;const items=filter==="all"?state.history:state.history.filter(x=>x.mode===filter);$("#history").innerHTML=items.length?items.map(x=>`<article class="history-item" data-id="${x.id}"><img data-path="${escapeHtml(x.path)}" alt=""><div class="history-info"><strong>${escapeHtml(x.prompt)}</strong><div><span>${x.mode==="edit"?"图生图":"文生图"} · ${escapeHtml(x.model||"")}</span><span>${Number(x.elapsedSeconds||0).toFixed(0)}秒</span></div></div></article>`).join(''):'<div class="history-empty">暂无生成记录</div>';
  $$('.history-item img').forEach(async img=>{try{img.src=await invoke("read_image",{path:img.dataset.path})}catch{}});$$('.history-item').forEach(card=>card.onclick=async()=>{const item=state.history.find(x=>x.id===card.dataset.id);showImage(await imageFor(item),item)})
}

$$('.mode-tab').forEach(x=>x.onclick=()=>setMode(x.dataset.mode));
$("#sourceInput").onchange=async e=>{const file=e.target.files[0];if(!file)return;try{state.source=await fileData(file);$("#sourceName").textContent=file.name}catch(err){error(String(err))}};
$("#maskInput").onchange=async e=>{const file=e.target.files[0];if(!file)return;try{state.mask=await fileData(file);$("#maskName").textContent=file.name}catch(err){error(String(err))}};
$("#modelSelect").onchange=async e=>{await invoke("save_selected_model",{model:e.target.value});$("#settingsModel").value=e.target.value;await refreshStatus()};
$("#settingsModel").onchange=e=>$("#modelSelect").value=e.target.value;
$("#refreshModels").onclick=()=>{$("#settingsDialog").showModal();fetchModels()};
$("#fetchModels").onclick=fetchModels;
$("#historyFilter").onchange=renderHistory;
$("#generate").onclick=async()=>{
  const prompt=$("#prompt").value.trim(),model=$("#modelSelect").value;if(!prompt)return error("请填写图片描述");if(!model)return error("请先在设置中获取并选择模型");if(state.mode==="edit"&&!state.source)return error("图生图需要先上传原图");
  const button=$("#generate");button.disabled=true;button.querySelector('span').textContent="正在生成…";button.querySelector('small').textContent="模型处理中，请保持软件开启";error();const input={prompt,model,size:$("#size").value,quality:$("#quality").value,filename:$("#filename").value.trim()||"created-image.png"};if(state.mode==="edit")Object.assign(input,{imageData:state.source.data,imageName:state.source.name,maskData:state.mask?.data||null,maskName:state.mask?.name||null});
  try{const item=await invoke(state.mode==="edit"?"edit_image":"generate_image",{input});showImage(await imageFor(item),item);await loadHistory()}catch(e){error(String(e))}finally{button.disabled=false;button.querySelector('span').textContent=state.mode==="edit"?"生成修改图片":"生成图片";button.querySelector('small').textContent=(state.settings?.responseFormat||"url")==="url"?"URL 返回 · 速度优先":"Base64 返回 · 兼容优先"}
};
$("#reuse").onclick=async()=>{if(!state.current)return;state.source={name:state.current.filename,data:await imageFor(state.current)};$("#sourceName").textContent=state.current.filename;setMode("edit")};
$("#resultImage").onclick=()=>{if(!state.current)return;$("#previewImage").src=$("#resultImage").src;$("#previewCaption").textContent=`${state.current.filename} · ${state.current.model} · ${Number(state.current.elapsedSeconds||0).toFixed(1)} 秒`;$("#previewDialog").showModal()};
$("#openSettings").onclick=()=>{$("#apiKey").value="";$("#fetchStatus").textContent="图片模型会排在列表前面";$("#settingsDialog").showModal()};
$("#closeSettings").onclick=$("#cancelSettings").onclick=()=>$("#settingsDialog").close();
$("#closePreview").onclick=()=>$("#previewDialog").close();
$("#settingsForm").onsubmit=async e=>{e.preventDefault();const button=$("#saveSettings");button.disabled=true;button.textContent="正在保存…";try{await invoke("save_settings",{apiKey:$("#apiKey").value,baseUrl:$("#baseUrl").value,selectedModel:$("#settingsModel").value,responseFormat:$("#responseFormat").value,outputDir:$("#outputDir").value});$("#apiKey").value="";$("#settingsDialog").close();await refreshStatus()}catch(err){$("#fetchStatus").textContent=String(err)}finally{button.disabled=false;button.textContent="保存设置"}};

if(!isTauri){
  $("#privacyNote").textContent="网页版的 Key、模型设置与图片历史仅保存在当前浏览器中；清除 Safari 网站数据会一并删除。";
  $("#outputDir").value="浏览器下载目录";$("#outputDir").disabled=true;
  if("serviceWorker" in navigator)window.addEventListener("load",()=>navigator.serviceWorker.register("./sw.js").catch(()=>{}));
}
Promise.all([refreshStatus(),loadHistory()]).catch(e=>error(String(e)));
