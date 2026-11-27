//! Optional CSP Companion sidecar. No external Python needed for native mode.
//! Sensei downloads and verifies uv and the pinned Companion files only when requested.
use anyhow::{anyhow, bail, Context, Result};
use rmcp::{model::CallToolRequestParam, service::{RunningService, RoleClient}, transport::TokioChildProcess, ServiceExt};
use serde_json::{json, Value};
use std::{path::{Path, PathBuf}, process::Stdio, time::Duration};
use tokio::process::Command;

const REV: &str = "f0d611e4a156e29931ce9255dd76627d8160f7ec";
const UV_URL: &str = "https://github.com/astral-sh/uv/releases/download/0.13.10/uv-x86_64-pc-windows-msvc.zip";
const UV_SHA256: &str = "297b3881fe1adfb922becc2c1613202abb41dc83ea5ff43d472aa087ca1f8683";
const CSP_FILES: &[(&str, &str)] = &[
    ("LICENSE","b5c2d4d9458f553d0782b180b61e91f90cb0b65e"),
    ("THIRD_PARTY_NOTICES.md","f49da7b1b925c5a8b0780e1c1a2f09965a9f9cae"),
    ("companion.py","9c868fd51caf90032c8c3782137560cae6a36620"),
    ("policy.json","29c997fad3dd316e8a26e49666336fe97201a966"),
    ("requirements.txt","3a285001840d8a95972852b3f4efa5f754e00bf6"),
    ("secure_config.py","74f5963cb51ff0d166446f59e2fa7257c71316b4"),
    ("server.py","37d68ca3121985e325b3f7325bed85ab4105b447"),
    ("setup_pairing.py","6ec15609fad3c33bf980934ac5f451bf6408a7bb"),
    ("check_connection.py","700de71e8c05dcf1468173d29f0134c849198aee"),
];
fn source(root:&Path)->PathBuf { root.join("companion") }
fn python(root:&Path)->PathBuf {
    root.join("companion-env").join(if cfg!(windows) {"Scripts/python.exe"}else{"bin/python"})
}
pub fn installed(root:&Path)->bool { python(root).is_file() && source(root).join("server.py").is_file() && root.join("companion-installed").is_file() }

