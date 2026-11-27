use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::{Path, PathBuf}};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum PaintKind { Csp, Krita }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub paint: PaintKind,
    pub app: String,
    pub model: String,
    pub api_base: String,
}

pub fn default_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from(".")).join("Sensei").join("rust")
}
impl Config {
    pub fn provider(&self) -> crate::provider::Provider { crate::provider::Provider::from_base(&self.api_base) }
    pub fn new(paint: PaintKind, app: Option<String>, model: String, api_base: String) -> Result<Self> {
        let app = app.unwrap_or_else(|| match paint { PaintKind::Csp => "CLIPStudioPaint.exe", PaintKind::Krita => "krita.exe" }.into());
        let cfg = Self { paint, app: app.to_ascii_lowercase(), model, api_base: api_base.trim_end_matches('/').into() };
        cfg.validate()?;
        Ok(cfg)
    }
    pub fn validate(&self) -> Result<()> {
        if self.app.is_empty() || !self.app.to_ascii_lowercase().ends_with(".exe") ||
            self.app.contains(['/', '\\', ':']) || !valid_model_id(&self.model) {
            bail!("Specify an exact executable basename and model");
        }
        let u = url::Url::parse(&self.api_base)?;
        if u.scheme() != "https" || u.host_str().is_none() || !u.username().is_empty() ||
            u.password().is_some() || u.query().is_some() || u.fragment().is_some() {
            bail!("API base must be an HTTPS URL without embedded credentials, query or fragment");
        }
        Ok(())
    }
}

pub fn valid_model_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 &&
        !id.chars().any(|c| c.is_whitespace() || c.is_control())
}

pub fn load(root: &Path) -> Result<Config> {
    let cfg: Config = serde_json::from_slice(&fs::read(root.join("settings.json")).context("Run sensei setup first")?)?;
    cfg.validate()?;
    Ok(cfg)
}
pub fn save_new(root: &Path, cfg: &Config) -> Result<()> {
    cfg.validate()?;
    fs::create_dir_all(root)?;
    let data = serde_json::to_vec_pretty(cfg)?;
    let mut f = fs::OpenOptions::new().write(true).create_new(true).open(root.join("settings.json"))
        .context("Settings already exist; use a separate --root or edit settings.json locally")?;
    f.write_all(&data)?;
    Ok(())
}
/// Replace settings atomically without modifying endpoint-scoped credentials.
pub fn save_existing(root: &Path, cfg: &Config) -> Result<()> {
    cfg.validate()?;
    let path = root.join("settings.json");
    if !path.is_file() { bail!("Run sensei setup first"); }
    let mut tmp = tempfile::NamedTempFile::new_in(root)?;
    serde_json::to_writer_pretty(&mut tmp, cfg)?;
    tmp.write_all(b"\n")?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).context("Could not replace local settings")?;
    Ok(())
}

#[cfg(windows)]
fn entry(root: &Path, cfg: &Config) -> Result<keyring::Entry> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    Ok(keyring::Entry::new("Sensei", &format!("{}|{}", root.display(), cfg.api_base))?)
}
pub fn prompt_key(root: &Path, cfg: &Config) -> Result<()> {
    #[cfg(windows)]
    {
        println!("{} API endpoint: {} (キーはここに送信されます)", cfg.provider().label(), cfg.api_base);
        if let Some(url) = cfg.provider().key_url() { println!("キー発行: {url}"); }
        let key = rpassword::prompt_password("LLM API key (hidden): ")?;
        if key.trim().is_empty() { bail!("Empty API key"); }
        entry(root, cfg)?.set_password(key.trim())?;
        println!("Windows Credential Managerに保存しました。");
        Ok(())
    }
    #[cfg(not(windows))]
    { let _ = (root, cfg); bail!("Use SENSEI_API_KEY environment variable on non-Windows") }
}
/// Provider-specific credentials only. Never reuse a different provider's key.
pub fn read_provider_key(root: &Path, cfg: &Config) -> Result<String> {
    #[cfg(windows)]
    if let Ok(key) = entry(root, cfg)?.get_password() {
        if !key.trim().is_empty() { return Ok(key.trim().to_owned()); }
    }
    if let Some(name) = cfg.provider().env_key() {
        if let Ok(key) = std::env::var(name) {
            if !key.trim().is_empty() { return Ok(key.trim().to_owned()); }
        }
    }
    #[cfg(windows)]
    { bail!("No API key registered for this endpoint; use sensei key") }
    #[cfg(not(windows))]
    { let _ = root; bail!("No provider-specific API key environment variable is set") }
}
pub fn read_key(root: &Path, cfg: &Config) -> Result<String> {
    // Backward-compatible generic override ONLY for the original OpenAI endpoint.
    // This avoids sending an existing OpenAI override to another provider.
    if cfg.provider() == crate::provider::Provider::Openai {
        if let Ok(key) = std::env::var("SENSEI_API_KEY") {
            if !key.trim().is_empty() { return Ok(key.trim().to_owned()); }
        }
    }
    read_provider_key(root, cfg)
}
pub fn forget_key(root: &Path, cfg: &Config) -> Result<()> {
    #[cfg(windows)]
    { entry(root, cfg)?.delete_credential()?; Ok(()) }
    #[cfg(not(windows))]
    { let _ = (root, cfg); bail!("No Windows credential store") }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn model_change_preserves_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::new(PaintKind::Csp,None,"gpt-4.1-mini".into(),"https://api.openai.com/v1".into()).unwrap();
        save_new(dir.path(), &cfg).unwrap();
        cfg.model = "custom/model:1".into();
        save_existing(dir.path(), &cfg).unwrap();
        let saved = load(dir.path()).unwrap();
        assert_eq!(saved.model, "custom/model:1");
        assert_eq!(saved.api_base, cfg.api_base);
        assert_eq!(saved.app, cfg.app);
        cfg.model = "bad\nmodel".into();
        assert!(save_existing(dir.path(), &cfg).is_err());
        assert_eq!(load(dir.path()).unwrap().model, "custom/model:1");
    }
    #[test] fn config_no_secrets_or_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config::new(PaintKind::Csp,None,"gpt-4.1-mini".into(),"https://api.openai.com/v1/".into()).unwrap();
        save_new(dir.path(), &cfg).unwrap();
        assert!(save_new(dir.path(), &cfg).is_err());
        assert_eq!(load(dir.path()).unwrap().api_base, "https://api.openai.com/v1");
        assert!(Config::new(PaintKind::Csp,None,"m".into(),"http://example.com".into()).is_err());
    }
}
