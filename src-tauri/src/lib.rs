use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::Utc;
use reqwest::{multipart, Client};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{path::{Path, PathBuf}, time::{Duration, Instant}};
use tokio::{fs, time::sleep};
use uuid::Uuid;

const SERVICE: &str = "krill-image-studio";
const ACCOUNT: &str = "image-api-key";

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Settings {
    base_url: String,
    selected_model: String,
    #[serde(default)] available_models: Vec<String>,
    #[serde(default = "default_response_format")] response_format: String,
    output_dir: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self { base_url: "https://api.forkc2p.com/v1".into(), selected_model: String::new(), available_models: vec![], response_format: default_response_format(), output_dir: default_output() }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct HistoryItem {
    id: String, mode: String, filename: String, path: String, prompt: String,
    size: String, quality: String, model: String, elapsed_seconds: f64, created_at: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateInput { prompt: String, size: String, quality: String, filename: String, model: String }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditInput {
    prompt: String, size: String, quality: String, filename: String, model: String,
    image_data: String, image_name: String, mask_data: Option<String>, mask_name: Option<String>,
}

fn default_response_format() -> String { "url".into() }
fn app_dir() -> Result<PathBuf, String> { dirs::data_local_dir().map(|p|p.join("Krill Image Studio")).ok_or("无法确定应用数据目录".into()) }
fn settings_path() -> Result<PathBuf, String> { Ok(app_dir()?.join("settings.json")) }
fn history_path() -> Result<PathBuf, String> { Ok(app_dir()?.join("history.json")) }
fn default_output() -> String { dirs::picture_dir().unwrap_or_else(||app_dir().unwrap()).join("Krill Image Studio").to_string_lossy().to_string() }

fn normalize_base_url(raw:&str)->Result<String,String>{
    let mut value=raw.trim().trim_matches('`').trim_end_matches('/').to_string();
    for suffix in ["/images/generations","/images/edits","/models"] {
        if value.to_ascii_lowercase().ends_with(suffix){value.truncate(value.len()-suffix.len());value=value.trim_end_matches('/').to_string();break}
    }
    if !value.starts_with("http://")&&!value.starts_with("https://"){return Err("API 地址必须以 http:// 或 https:// 开头".into())}
    let rest=value.split_once("://").map(|(_,x)|x).unwrap_or("");if !rest.contains('/'){value.push_str("/v1")};Ok(value)
}

async fn read_settings()->Settings{
    let mut value=fs::read_to_string(settings_path().unwrap()).await.ok().and_then(|s|serde_json::from_str::<Settings>(&s).ok()).unwrap_or_default();
    if value.output_dir.trim().is_empty(){value.output_dir=default_output()};if !matches!(value.response_format.as_str(),"url"|"b64_json"){value.response_format=default_response_format()};value
}
async fn write_settings(value:&Settings)->Result<(),String>{fs::create_dir_all(app_dir()?).await.map_err(|e|e.to_string())?;fs::write(settings_path()?,serde_json::to_vec_pretty(value).map_err(|e|e.to_string())?).await.map_err(|e|e.to_string())}
fn read_key()->Option<String>{keyring::Entry::new(SERVICE,ACCOUNT).ok()?.get_password().ok()}
fn client()->Result<Client,String>{Client::builder().connect_timeout(Duration::from_secs(20)).timeout(Duration::from_secs(600)).build().map_err(|e|e.to_string())}

#[tauri::command]
async fn get_status()->Value{let s=read_settings().await;json!({"connected":read_key().is_some(),"baseUrl":s.base_url,"selectedModel":s.selected_model,"availableModels":s.available_models,"responseFormat":s.response_format,"outputDir":s.output_dir})}

#[tauri::command]
async fn save_settings(api_key:String,base_url:String,selected_model:String,response_format:String,output_dir:String)->Result<Value,String>{
    if !api_key.trim().is_empty(){keyring::Entry::new(SERVICE,ACCOUNT).map_err(|e|e.to_string())?.set_password(api_key.trim()).map_err(|e|e.to_string())?}
    let mut s=read_settings().await;let normalized=normalize_base_url(&base_url)?;if normalized!=s.base_url{s.available_models.clear()};s.base_url=normalized;s.selected_model=selected_model.trim().into();s.response_format=if response_format=="b64_json"{"b64_json".into()}else{"url".into()};s.output_dir=if output_dir.trim().is_empty(){default_output()}else{output_dir.trim().into()};write_settings(&s).await?;Ok(json!({"ok":true,"baseUrl":s.base_url}))
}

fn image_rank(s:&str)->u8{let x=s.to_ascii_lowercase();if ["image","dall-e","flux","seedream","ideogram","banana","recraft","imagen","stable-diffusion"].iter().any(|k|x.contains(k)){0}else{1}}
fn model_ids(value:&Value)->Vec<String>{
    let entries=value.get("data").and_then(Value::as_array).cloned().or_else(||value.get("models").and_then(Value::as_array).cloned()).or_else(||value.as_array().cloned()).unwrap_or_default();
    let mut out:Vec<String>=entries.iter().filter_map(|x|x.as_str().map(str::to_string).or_else(||x.get("id").and_then(Value::as_str).map(str::to_string)).or_else(||x.get("name").and_then(Value::as_str).map(str::to_string))).filter(|x|!x.trim().is_empty()).collect();
    out.sort_by(|a,b|image_rank(a).cmp(&image_rank(b)).then_with(||a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase())));out.dedup();out
}
async fn response_json(response:reqwest::Response,label:&str)->Result<Value,String>{
    let status=response.status();let text=response.text().await.map_err(|e|format!("{}响应读取失败：{}",label,e))?;let value:Value=serde_json::from_str(&text).unwrap_or_else(|_|json!({"raw":text}));
    if !status.is_success(){let detail=value.pointer("/error/message").and_then(Value::as_str).or_else(||value.get("message").and_then(Value::as_str)).or_else(||value.get("raw").and_then(Value::as_str)).unwrap_or("服务端未返回错误详情");return Err(format!("{}返回 {}：{}",label,status,detail))}Ok(value)
}

#[tauri::command]
async fn fetch_models(base_url:String,api_key:String)->Result<Value,String>{
    let base=normalize_base_url(&base_url)?;let key=if api_key.trim().is_empty(){read_key().ok_or("请填写 API Key")?}else{api_key.trim().into()};let started=Instant::now();
    let value=response_json(client()?.get(format!("{}/models",base)).bearer_auth(&key).send().await.map_err(|e|format!("获取模型失败：{}",e))?,"模型接口").await?;let models=model_ids(&value);if models.is_empty(){return Err("接口已连接，但 /models 没有返回可识别的模型".into())}
    if !api_key.trim().is_empty(){keyring::Entry::new(SERVICE,ACCOUNT).map_err(|e|e.to_string())?.set_password(&key).map_err(|e|e.to_string())?}
    let mut s=read_settings().await;s.base_url=base;s.available_models=models.clone();if s.selected_model.is_empty()||!models.contains(&s.selected_model){s.selected_model=models[0].clone()};write_settings(&s).await?;
    Ok(json!({"models":models,"selectedModel":s.selected_model,"baseUrl":s.base_url,"elapsedMs":started.elapsed().as_millis()}))
}

#[tauri::command]
async fn save_selected_model(model:String)->Result<(),String>{let mut s=read_settings().await;s.selected_model=model.trim().into();write_settings(&s).await}

fn clean_data_url(value:&str)->Result<(Vec<u8>,String),String>{let(meta,data)=value.split_once(',').ok_or("图片数据格式无效")?;let mime=meta.strip_prefix("data:").and_then(|s|s.split(';').next()).unwrap_or("image/png").into();let bytes=STANDARD.decode(data).map_err(|_|"图片 Base64 无效")?;if bytes.len()>25*1024*1024{return Err("单张图片不能超过 25MB".into())}Ok((bytes,mime))}

async fn download_url(c:&Client,url:&str,key:&str)->Result<Vec<u8>,String>{
    let first=c.get(url).send().await.map_err(|e|format!("下载图片失败：{}",e))?;let response=if matches!(first.status().as_u16(),401|403){c.get(url).bearer_auth(key).send().await.map_err(|e|format!("下载图片失败：{}",e))?}else{first};if !response.status().is_success(){return Err(format!("图片链接下载返回 {}",response.status()))}response.bytes().await.map(|b|b.to_vec()).map_err(|e|format!("读取图片失败：{}",e))
}
async fn resolve_image(c:&Client,mut payload:Value,base:&str,key:&str)->Result<Vec<u8>,String>{
    let id=payload.get("id").and_then(Value::as_str).map(str::to_string);let deadline=tokio::time::Instant::now()+Duration::from_secs(900);
    loop{
        if let Some(data)=payload.pointer("/data/0/b64_json").and_then(Value::as_str){return STANDARD.decode(data).map_err(|e|e.to_string())}
        if let Some(url)=payload.pointer("/data/0/url").and_then(Value::as_str).or_else(||payload.get("url").and_then(Value::as_str)){return download_url(c,url,key).await}
        let status=payload.get("status").and_then(Value::as_str).unwrap_or("");if matches!(status,"failed"|"cancelled"|"canceled"|"expired"){let detail=payload.pointer("/error/message").and_then(Value::as_str).or_else(||payload.get("message").and_then(Value::as_str)).unwrap_or("服务端未提供详情");return Err(format!("图片任务失败：{}",detail))}
        let task=id.as_deref().ok_or("接口没有返回图片 URL、Base64 或任务 ID")?;if tokio::time::Instant::now()>=deadline{return Err(format!("等待图片任务 {} 超过15分钟",task))};sleep(Duration::from_secs(4)).await;payload=response_json(c.get(format!("{}/images/{}",base,task)).bearer_auth(key).send().await.map_err(|e|format!("查询图片任务失败：{}",e))?,"任务状态接口").await?
    }
}

fn safe_name(name:&str)->String{let stem=Path::new(name).file_stem().and_then(|s|s.to_str()).unwrap_or("created-image");let clean:String=stem.chars().map(|c|if "<>:\"/\\|?*".contains(c)||c.is_control(){'-'}else{c}).collect();format!("{}.png",clean.trim_matches(&[' ','.','-'][..]))}
async fn store(bytes:Vec<u8>,mode:&str,prompt:String,size:String,quality:String,model:String,filename:String,elapsed:f64)->Result<HistoryItem,String>{
    let s=read_settings().await;let dir=PathBuf::from(s.output_dir);fs::create_dir_all(&dir).await.map_err(|e|e.to_string())?;let mut name=safe_name(&filename);if dir.join(&name).exists(){name=format!("{}-{}.png",Path::new(&name).file_stem().unwrap().to_string_lossy(),Utc::now().format("%Y%m%d-%H%M%S"))};let path=dir.join(&name);fs::write(&path,bytes).await.map_err(|e|format!("保存图片失败：{}",e))?;
    let item=HistoryItem{id:Uuid::new_v4().to_string(),mode:mode.into(),filename:name,path:path.to_string_lossy().into(),prompt,size,quality,model,elapsed_seconds:elapsed,created_at:Utc::now().to_rfc3339()};let mut h=get_history().await;h.insert(0,item.clone());h.truncate(100);fs::create_dir_all(app_dir()?).await.map_err(|e|e.to_string())?;fs::write(history_path()?,serde_json::to_vec_pretty(&h).unwrap()).await.map_err(|e|e.to_string())?;Ok(item)
}

#[tauri::command]
async fn generate_image(input:GenerateInput)->Result<HistoryItem,String>{
    if input.prompt.trim().is_empty(){return Err("请填写图片描述".into())}if input.model.trim().is_empty(){return Err("请选择图片模型".into())}let s=read_settings().await;let key=read_key().ok_or("请先填写 API Key 并获取模型")?;let c=client()?;let started=Instant::now();
    let body=json!({"model":input.model,"prompt":input.prompt,"size":input.size,"quality":input.quality,"response_format":s.response_format});let value=response_json(c.post(format!("{}/images/generations",s.base_url)).bearer_auth(&key).json(&body).send().await.map_err(|e|format!("提交文生图失败：{}",e))?,"文生图接口").await?;let bytes=resolve_image(&c,value,&s.base_url,&key).await?;let elapsed=started.elapsed().as_secs_f64();store(bytes,"generate",input.prompt,input.size,input.quality,input.model,input.filename,elapsed).await
}

#[tauri::command]
async fn edit_image(input:EditInput)->Result<HistoryItem,String>{
    if input.prompt.trim().is_empty(){return Err("请填写修改要求".into())}if input.model.trim().is_empty(){return Err("请选择图片模型".into())}let s=read_settings().await;let key=read_key().ok_or("请先填写 API Key 并获取模型")?;let c=client()?;let started=Instant::now();let(image,mime)=clean_data_url(&input.image_data)?;
    let mut form=multipart::Form::new().text("model",input.model.clone()).text("prompt",input.prompt.clone()).text("size",input.size.clone()).text("quality",input.quality.clone()).text("response_format",s.response_format.clone()).part("image",multipart::Part::bytes(image).file_name(input.image_name).mime_str(&mime).map_err(|e|e.to_string())?);if let Some(mask_data)=input.mask_data.filter(|x|!x.is_empty()){let(mask,mask_mime)=clean_data_url(&mask_data)?;form=form.part("mask",multipart::Part::bytes(mask).file_name(input.mask_name.unwrap_or("mask.png".into())).mime_str(&mask_mime).map_err(|e|e.to_string())?)}
    let value=response_json(c.post(format!("{}/images/edits",s.base_url)).bearer_auth(&key).multipart(form).send().await.map_err(|e|format!("提交图生图失败：{}",e))?,"图生图接口").await?;let bytes=resolve_image(&c,value,&s.base_url,&key).await?;let elapsed=started.elapsed().as_secs_f64();store(bytes,"edit",input.prompt,input.size,input.quality,input.model,input.filename,elapsed).await
}

#[tauri::command]
async fn get_history()->Vec<HistoryItem>{fs::read_to_string(history_path().unwrap()).await.ok().and_then(|s|serde_json::from_str(&s).ok()).unwrap_or_default()}
#[tauri::command]
async fn read_image(path:String)->Result<String,String>{let bytes=fs::read(&path).await.map_err(|e|format!("读取图片失败：{}",e))?;let mime=match Path::new(&path).extension().and_then(|s|s.to_str()).unwrap_or("").to_ascii_lowercase().as_str(){"jpg"|"jpeg"=>"image/jpeg","webp"=>"image/webp",_=>"image/png"};Ok(format!("data:{};base64,{}",mime,STANDARD.encode(bytes)))}
#[tauri::command]
async fn delete_history_item(id:String)->Result<(),String>{
    let mut history=get_history().await;
    let item=history.iter().find(|item|item.id==id).cloned().ok_or("没有找到这条历史记录")?;
    history.retain(|entry|entry.id!=id);
    fs::create_dir_all(app_dir()?).await.map_err(|e|e.to_string())?;
    fs::write(history_path()?,serde_json::to_vec_pretty(&history).map_err(|e|e.to_string())?).await.map_err(|e|e.to_string())?;
    if Path::new(&item.path).exists(){fs::remove_file(&item.path).await.map_err(|e|format!("历史记录已删除，但图片文件删除失败：{}",e))?}
    Ok(())
}

pub fn run(){tauri::Builder::default().invoke_handler(tauri::generate_handler![get_status,save_settings,fetch_models,save_selected_model,generate_image,edit_image,get_history,read_image,delete_history_item]).run(tauri::generate_context!()).expect("启动 Krill Image Studio 失败")}