fn scrub(cmd:&mut Command) {
    for key in ["SENSEI_API_KEY","OPENAI_API_KEY","ANTHROPIC_API_KEY","GEMINI_API_KEY","OPENROUTER_API_KEY","PYTHONPATH","PYTHONHOME"] {cmd.env_remove(key);}
}
async fn execute(cmd:&mut Command)->Result<()> {
    scrub(cmd);
    let result=cmd.status().await.context("Failed to start managed dependency command")?;
    if !result.success() {bail!("Dependency command failed: {}",result);}
    Ok(())
}
fn blob_hash(data:&[u8])->String {
    use sha1::{Digest,Sha1};
    let mut h=Sha1::new();
    h.update(format!("blob {}\0",data.len()).as_bytes());
    h.update(data);
    format!("{:x}",h.finalize())
}
fn verify_source(dir:&Path)->Result<()> {
    for &(name,hash) in CSP_FILES {
        let bytes=std::fs::read(dir.join(name)).with_context(||format!("Missing Companion file {name}"))?;
        if blob_hash(&bytes)!=hash {bail!("Companion source integrity mismatch: {name}");}
    }
    Ok(())
}
fn trusted_download(url:&url::Url)->bool {
    url.scheme()=="https" && matches!(url.host_str(),Some("github.com"|"raw.githubusercontent.com"|"release-assets.githubusercontent.com"|"objects.githubusercontent.com"))
}
async fn download(url:&str,limit:usize)->Result<Vec<u8>> {
    let target=url::Url::parse(url)?;
    if !trusted_download(&target){bail!("Untrusted download host");}
    let client=reqwest::Client::builder().timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::custom(|attempt|{
            if attempt.previous().len()>=5 || !trusted_download(attempt.url()){attempt.error("Untrusted dependency redirect")}else{attempt.follow()}
        })).build()?;
    let mut response=client.get(target).send().await?.error_for_status()?;
    if response.content_length().is_some_and(|n|n>limit as u64){bail!("Dependency exceeds size limit");}
    let mut bytes=Vec::new();
    while let Some(chunk)=response.chunk().await? {
        if bytes.len().saturating_add(chunk.len())>limit {bail!("Dependency exceeds size limit");}
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
async fn download_source(root:&Path)->Result<()> {
    let dir=source(root);
    if dir.exists(){return verify_source(&dir);}
    let stage=root.join("companion-download");
    std::fs::create_dir(&stage).context("Existing incomplete companion-download; inspect/remove it before retrying")?;
    for &(name,hash) in CSP_FILES {
        let url=format!("https://raw.githubusercontent.com/TMAK1710/csp-companion-mcp/{REV}/{name}");
        let bytes=download(&url,128*1024).await?;
        if blob_hash(&bytes)!=hash {bail!("Companion source integrity mismatch: {name}");}
        std::fs::write(stage.join(name),bytes)?;
    }
    verify_source(&stage)?;
    std::fs::rename(stage,dir)?;
    Ok(())
}
async fn managed_uv(root:&Path)->Result<PathBuf>{
    use sha2::{Digest,Sha256};
    use std::io::{Cursor,Read};
    if !cfg!(all(windows,target_arch="x86_64")){bail!("Managed uv installer supports Windows x64");}
    let dir=root.join("tools");std::fs::create_dir_all(&dir)?;
    let archive=dir.join("uv-0.13.10.zip");
    let data=if archive.exists(){std::fs::read(&archive)?}else{download(UV_URL,32*1024*1024).await?};
    if format!("{:x}",Sha256::digest(&data))!=UV_SHA256{bail!("uv archive SHA-256 mismatch");}
    if !archive.exists(){std::fs::write(&archive,&data)?;}
    let mut zip=zip::ZipArchive::new(Cursor::new(data))?;
    let mut executable=None;
    for i in 0..zip.len() {
        let mut f=zip.by_index(i)?;
        if f.enclosed_name().and_then(|p|p.file_name().map(|n|n.to_owned())).as_deref()==Some(std::ffi::OsStr::new("uv.exe")) && f.is_file(){
            if executable.is_some()||f.size()>64*1024*1024{bail!("Unexpected uv archive");}
            let mut bytes=Vec::new();f.read_to_end(&mut bytes)?;executable=Some(bytes);
        }
    }
    let bytes=executable.ok_or_else(||anyhow!("uv.exe missing from verified archive"))?;
    let uv=dir.join("uv.exe");
    if uv.exists(){if std::fs::read(&uv)?!=bytes{bail!("Managed uv executable changed; inspect or remove tools/uv.exe");}}
    else{std::fs::write(&uv,bytes)?;}
    Ok(uv)
}
fn uv_command(uv:&Path,root:&Path)->Command{
    let mut cmd=Command::new(uv);
    // Isolate uv's Python installation/cache and do not read project-specific uv config.
    cmd.env("UV_PYTHON_INSTALL_DIR",root.join("python"))
        .env("UV_CACHE_DIR",root.join("uv-cache")).env("UV_NO_CONFIG","1");
    cmd
}
pub async fn install(root:&Path)->Result<()> {
    if installed(root) {verify_source(&source(root))?;println!("Companion is already installed.");return Ok(());}
    std::fs::create_dir_all(root)?;
    download_source(root).await?;
    let uv=managed_uv(root).await?;
    if !python(root).is_file() {
        execute(uv_command(&uv,root).args(["venv","--python","3.12","--managed-python"]).arg(root.join("companion-env"))).await?;
    }
    execute(uv_command(&uv,root).args(["pip","install","--python"]).arg(python(root)).arg("-r").arg(source(root).join("requirements.txt"))).await?;
    std::fs::create_dir_all(root.join("previews"))?;
    std::fs::write(root.join("companion-installed"),REV)?;
    println!("Companion installed. Run sensei pair.");
    Ok(())
}
pub async fn pair(root:&Path)->Result<()> {
    if !installed(root){bail!("Run sensei install-companion first");}
    verify_source(&source(root))?;
    if source(root).join("session.json").is_file(){println!("Pairing already exists; not replacing credentials.");return Ok(());}
    println!("CSP: ファイル → スマートフォンと接続。QRのURLはローカル端末にだけ入力してください。");
    execute(Command::new(python(root)).arg(source(root).join("setup_pairing.py")).current_dir(source(root))).await
}

pub struct Companion { client:RunningService<RoleClient,()> }
impl Companion {
    pub async fn start(root:&Path)->Result<Self> {
        if !installed(root){bail!("Companion is not installed");}
        verify_source(&source(root))?;
        let dir=source(root);
        let mut cmd=Command::new(python(root));
        cmd.arg("-u").arg(dir.join("server.py")).current_dir(&dir)
            .env("CSP_COMPANION_CONFIG",dir.join("session.json"))
            .env("CSP_COMPANION_OUTPUT",root.join("previews"))
            .env("PYTHONUTF8","1").env_remove("SENSEI_API_KEY").env_remove("OPENAI_API_KEY")
            .env_remove("ANTHROPIC_API_KEY").env_remove("GEMINI_API_KEY")
            .stderr(Stdio::inherit());
        scrub(&mut cmd);
        cmd.env("PYTHONNOUSERSITE","1");
        let client=tokio::time::timeout(Duration::from_secs(30),().serve(TokioChildProcess::new(cmd)?)).await??;
        Ok(Self{client})
    }
    pub async fn call(&self,name:&str,args:Value)->Result<Value> {
        // Fixed internal allowlist; model never controls a tool name or raw arguments.
        if !["csp_status","csp_connect","csp_disconnect","csp_brush_state","csp_navigate"].contains(&name) {
            bail!("Unsupported Companion tool");
        }
        let arguments=args.as_object().cloned().ok_or_else(||anyhow!("Invalid arguments"))?;
        let result=tokio::time::timeout(Duration::from_secs(20),self.client.call_tool(CallToolRequestParam{name:name.to_owned().into(),arguments:Some(arguments)})).await??;
        let value=serde_json::to_value(result)?;
        let body=value.get("structuredContent").filter(|v|v.is_object()).cloned()
            .or_else(||value.get("content")?.as_array()?.iter().find_map(|v|serde_json::from_str::<Value>(v.get("text")?.as_str()?).ok()))
            .ok_or_else(||anyhow!("Unexpected Companion response"))?;
        if value.get("isError").and_then(Value::as_bool)==Some(true) || body.get("ok").and_then(Value::as_bool)==Some(false) {
            bail!("Companion command failed; do not retry mutations automatically");
        }
        Ok(body)
    }
    pub async fn brush(&self)->Option<Value> {
        let b=self.call("csp_brush_state",json!({})).await.ok()?;
        let state=b.get("state")?.as_object()?;
        // Keep only bounded numeric or boolean brush data. Never forward arbitrary text.
        let out:serde_json::Map<String,Value>=state.iter()
            .filter(|(k,v)|k.len()<64 && k.chars().all(|c|c.is_ascii_alphanumeric()||c=='_') && (v.is_number()||v.is_boolean()))
            .take(30).map(|(k,v)|(k.clone(),v.clone())).collect();
        Some(json!({"source":"csp_companion","brush_fields":out}))
    }
    pub async fn navigate(&self,action:&str)->Result<()> {
        let a=match action {"in"=>"ZoomIn","out"=>"ZoomOut","fit"=>"Fitting","reset"=>"Rotate0",_=>bail!("Use :zoom in/out/fit/reset")};
        self.call("csp_navigate",json!({"action":a})).await?;
        Ok(())
    }
    pub async fn close(self) {let _=tokio::time::timeout(Duration::from_secs(5),self.client.cancel()).await;}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]fn git_blob_hash_matches_known_empty_blob(){
        assert_eq!(blob_hash(b""),"e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
    }
    #[test]fn download_hosts_are_restricted(){
        assert!(trusted_download(&url::Url::parse(UV_URL).unwrap()));
        assert!(!trusted_download(&url::Url::parse("https://github.com.example.org/").unwrap()));
        assert!(!trusted_download(&url::Url::parse("http://github.com/").unwrap()));
    }
}
