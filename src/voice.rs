//! Local Windows speech via installed System.Speech engines. No audio files,
//! remote transcription, shell interpolation, or always-on microphone.
use anyhow::{bail,Result};
use std::time::{Duration,Instant};
use tokio::sync::mpsc;

#[cfg(windows)]
mod platform {
    use super::*;
    use base64::{Engine,engine::general_purpose::STANDARD};
    use std::{process::Stdio,sync::{Arc,atomic::{AtomicBool,Ordering}},thread};
    use tokio::{io::AsyncWriteExt,process::{Child,Command}};
    use windows_sys::Win32::UI::{
        Input::KeyboardAndMouse::{RegisterHotKey,UnregisterHotKey,MOD_ALT,MOD_CONTROL,MOD_NOREPEAT,VK_F9},
        WindowsAndMessaging::{PeekMessageW,MSG,PM_REMOVE,WM_HOTKEY},
    };
    const COMMON:&str=r#"
$ErrorActionPreference = 'Stop'
[Console]::InputEncoding = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
Add-Type -AssemblyName System.Speech
"#;
    const SPEAK:&str=r#"
$text = [Console]::In.ReadToEnd()
$s = New-Object System.Speech.Synthesis.SpeechSynthesizer
try {
    $voice = $s.GetInstalledVoices() | Where-Object { $_.Enabled -and $_.VoiceInfo.Culture.Name -eq 'ja-JP' } | Select-Object -First 1
    if (-not $voice) { throw 'Japanese SAPI voice not installed' }
    $s.SelectVoice($voice.VoiceInfo.Name)
    $s.SetOutputToDefaultAudioDevice()
    $s.Speak($text)
} finally { $s.Dispose() }
"#;
    const LISTEN:&str=r#"
$info = [System.Speech.Recognition.SpeechRecognitionEngine]::InstalledRecognizers() | Where-Object { $_.Culture.Name -eq 'ja-JP' } | Select-Object -First 1
if (-not $info) { throw 'Japanese desktop speech recognizer not installed' }
$r = [System.Speech.Recognition.SpeechRecognitionEngine]::new($info)
try {
    $r.SetInputToDefaultAudioDevice()
    $r.LoadGrammar((New-Object System.Speech.Recognition.DictationGrammar))
    $r.EndSilenceTimeout = [TimeSpan]::FromMilliseconds(700)
    $r.EndSilenceTimeoutAmbiguous = [TimeSpan]::FromMilliseconds(1200)
    [Console]::Error.WriteLine('Sensei: microphone ready; speak now')
    $result = $r.Recognize([TimeSpan]::FromSeconds(6))
    if ($result) {
        @{text=$result.Text;confidence=$result.Confidence} | ConvertTo-Json -Compress
    } else { @{text='';confidence=0} | ConvertTo-Json -Compress }
} finally { $r.Dispose() }
"#;
    const DIAG:&str=r#"
$s = New-Object System.Speech.Synthesis.SpeechSynthesizer
try {
    $voices = @($s.GetInstalledVoices() | Where-Object { $_.Enabled } | ForEach-Object { @{name=$_.VoiceInfo.Name;culture=$_.VoiceInfo.Culture.Name} })
    $recognizers = @([System.Speech.Recognition.SpeechRecognitionEngine]::InstalledRecognizers() | ForEach-Object { @{name=$_.Name;culture=$_.Culture.Name} })
    @{voices=$voices;recognizers=$recognizers} | ConvertTo-Json -Depth 4 -Compress
} finally { $s.Dispose() }
"#;
    fn command(script:&str)->Result<Command>{
        let root=std::env::var_os("SystemRoot").ok_or_else(||anyhow::anyhow!("SystemRoot not set"))?;
        let exe=std::path::PathBuf::from(root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        if !exe.is_file(){bail!("Windows PowerShell unavailable");}
        let encoded=STANDARD.encode(format!("{COMMON}\n{script}").encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>());
        let mut c=Command::new(exe);
        c.args(["-NoLogo","-NoProfile","-NonInteractive","-EncodedCommand",&encoded])
            .kill_on_drop(true).creation_flags(0x08000000);
        for name in ["SENSEI_API_KEY","OPENAI_API_KEY","GEMINI_API_KEY","ANTHROPIC_API_KEY","OPENROUTER_API_KEY"]{c.env_remove(name);}
        Ok(c)
    }
    pub struct Voice { child:Option<Child> }
    impl Voice{
        pub fn new()->Self{Self{child:None}}
        pub async fn stop(&mut self){
            if let Some(mut child)=self.child.take(){let _=child.kill().await;let _=child.wait().await;}
        }
        pub async fn speak(&mut self,text:&str)->Result<()>{
            self.stop().await;
            let mut c=command(SPEAK)?;
            let mut child=c.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::inherit()).spawn()?;
            let mut stdin=child.stdin.take().ok_or_else(||anyhow::anyhow!("No speech input"))?;
            stdin.write_all(text.as_bytes()).await?;
            drop(stdin);
            self.child=Some(child);Ok(())
        }
        pub async fn listen(&mut self)->Result<String>{
            self.stop().await;
            eprintln!("Sensei: 日本語の音声認識を準備中（最大20秒、音声はローカル処理）");
            let mut c=command(LISTEN)?;
            let out=tokio::time::timeout(Duration::from_secs(20),c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::inherit()).output()).await??;
            if !out.status.success(){bail!("Local recognition failed; check voice-test and microphone permission");}
            super::transcript(&out.stdout)
        }
        pub async fn finish(&mut self)->Result<()>{
            if let Some(mut child)=self.child.take(){
                let status=tokio::time::timeout(Duration::from_secs(45),child.wait()).await??;
                if !status.success(){bail!("Local speech synthesis failed");}
            }
            Ok(())
        }
    }
    pub async fn diagnostic()->Result<()>{
        let out=tokio::time::timeout(Duration::from_secs(15),command(DIAG)?.stdin(Stdio::null()).output()).await??;
        if !out.status.success(){bail!("System.Speech unavailable");}
        let value:serde_json::Value=serde_json::from_slice(&out.stdout)?;
        println!("{}",serde_json::to_string_pretty(&value)?);
        Ok(())
    }
    pub struct Hotkey{stop:Arc<AtomicBool>,worker:Option<thread::JoinHandle<()>>}
    impl Hotkey{
        pub fn start(tx:mpsc::Sender<Instant>,obs:Arc<crate::desktop::Observer>)->Result<Self>{
            let stop=Arc::new(AtomicBool::new(false));let flag=stop.clone();
            let (ready,rx)=std::sync::mpsc::sync_channel(1);
            let worker=thread::spawn(move||unsafe{
                let mut msg:MSG=std::mem::zeroed();
                // Create the thread message queue before registration.
                PeekMessageW(&mut msg,std::ptr::null_mut(),0,0,0);
                let ok=RegisterHotKey(std::ptr::null_mut(),1,MOD_CONTROL|MOD_ALT|MOD_NOREPEAT,VK_F9 as u32)!=0;
                let _=ready.send(ok);
                if !ok{return;}
                while !flag.load(Ordering::Relaxed){
                    while PeekMessageW(&mut msg,std::ptr::null_mut(),0,0,PM_REMOVE)!=0{
                        if msg.message==WM_HOTKEY&&msg.wParam==1&&obs.target_foreground(){let _=tx.try_send(Instant::now());}
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                UnregisterHotKey(std::ptr::null_mut(),1);
            });
            if !rx.recv_timeout(Duration::from_secs(2)).unwrap_or(false){
                stop.store(true,Ordering::Relaxed);let _=worker.join();bail!("Ctrl+Alt+F9 registration failed (possibly already in use)");
            }
            Ok(Self{stop,worker:Some(worker)})
        }
    }
    impl Drop for Hotkey{fn drop(&mut self){self.stop.store(true,Ordering::Relaxed);if let Some(w)=self.worker.take(){let _=w.join();}}}
}
#[cfg(windows)]
pub use platform::{Voice,Hotkey,diagnostic};
#[cfg(not(windows))]
pub struct Voice;
#[cfg(not(windows))]
impl Voice{
    pub fn new()->Self{Self}
    pub async fn stop(&mut self){}
    pub async fn speak(&mut self,_:&str)->Result<()>{bail!("Windows only")}
    pub async fn listen(&mut self)->Result<String>{bail!("Windows only")}
    pub async fn finish(&mut self)->Result<()>{bail!("Windows only")}
}
#[cfg(not(windows))]
pub struct Hotkey;
#[cfg(not(windows))]
impl Hotkey{pub fn start(_:mpsc::Sender<Instant>,_:std::sync::Arc<crate::desktop::Observer>)->Result<Self>{bail!("Windows only")}}
#[cfg(not(windows))]
pub async fn diagnostic()->Result<()>{bail!("Windows only")}

fn transcript(bytes:&[u8])->Result<String>{
    #[derive(serde::Deserialize)]
    struct Heard{text:String,confidence:f64}
    let heard:Heard=serde_json::from_slice(bytes.strip_prefix(&[0xef,0xbb,0xbf]).unwrap_or(bytes))?;
    if !heard.confidence.is_finite()||heard.confidence<0.35||heard.confidence>1.0{bail!("No sufficiently confident speech recognized");}
    let text:String=heard.text.chars().filter(|c|!c.is_control()).take(500).collect();
    if text.trim().is_empty(){bail!("No speech recognized");}
    Ok(text)
}
pub async fn test(speak:bool,listen:bool)->Result<()>{
    diagnostic().await?;
    let mut voice=Voice::new();
    if speak{voice.speak("こんにちは。Senseiの音声テストです。").await?;voice.finish().await?;}
    if listen{println!("Recognized: {}",voice.listen().await?);}
    Ok(())
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn speech_is_data_and_low_confidence_rejected(){
        assert_eq!(transcript(br#"{"text":":accept","confidence":0.8}"#).unwrap(),":accept");
        assert!(transcript(br#"{"text":"x","confidence":0.1}"#).is_err());
        assert!(transcript(br#"{"text":"","confidence":0.8}"#).is_err());
    }
}
