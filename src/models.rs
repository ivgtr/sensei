//! Model discovery is an optional, non-inference GET to the configured endpoint.
use crate::config::{self, Config};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::{io::{self, BufRead, Write}, path::Path, time::Duration};


const MENU_LIMIT: usize = 8;

fn parse_ids(value: Value) -> Result<Vec<String>> {
    let data = value.get("data").and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Unexpected /models response"))?;
    let mut ids: Vec<String> = data.iter()
        .filter_map(|row| row.get("id").and_then(Value::as_str))
        .filter(|id| config::valid_model_id(id))
        .map(str::to_owned).collect();
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

pub async fn available(root: &Path, cfg: &Config) -> Result<Vec<String>> {
    cfg.validate()?;
    let key = config::read_key(root, cfg)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none()).build()?;
    let provider = cfg.provider();
    let request = client.get(format!("{}/models", cfg.api_base.trim_end_matches('/')));
    let request = if provider == crate::provider::Provider::Anthropic { request.query(&[("limit", "1000")]) } else { request };
    let response = provider.auth(request, &key).send().await.map_err(|_| anyhow!("Model list request failed"))?;
    if !response.status().is_success() {
        bail!("Model list HTTP {}", response.status());
    }
    parse_ids(response.json().await.map_err(|_| anyhow!("Invalid model list JSON"))?)
}

fn candidates(cfg: &Config, discovered: Option<&[String]>) -> Vec<String> {
    let presets = cfg.provider().presets();
    let mut result = Vec::new();
    let mut add = |id: &str| {
        if result.len() < MENU_LIMIT && !result.iter().any(|existing| existing == id) {
            result.push(id.to_owned());
        }
    };
    if let Some(ids) = discovered {
        // Prefer familiar multimodal Chat Completions model IDs, but only if listed.
        for &id in presets {
            if ids.iter().any(|available| available == id) { add(id); }
        }
        if ids.contains(&cfg.model) { add(&cfg.model); }
        // If no known models are listed, expose a small unverified selection.
        // Full discovery output is accessible with `sensei models`.
        if !ids.iter().any(|id| presets.contains(&id.as_str())) {
            for id in ids { add(id); }
        }
    } else {
        add(&cfg.model);
        for &id in presets { add(id); }
    }
    result
}

fn resolve_choice(input: &str, current: &str, options: &[String]) -> Result<String> {
    let input = input.trim();
    if input.is_empty() { return Ok(current.to_owned()); }
    // The number prefix is optional; explicit "id:" permits purely numeric IDs.
    let id = if let Some(id) = input.strip_prefix("id:") {
        id.trim().to_owned()
    } else if input.bytes().all(|c| c.is_ascii_digit()) {
        let n = input.parse::<usize>().map_err(|_| anyhow!("Invalid selection"))?;
        options.get(n.checked_sub(1).ok_or_else(|| anyhow!("Invalid selection"))?)
            .cloned().ok_or_else(|| anyhow!("Invalid selection"))?
    } else { input.to_owned() };
    if !config::valid_model_id(&id) { bail!("Invalid model ID"); }
    Ok(id)
}

pub async fn choose(root: &Path, cfg: &Config) -> Result<String> {
    let discovered = match available(root, cfg).await {
        Ok(ids) => Some(ids),
        Err(e) => { eprintln!("モデル一覧を取得できません: {e}。直接入力も可能です。"); None }
    };
    let options = candidates(cfg, discovered.as_deref());
    let default = if discovered.as_ref().is_some_and(|ids| !ids.contains(&cfg.model)) {
        options.first().map(String::as_str).unwrap_or(&cfg.model)
    } else { &cfg.model };
    println!("現在のモデル: {} / Enterで選択: {}", cfg.model, default);
    println!("モデルを選択してください。候補の全機能への対応は未検証です。");
    for (i, id) in options.iter().enumerate() { println!("  {}) {}", i + 1, id); }
    if discovered.is_some() { println!("全モデルIDは sensei models で確認できます。"); }
    let mut stdin = io::stdin().lock();
    loop {
        print!("番号 / モデルID直接入力 / Enterで表示された既定値: ");
        io::stdout().flush()?;
        let mut line = String::new();
        if stdin.read_line(&mut line)? == 0 { return Ok(default.to_owned()); }
        match resolve_choice(&line, default, &options) {
            Ok(id) => return Ok(id),
            Err(_) => eprintln!("有効な番号かモデルIDを入力してください。数字だけのIDは id:123 のように指定できます。"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn cfg() -> Config {
        Config::new(config::PaintKind::Csp, None, crate::provider::Provider::Openai.default_model().into(), "https://api.openai.com/v1".into()).unwrap()
    }
    #[test] fn discovery_filters_and_deduplicates() {
        assert_eq!(parse_ids(json!({"data":[{"id":"b"},{"id":"a"},{"id":"a"},{"id":"\u{001b}[31m"},{"id":null}]})).unwrap(), vec!["a","b"]);
        assert!(parse_ids(json!({"error":"bad"})).is_err());
    }
    #[test] fn discovered_presets_are_prioritized() {
        let cfg = cfg();
        let ids = vec!["embedding".into(), "gpt-5-mini".into(), "gpt-4.1-mini".into()];
        assert_eq!(candidates(&cfg, Some(&ids)), vec!["gpt-4.1-mini","gpt-5-mini"]);
        assert!(candidates(&cfg, None).contains(&cfg.model));
        let custom = Config::new(config::PaintKind::Csp,None,"old".into(),"https://example.com/v1".into()).unwrap();
        assert_eq!(candidates(&custom, Some(&["provider/model".into()])),vec!["provider/model"]);
    }
    #[test] fn direct_or_number_or_default() {
        let options = vec!["gpt-4.1-mini".into(),"gpt-5-mini".into()];
        assert_eq!(resolve_choice("2","old",&options).unwrap(),"gpt-5-mini");
        assert_eq!(resolve_choice("vendor/new-model:free","old",&options).unwrap(),"vendor/new-model:free");
        assert_eq!(resolve_choice(" ","old",&options).unwrap(),"old");
        assert_eq!(resolve_choice("id:123","old",&options).unwrap(),"123");
        assert!(resolve_choice("0","old",&options).is_err());
        assert!(resolve_choice("3","old",&options).is_err());
        assert!(resolve_choice("bad model","old",&options).is_err());
    }
}
