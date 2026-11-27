#[cfg(windows)]
#[tokio::test]
async fn stdio_smoke() {
    use rmcp::{ServiceExt, transport::TokioChildProcess, model::CallToolRequestParam};
    use tokio::process::Command;
    use std::time::Duration;
    let root=tempfile::tempdir().unwrap();
    let bin=env!("CARGO_BIN_EXE_sensei");
    let result=Command::new(bin).arg("--root").arg(root.path()).args(["setup","--no-key"]).status().await.unwrap();
    assert!(result.success());
    let mut cmd=Command::new(bin);cmd.arg("--root").arg(root.path()).arg("mcp");
    let client=tokio::time::timeout(Duration::from_secs(30),().serve(TokioChildProcess::new(cmd).unwrap())).await.unwrap().unwrap();
    let tools=client.list_tools(Default::default()).await.unwrap();
    assert_eq!(tools.tools.len(),3);
    let status=client.call_tool(CallToolRequestParam{name:"sensei_status".into(),arguments:None}).await.unwrap();
    assert_ne!(status.is_error,Some(true));
    let image=client.call_tool(CallToolRequestParam{name:"paint_preview".into(),arguments:None}).await.unwrap();
    assert_eq!(image.is_error,Some(true));
    client.cancel().await.unwrap();
}
