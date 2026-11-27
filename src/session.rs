//! Explicit, local-only diagnostics and recording. Never calls an LLM.
use crate::desktop::{Action,Observer,Snapshot};
use anyhow::{bail,Context,Result};
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use std::{collections::BTreeMap,fs::{File,OpenOptions},io::{BufRead,BufReader,BufWriter,Write},path::{Path,PathBuf},time::{Duration,Instant}};

pub fn compact_action(a:&Action)->Value{
    let mut v=serde_json::to_value(a).unwrap_or(Value::Null);
    if let Some(p)=v.get_mut("pen").and_then(Value::as_object_mut){p.remove("trace");}
    v
}
pub fn summarize<'a>(actions:impl IntoIterator<Item=&'a Action>)->Value{
    let mut kinds=BTreeMap::<String,u64>::new();
    let (mut native,mut duration,mut samples,mut pressure_n)=(0u64,0u64,0u64,0u64);
    let (mut sum,mut min,mut max)=(0.0f64,None::<f64>,None::<f64>);
    let mut pointer_distance=0u64;
    for a in actions{
        *kinds.entry(a.kind.clone()).or_default()+=1;
        pointer_distance=pointer_distance.saturating_add(a.distance_px);
        if let Some(p)=&a.pen{
            native+=1;duration=duration.saturating_add(a.duration_ms);samples=samples.saturating_add(u64::from(p.samples));
            if let Some(mean)=p.pressure_mean{
                pressure_n=pressure_n.saturating_add(u64::from(p.pressure_samples));
                sum+=mean*f64::from(p.pressure_samples);
            }
            if let Some(v)=p.pressure_min{min=Some(min.map_or(v,|a|a.min(v)));}
            if let Some(v)=p.pressure_max{max=Some(max.map_or(v,|a|a.max(v)));}
        }
    }
    json!({"counts":kinds,"native_pen_strokes":native,"pen_duration_ms":duration,"pen_samples":samples,
        "pressure_samples":pressure_n,"pressure_mean":(pressure_n>0).then(||sum/pressure_n as f64),"pressure_min":min,"pressure_max":max,
        "pointer_distance_px":pointer_distance})
}
pub fn devices()->Result<()>{
    #[cfg(windows)]{
        let devices=crate::pen::native::devices()?;
        println!("{}",serde_json::to_string_pretty(&json!({"standard_pen_collections":devices,"note":"Advertised capabilities only. Use observe to test report delivery. No serial numbers or device paths."}))?);
        Ok(())
    }
    #[cfg(not(windows))]{bail!("Device enumeration is Windows-only")}
}
#[derive(Debug,Serialize,Deserialize)]
#[serde(tag="type",rename_all="snake_case")]
enum Row{
    Header{version:u32,app:String,coordinate_space:String},
    Action{event:Action},
    Gap{missing:u64},
    End{events:u64,missing:u64},
}
struct Writer{out:BufWriter<File>,events:u64,missing:u64}
impl Writer{
    fn new(path:&Path,app:&str)->Result<Self>{
        let file=OpenOptions::new().write(true).create_new(true).open(path).context("Cannot create recording (existing files are never overwritten)")?;
        let mut w=Self{out:BufWriter::new(file),events:0,missing:0};
        w.row(&Row::Header{version:1,app:app.into(),coordinate_space:"pointer bounds: window relative px; pen.xy: normalized tablet logical units; tilt: raw logical units".into()})?;Ok(w)
    }
    fn row(&mut self,row:&Row)->Result<()>{serde_json::to_writer(&mut self.out,row)?;self.out.write_all(b"\n")?;self.out.flush()?;Ok(())}
    fn event(&mut self,event:&Action)->Result<()>{self.row(&Row::Action{event:event.clone()})?;self.events+=1;Ok(())}
    fn gap(&mut self,missing:u64)->Result<()>{if missing>0{self.row(&Row::Gap{missing})?;self.missing+=missing;}Ok(())}
    fn finish(&mut self)->Result<()>{self.row(&Row::End{events:self.events,missing:self.missing})?;self.out.get_ref().sync_all()?;Ok(())}
}
fn drain(s:&Snapshot,last:&mut u64,w:&mut Option<Writer>)->Result<()>{
    for a in s.actions.iter().filter(|a|a.seq>*last).cloned().collect::<Vec<_>>(){
        let gap=a.seq.saturating_sub(*last).saturating_sub(1);
        if gap>0{eprintln!("Missed {gap} events (bounded ring/window switch)");if let Some(w)=w{w.gap(gap)?;}}
        if let Some(w)=w{w.event(&a)?;}
        println!("{}",compact_action(&a));
        *last=a.seq;
    }
    // A target switch can clear all unseen events.
    if s.seq>*last {
        let gap=s.seq-*last;eprintln!("Missed {gap} events (window switch)");
        if let Some(w)=w{w.gap(gap)?;}*last=s.seq;
    }
    Ok(())
}
pub async fn observe(app:String,path:Option<PathBuf>,seconds:u64)->Result<()>{
    let mut w=path.as_deref().map(|p|Writer::new(p,&app)).transpose()?;
    eprintln!("Local observation: {app}; no API, no images. Ctrl+C stops. Records include drawing trajectories if a file was requested.");
    let obs=Observer::start(app);
    let start=Instant::now();let mut tick=tokio::time::interval(Duration::from_millis(100));
    let mut last=0;let mut last_status=Instant::now()-Duration::from_secs(5);
    loop{
        tokio::select!{
            _=tokio::signal::ctrl_c()=>break,
            _=tick.tick()=>{
                let s=obs.snapshot();drain(&s,&mut last,&mut w)?;
                if last_status.elapsed()>=Duration::from_secs(5){
                    eprintln!("target={} active={} drawing={} raw_pen_registered={} pen_reports={} pen_strokes={}",s.target.is_some(),s.active,s.drawing,s.native_registered,s.native_reports,s.native_strokes);
                    last_status=Instant::now();
                }
                if seconds>0&&start.elapsed()>=Duration::from_secs(seconds){break;}
            }
        }
    }
    drain(&obs.snapshot(),&mut last,&mut w)?;
    if let Some(w)=w.as_mut(){w.finish()?;}
    eprintln!("Observation stopped.");Ok(())
}
const MAX_LINE:usize=128*1024;
pub async fn replay(path:&Path,timed:bool,speed:f64)->Result<()>{
    if !speed.is_finite()||!(0.1..=100.0).contains(&speed){bail!("Replay speed must be 0.1–100");}
    let mut reader=BufReader::new(File::open(path)?);
    let mut line=Vec::new();let mut header=false;let mut ended=false;let mut last_seq=0u64;let mut last_time=None::<u64>;
    let mut count=0u64;let mut missing=0u64;let mut kinds=BTreeMap::<String,u64>::new();
    let mut pressure_samples=0u64;let mut pressure_sum=0.0;
    loop{
        line.clear();
        // Bound untrusted local input before allocating a full line.
        let mut limited=std::io::Read::take(&mut reader,(MAX_LINE+1) as u64);
        let n=limited.read_until(b'\n',&mut line)?;
        if n==0{break;}
        if n>MAX_LINE{bail!("Recording line exceeds size limit");}
        let row:Row=serde_json::from_slice(&line).context("Invalid recording row")?;
        if ended{bail!("Unexpected row after end");}
        match row{
            Row::Header{version,app,..} if !header=>{
                if version!=1{bail!("Unsupported recording version");}header=true;println!("{}",json!({"app":app,"version":version}));
            }
            Row::Action{event} if header=>{
                if event.seq<=last_seq || !["pen_stroke","pointer_drag_candidate","click_candidate","ctrl_z_candidate","ctrl_y_candidate"].contains(&event.kind.as_str()){bail!("Invalid recording event");}
                if event.pen.as_ref().is_some_and(|p|p.trace.len()>128){bail!("Invalid pen trace");}
                if timed{if let Some(t)=last_time{
                    // Avoid unbounded waits for malformed or idle-heavy logs.
                    let ms=event.at_ms.saturating_sub(t).min(60_000);
                    tokio::select!{_=tokio::signal::ctrl_c()=>return Ok(()),_=tokio::time::sleep(Duration::from_secs_f64(ms as f64/1000.0/speed))=>{}}
                }}
                last_time=Some(last_time.unwrap_or(0).max(event.at_ms));last_seq=event.seq;
                *kinds.entry(event.kind.clone()).or_default()+=1;count+=1;
                if let Some(p)=&event.pen{if let Some(v)=p.pressure_mean{
                    pressure_samples=pressure_samples.saturating_add(u64::from(p.pressure_samples));
                    pressure_sum+=v*f64::from(p.pressure_samples);
                }}
                println!("{}",compact_action(&event));
            }
            Row::Gap{missing:n} if header=>{missing=missing.saturating_add(n);eprintln!("Recording gap: {n}");}
            Row::End{events,missing:n} if header=>{if events!=count||n!=missing{bail!("Recording totals mismatch");}ended=true;}
            _=>bail!("Invalid recording row order"),
        }
    }
    if !header{bail!("Recording header missing");}
    println!("{}",json!({"events":count,"missing":missing,"complete":ended,"counts":kinds,"pressure_samples":pressure_samples,
        "pressure_mean":(pressure_samples>0).then(||pressure_sum/pressure_samples as f64)}));
    if !ended{eprintln!("Recording was interrupted; summary covers valid rows only.");}
    Ok(())
}

#[cfg(test)]
mod tests{
    use super::*;
    #[tokio::test]async fn record_round_trip_and_no_overwrite(){
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("test.jsonl");
        let mut w=Writer::new(&path,"krita.exe").unwrap();
        let a=Action{seq:1,at_ms:10,generation:1,kind:"click_candidate".into(),bounds:Some([0,0,1,1]),duration_ms:10,samples:1,distance_px:0,pen:None};
        w.event(&a).unwrap();w.gap(2).unwrap();w.finish().unwrap();drop(w);
        assert!(Writer::new(&path,"x").is_err());replay(&path,false,1.0).await.unwrap();
        assert_eq!(summarize([&a])["counts"]["click_candidate"],1);
    }
    #[tokio::test]async fn rejects_oversize(){
        let d=tempfile::tempdir().unwrap();let p=d.path().join("bad");std::fs::write(&p,vec![b'x';MAX_LINE+1]).unwrap();
        assert!(replay(&p,false,1.0).await.is_err());
    }
}
