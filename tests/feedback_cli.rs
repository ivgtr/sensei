use std::process::Command;
#[test]
fn pointer_requires_explicit_image_permission(){
    let output=Command::new(env!("CARGO_BIN_EXE_sensei")).args(["run","--pointer"]).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--vision"));
}
#[test]
fn voice_and_pointer_flags_are_discoverable(){
    let output=Command::new(env!("CARGO_BIN_EXE_sensei")).args(["run","--help"]).output().unwrap();
    assert!(output.status.success());
    let help=String::from_utf8(output.stdout).unwrap();
    for flag in ["--voice","--listen","--pointer"]{assert!(help.contains(flag));}
}
