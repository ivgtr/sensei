mod config;
mod desktop;
mod mentor;
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
        #[arg(long, default_value = "gpt-4.1-mini")]
        model: String,
        /// Explicitly trusted OpenAI-compatible HTTPS API base.
        #[arg(long, default_value = "https://api.openai.com/v1")]
        api_base: String,
        /// Install optional upstream Companion via uv and Git.
        #[arg(long)]
        companion: bool,
        /// Skip credential prompt (e.g. for a smoke test).
        #[arg(long)]
        no_key: bool,
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
            let cfg = config::Config::new(paint, app, model, api_base)?;
            config::save_new(&root, &cfg)?;
            if companion { paint::install(&root).await?; }
            if !no_key { config::prompt_key(&root, &cfg)?; }
            println!("設定を作成しました: {}", root.display());
            println!("次: sensei doctor → sensei run --vision");
            if companion { println!("CSP Companion を使う場合は sensei pair を実行してください。"); }
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
        Command::Run { vision, manual, max_calls_per_hour, cooldown_seconds, goal } => {
            if !cfg!(windows) { bail!("Desktop monitoring is Windows-only"); }
            if !(1..=120).contains(&max_calls_per_hour) || !(15..=3600).contains(&cooldown_seconds) {
                bail!("max calls: 1–120; cooldown: 15–3600 seconds");
            }
            let cfg = config::load(&root)?;
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
