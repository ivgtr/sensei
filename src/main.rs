mod config;
mod desktop;
mod mentor;
mod models;
mod paint;
mod server;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::{path::PathBuf, time::Duration};

#[derive(Parser)]
#[command(version, about = "Sensei: action-first illustration mentor")]
struct Cli {
    /// Local data directory, not the repository.
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create local configuration; optionally install CSP Companion.
    Setup {
        #[arg(long, default_value = "csp")]
        paint: config::PaintKind,
        /// Exact executable basename override.
        #[arg(long)]
        app: Option<String>,
        /// Model ID; omit to select interactively after API key registration.
        #[arg(long)]
        model: Option<String>,
        /// Explicitly trusted OpenAI-compatible HTTPS API base.
        #[arg(long, default_value = "https://api.openai.com/v1")]
        api_base: String,
        /// Install optional upstream Companion and managed uv/Python.
        #[arg(long)]
        companion: bool,
        /// Skip credential prompt (e.g. for a smoke test).
        #[arg(long)]
        no_key: bool,
    },
    /// List model IDs returned by the configured API.
    Models,
    /// Select and save a model (omit ID for interactive selection).
    Model {
        /// Exact model ID, e.g. gpt-4.1-mini.
        id: Option<String>,
    },
    /// Enter or replace API key in Windows Credential Manager.
    Key,
    /// Remove the API key from Windows Credential Manager.
    ForgetKey,
    /// Install the optional Companion integration in an existing setup.
    InstallCompanion,
    /// Pair CSP using the local QR URL; never send this URL to an AI.
    Pair,
    /// Local checks; --api explicitly performs one billable test request.
    Doctor { #[arg(long)] api: bool },
    /// Start a local monitoring session; Ctrl+C or :quit to stop.
    Run {
        /// Override the saved model for this session only.
        #[arg(long)]
        model: Option<String>,
        /// Allow on-demand artwork/window images to be sent to the model.
        #[arg(long)]
        vision: bool,
        /// Disable automatic model calls, keep manual questions.
        #[arg(long)]
        manual: bool,
        #[arg(long, default_value_t = 20)]
        max_calls_per_hour: u32,
        #[arg(long, default_value_t = 60)]
        cooldown_seconds: u64,
        /// Optional session goal, e.g. practice torso construction.
        #[arg(long, default_value = "イラスト制作の上達")]
        goal: String,
    },
    /// Expose local read-only tools over stdio, no API key or LLM calls.
    Mcp {
        /// Explicitly allow on-demand target-window images through MCP.
        #[arg(long)]
        vision: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = cli.root.unwrap_or_else(config::default_root);
    let root = if root.is_absolute() {root} else {std::env::current_dir()?.join(root)};
    match cli.command {
        Command::Setup { paint, app, model, api_base, companion, no_key } => {
            if companion && paint != config::PaintKind::Csp {
                bail!("Companion is for CSP only");
            }
            let interactive_model = model.is_none() && !no_key;
            let mut cfg = config::Config::new(paint, app, model.unwrap_or_else(|| models::DEFAULT_MODEL.into()), api_base)?;
            config::save_new(&root, &cfg)?;
            if companion { paint::install(&root).await?; }
            if !no_key { config::prompt_key(&root, &cfg)?; }
            if interactive_model {
                cfg.model = models::choose(&root, &cfg).await?;
                config::save_existing(&root, &cfg)?;
            }
            println!("使用モデル: {}", cfg.model);
            println!("設定を作成しました: {}", root.display());
            println!("次: sensei doctor → sensei run --vision");
            if companion { println!("CSP Companion を使う場合は sensei pair を実行してください。"); }
        }
        Command::Models => {
            let cfg = config::load(&root)?;
            let ids = models::available(&root, &cfg).await?;
            println!("API: {} / 現在のモデル: {}", cfg.api_base, cfg.model);
            println!("モデル一覧（画像入力・JSON mode・Chat Completions 対応を保証するものではありません）:");
            for id in ids { println!("  {id}"); }
        }
        Command::Model { id } => {
            let mut cfg = config::load(&root)?;
            cfg.model = match id { Some(id) => id.trim().to_owned(), None => models::choose(&root, &cfg).await? };
            config::save_existing(&root, &cfg)?;
            println!("使用モデルを保存しました: {}（次回起動から有効）", cfg.model);
        }
        Command::Key => config::prompt_key(&root, &config::load(&root)?)?,
        Command::ForgetKey => config::forget_key(&root, &config::load(&root)?)?,
        Command::InstallCompanion => {
            let cfg = config::load(&root)?;
            if cfg.paint != config::PaintKind::Csp { bail!("CSP setup required"); }
            paint::install(&root).await?;
        }
        Command::Pair => paint::pair(&root).await?,
        Command::Doctor { api } => {
            let cfg = config::load(&root)?;
            println!("target={} model={} endpoint={}", cfg.app, cfg.model, cfg.api_base);
            println!("Windows observer: {}", if cfg!(windows) { "available (実機で確認)" } else { "unsupported" });
            if paint::installed(&root) {
                let p = paint::Companion::start(&root).await?;
                println!("Companion MCP handshake: OK");
                let status = p.call("csp_status", serde_json::json!({})).await?;
                println!("Companion status: {}", status.get("pairing_required").and_then(|v| v.as_bool()).map(|v| if v {"pairing required"} else {"paired"}).unwrap_or("unknown"));
                p.close().await;
            }
            if api {
                let key = config::read_key(&root, &cfg)?;
                let m = mentor::Model::new(&cfg, key)?;
                let d = m.ask(&serde_json::json!({"question":"疎通確認。短く回答してください。"}), None, true).await?;
                println!("API response: {:?}", d.kind);
            } else { println!("APIは未呼び出し。sensei doctor --api で疎通確認できます。"); }
        }
        Command::Run { model: model_override, vision, manual, max_calls_per_hour, cooldown_seconds, goal } => {
            if !cfg!(windows) { bail!("Desktop monitoring is Windows-only"); }
            if !(1..=120).contains(&max_calls_per_hour) || !(15..=3600).contains(&cooldown_seconds) {
                bail!("max calls: 1–120; cooldown: 15–3600 seconds");
            }
            let mut cfg = config::load(&root)?;
            if let Some(id) = model_override { cfg.model = id.trim().to_owned(); cfg.validate()?; }
            println!("使用モデル: {}", cfg.model);
            let key = config::read_key(&root, &cfg).context("sensei key でAPIキーを登録してください")?;
            let model = mentor::Model::new(&cfg, key)?;
            let obs = desktop::Observer::start(cfg.app.clone());
            let companion = if cfg.paint == config::PaintKind::Csp && paint::installed(&root) {
                match paint::Companion::start(&root).await {
                    Ok(p) => match p.call("csp_connect", serde_json::json!({})).await {
                        Ok(_) => Some(p),
                        Err(_) => { eprintln!("Companion接続不可。Windows観測のみで続行します。"); p.close().await; None }
                    },
                    Err(_) => { eprintln!("Companion起動不可。Windows観測のみで続行します。"); None }
                }
            } else { None };
            let result = mentor::run(model, obs, companion.as_ref(), mentor::Options {
                vision, manual, max_calls: max_calls_per_hour,
                cooldown: Duration::from_secs(cooldown_seconds), goal
            }).await;
            if let Some(p) = companion { p.close().await; }
            result?;
        }
        Command::Mcp { vision } => {
            let cfg = config::load(&root)?;
            if !cfg!(windows) { bail!("Desktop monitoring is Windows-only"); }
            server::run(cfg, vision).await?;
        }
    }
    Ok(())
}
