//! Native MCP server for external hosts. No LLM requests; scoped, in-memory observer.
use crate::{config::Config,desktop::Observer};
use anyhow::Result;
use rmcp::{handler::server::router::tool::ToolRouter, model::*, tool, tool_handler, tool_router, transport::stdio, ErrorData as McpError, ServiceExt};
use serde_json::json;
use std::sync::Arc;

#[derive(Clone)]
struct Sensei {observer:Arc<Observer>,app:String,vision:bool,tool_router:ToolRouter<Self>}
#[tool_router]
impl Sensei{
    fn new(cfg:Config,vision:bool)->Self{
        Self{observer:Arc::new(Observer::start(cfg.app.clone())),app:cfg.app,vision,tool_router:Self::tool_router()}
    }
    #[tool(description="Read target application, observer status and image permissions. No model request.")]
    async fn sensei_status(&self)->Result<CallToolResult,McpError>{
        let s=self.observer.snapshot();
        Ok(CallToolResult::success(vec![Content::text(json!({"app":self.app,"active":s.active,"target_seen":s.target.is_some(),"vision":self.vision,"native_pen_registered":s.native_registered,"native_reports":s.native_reports,"native_strokes":s.native_strokes}).to_string())]))
    }
    #[tool(description="Read recent native pen stroke summaries, pointer and shortcut candidates in the target window; no keyboard text or screenshots.")]
    async fn desktop_recent_actions(&self)->Result<CallToolResult,McpError>{
        let s=self.observer.snapshot();
        let a:Vec<_>=s.actions.iter().rev().take(40).map(crate::session::compact_action).collect();
        Ok(CallToolResult::success(vec![Content::text(json!({"actions_newest_first":a,"coordinate_space":"window-relative pixels, not canvas","drawing":s.drawing,"summary":crate::session::summarize(s.actions.iter()),"pen_coordinate_space":"normalized tablet logical range, not canvas"}).to_string())]))
    }
    #[tool(description="Analyze recent and preceding 30-second windows. Returns local behavioral signals and evidence, not artistic diagnoses. No API or images.")]
    async fn desktop_work_context(&self)->Result<CallToolResult,McpError>{
        let s=self.observer.snapshot();
        let c=crate::context::snapshot(&s,std::time::Instant::now());
        Ok(CallToolResult::success(vec![Content::text(json!({"active":s.active,"drawing":s.drawing,"context":c}).to_string())]))
    }
    #[tool(description="Request a PNG of the previously observed target window. Only enabled with sensei mcp --vision. No whole-desktop capture.")]
    async fn paint_preview(&self)->Result<CallToolResult,McpError>{
        if !self.vision{return Ok(CallToolResult::error(vec![Content::text("Image sharing disabled")]));}
        let s=self.observer.snapshot();
        let Some(t)=s.target else{return Ok(CallToolResult::error(vec![Content::text("No target seen")]));};
        let o=self.observer.clone();
        match tokio::task::spawn_blocking(move||o.image(t)).await{
            Ok(Ok(img))=>Ok(CallToolResult::success(vec![Content::image(img,"image/png")])),
            _=>Ok(CallToolResult::error(vec![Content::text("Target unavailable or capture failed")])),
        }
    }
}
#[tool_handler]
impl rmcp::ServerHandler for Sensei{
    fn get_info(&self)->ServerInfo{
        ServerInfo{instructions:Some("Action-first observer. Inspect only when necessary. Window coordinates are not canvas coordinates.".into()),capabilities:ServerCapabilities::builder().enable_tools().build(),..Default::default()}
    }
}
pub async fn run(cfg:Config,vision:bool)->Result<()>{
    let service=Sensei::new(cfg,vision).serve(stdio()).await?;
    service.waiting().await?;Ok(())
}
