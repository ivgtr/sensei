use serde_json::{json,Value};
use std::{io::Write,process::Command};

#[test]
fn analyze_detects_patterns_and_resets_at_gap(){
    let dir=tempfile::tempdir().unwrap();
    let path=dir.path().join("test.jsonl");
    let mut file=std::fs::File::create(&path).unwrap();
    writeln!(file,"{}",json!({"type":"header","version":1,"app":"krita.exe","coordinate_space":"window px"})).unwrap();
    for seq in 1..=6{
        writeln!(file,"{}",json!({"type":"action","event":{"seq":seq,"at_ms":seq*1000,"kind":"pointer_drag_candidate","generation":1,"bounds":[10,20,30,40],"duration_ms":100,"samples":10,"distance_px":25}})).unwrap();
    }
    writeln!(file,"{}",json!({"type":"gap","missing":1})).unwrap();
    for seq in 8..=9{
        writeln!(file,"{}",json!({"type":"action","event":{"seq":seq,"at_ms":seq*1000,"kind":"pointer_drag_candidate","generation":1,"bounds":[10,20,30,40],"duration_ms":100,"samples":10,"distance_px":25}})).unwrap();
    }
    writeln!(file,"{}",json!({"type":"end","events":8,"missing":1})).unwrap();
    drop(file);
    let output=Command::new(env!("CARGO_BIN_EXE_sensei")).arg("analyze").arg(&path).output().unwrap();
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    let rows:Vec<Value>=String::from_utf8(output.stdout).unwrap().lines().map(|l|serde_json::from_str(l).unwrap()).collect();
    assert!(rows.iter().filter_map(|r|r.pointer("/context/signals").and_then(Value::as_array)).flatten().any(|s|s["kind"]=="localized_activity"));
    let last=rows.iter().find(|r|r["type"]=="final_context").unwrap();
    assert_eq!(last["context"]["recent_count"],2);
    assert_eq!(last["context"]["signals"].as_array().unwrap().len(),0);
}
