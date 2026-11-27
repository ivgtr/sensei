//! Optional CSP Companion sidecar. No external Python needed for native mode.
//! The Python runtime and MCP are managed by uv only when requested.
use anyhow::{anyhow, bail, Context, Result};
use rmcp::{model::CallToolRequestParam, service::{RunningService, RoleClient}, transport::TokioChildProcess, ServiceExt};
use serde_json::{json, Value};
use std::{path::{Path, PathBuf}, process::Stdio, time::Duration};
use tokio::process::Command;

const REV: &str = "f0d611e4a156e29931ce9255dd76627d8160f7ec";
const URL: &str = "https://github.com/TMAK1710/csp-companion-mcp.git";
fn source(root:&Path)->PathBuf { root.join("companion") }
fn python(root:&Path)->PathBuf {
    root.join("companion-env").join(if cfg!(windows) {"Scripts/python.exe"}else{"bin/python"})
}
pub fn installed(root:&Path)->bool { python(root).is_file() && source(root).join("server.py").is_file() && root.join("companion-installed").is_file() }

async fn execute(cmd:&mut Command)->Result<()> {
    let result=cmd.status().await.context("Failed to start dependency command; install Git and uv for optional Companion")?;
    if !result.success() {bail!("Dependency command failed: {}",result);}
    Ok(())
}
async fn revision(dir:&Path)->Result<()> {
    let output=Command::new("git").args(["rev-parse","HEAD"]).current_dir(dir).output().await?;
    if !output.status.success() || String::from_utf8_lossy(&output.stdout).trim()!=REV {bail!("Companion checkout revision mismatch");}
    let output=Command::new("git").args(["status","--porcelain","--untracked-files=no"]).current_dir(dir).output().await?;
    if !output.status.success() || !output.stdout.is_empty() {bail!("Companion source has local changes; refusing to run it");}
    Ok(())
}
pub async fn install(root:&Path)->Result<()> {
    if installed(root) {revision(&source(root)).await?;println!("Companion is already installed.");return Ok(());}
    let dir=source(root);
    if !dir.exists() {
        let stage=root.join("companion-download");
        if stage.exists() {bail!("Previous incomplete download exists; remove only companion-download after inspecting it.");}
        execute(Command::new("git").arg("init").arg(&stage)).await?;
        execute(Command::new("git").args(["fetch","--depth=1",URL,REV]).current_dir(&stage)).await?;
        execute(Command::new("git").args(["checkout","--detach","FETCH_HEAD"]).current_dir(&stage)).await?;
        revision(&stage).await?;
        std::fs::rename(&stage,&dir)?;
    }
    revision(&dir).await?;
    if !python(root).is_file() {
        execute(Command::new("uv").args(["venv","--python","3.12"]).arg(root.join("companion-env"))).await?;
    }
    execute(Command::new("uv").args(["pip","install","--python"]).arg(python(root)).arg("-r").arg(dir.join("requirements.txt"))).await?;
    std::fs::create_dir_all(root.join("previews"))?;
    std::fs::write(root.join("companion-installed"),REV)?;
    println!("Companion installed. Run sensei pair.");
    Ok(())
}
pub async fn pair(root:&Path)->Result<()> {
    if !installed(root){bail!("Run sensei install-companion first");}
    revision(&source(root)).await?;
    if source(root).join("session.json").is_file(){println!("Pairing already exists; not replacing credentials.");return Ok(());}
    println!("CSP: ファイル → スマートフォンと接続。QRのURLはローカル端末にだけ入力してください。");
    execute(Command::new(python(root)).arg(source(root).join("setup_pairing.py")).current_dir(source(root))).await
}

pub struct Companion { client:RunningService<RoleClient,()> }
impl Companion {
    pub async fn start(root:&Path)->Result<Self> {
        if !installed(root){bail!("Companion is not installed");}
        revision(&source(root)).await?;
        let dir=source(root);
        let mut cmd=Command::new(python(root));
        cmd.arg("-u").arg(dir.join("server.py")).current_dir(&dir)
            .env("CSP_COMPANION_CONFIG",dir.join("session.json"))
            .env("CSP_COMPANION_OUTPUT",root.join("previews"))
            .env("PYTHONUTF8","1").env_remove("SENSEI_API_KEY").env_remove("OPENAI_API_KEY")
            .env_remove("ANTHROPIC_API_KEY").env_remove("GEMINI_API_KEY")
            .stderr(Stdio::inherit());
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
