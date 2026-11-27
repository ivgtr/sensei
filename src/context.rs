//! Deterministic, local work-context extraction. Signals are observations, not
//! diagnoses or inferred drawing stages. Never compares different coordinate units.
use crate::desktop::{Action,Snapshot};
use serde::Serialize;
use std::{collections::BTreeMap,time::Instant};

pub const WINDOW_MS:u64=30_000;
pub const BASELINE_MS:u64=30_000;
const MIN_STROKES:usize=6;

#[derive(Clone,Debug,Serialize)]
pub struct Signal{
    pub kind:&'static str,
    pub source:String,
    pub evidence_count:usize,
    pub last_seq:u64,
    pub evidence_seq:Vec<u64>,
    pub detail:serde_json::Value,
}
#[derive(Clone,Debug,Serialize)]
pub struct Metrics{
    pub source:String,
    pub count:usize,
    pub length_unit:&'static str,
    pub length_median:Option<f64>,
    pub duration_median_ms:Option<f64>,
    pub speed_median_per_s:Option<f64>,
    pub pressure_median:Option<f64>,
    pub pressure_strokes:usize,
    pub short_count:usize,
    pub long_count:usize,
}
#[derive(Clone,Debug,Serialize)]
pub struct WorkContext{
    pub generation:u64,
    pub as_of_ms:u64,
    pub recent_window_ms:u64,
    pub baseline_window_ms:u64,
    pub recent_count:usize,
    pub baseline_count:usize,
    pub undo_candidates:usize,
    pub redo_candidates:usize,
    pub recent:Vec<Metrics>,
    pub baseline:Vec<Metrics>,
    pub signals:Vec<Signal>,
    pub observed_span_ms:u64,
    pub potentially_truncated:bool,
    pub limitations:&'static str,
}
#[derive(Clone,Copy)]
struct Stroke<'a>{
    a:&'a Action,
    length:Option<f64>,
    bounds:Option<[f64;4]>,
    pressure:Option<f64>,
}
fn finite(v:f64)->Option<f64>{v.is_finite().then_some(v)}
fn pressure(v:f64)->Option<f64>{(v.is_finite()&&(0.0..=1.0).contains(&v)).then_some(v)}
fn valid_bounds(b:[f64;4])->Option<[f64;4]>{
    (b.iter().all(|v|v.is_finite())&&b[2]>=b[0]&&b[3]>=b[1]).then_some(b)
}
fn stroke(a:&Action)->Option<(String,Stroke<'_>)>{
    if a.kind=="pen_stroke"{
        let p=a.pen.as_ref()?;
        let bounds=p.tablet_bounds.and_then(valid_bounds).filter(|b|b.iter().all(|v|(0.0..=1.0).contains(v)));
        // Old recordings lack a per-device slot. Keep their descriptive stats
        // but never infer spatial repetition or temporal changes across devices.
        let source=p.device_slot.map(|id|format!("tablet:{id}")).unwrap_or_else(||"tablet:unknown".into());
        let length=bounds.and_then(|_|finite(p.path_length_normalized)).filter(|v|*v>=0.0&&p.samples>=2);
        let pressure=p.pressure_mean.filter(|_|p.pressure_samples>0&&p.pressure_samples<=p.samples).and_then(pressure);
        Some((source,Stroke{a,length,bounds,pressure}))
    }else if a.kind=="pointer_drag_candidate"{
        Some(("pointer_window_px".into(),Stroke{a,length:Some(a.distance_px as f64),bounds:a.bounds.and_then(|b|valid_bounds(b.map(f64::from))),pressure:None}))
    }else{None}
}
fn median(mut values:Vec<f64>)->Option<f64>{
    values.retain(|v|v.is_finite());
    values.sort_by(f64::total_cmp);
    let mid=values.len()/2;
    if values.is_empty(){None}else if values.len()%2==1{Some(values[mid])}else{Some(values[mid-1]/2.0+values[mid]/2.0)}
}
fn thresholds(source:&str)->(f64,f64,f64){
    if source=="pointer_window_px"{(30.0,150.0,48.0)}else{(0.035,0.15,0.07)}
}
fn metrics(source:&str,strokes:&[Stroke<'_>])->Metrics{
    let (short,long,_)=thresholds(source);
    Metrics{source:source.into(),count:strokes.len(),length_unit:if source=="pointer_window_px"{"window_px"}else{"normalized_tablet"},
        length_median:median(strokes.iter().filter_map(|s|s.length).collect()),
        duration_median_ms:median(strokes.iter().map(|s|s.a.duration_ms as f64).collect()),
        speed_median_per_s:median(strokes.iter().filter(|s|s.a.duration_ms>=40&&s.a.samples>=2)
            .filter_map(|s|s.length.map(|l|l*1000.0/s.a.duration_ms as f64)).collect()),
        pressure_median:median(strokes.iter().filter_map(|s|s.pressure).collect()),
        pressure_strokes:strokes.iter().filter(|s|s.pressure.is_some()).count(),
        short_count:strokes.iter().filter(|s|s.length.is_some_and(|l|l<=short)).count(),
        long_count:strokes.iter().filter(|s|s.length.is_some_and(|l|l>=long)).count()}
}
fn signal(kind:&'static str,source:&str,actions:impl IntoIterator<Item=u64>,detail:serde_json::Value)->Signal{
    let mut seq:Vec<_>=actions.into_iter().collect();seq.sort_unstable();seq.dedup();
    Signal{kind,source:source.into(),evidence_count:seq.len(),last_seq:seq.last().copied().unwrap_or(0),evidence_seq:seq,detail}
}
fn localized(source:&str,strokes:&[Stroke<'_>])->Option<Signal>{
    let (_,_,radius)=thresholds(source);
    let small:Vec<_>=strokes.iter().filter_map(|s|{
        let b=s.bounds?;
        if b[2]-b[0]>radius*2.0||b[3]-b[1]>radius*2.0{return None;}
        Some((s,[(b[0]+b[2])/2.0,(b[1]+b[3])/2.0]))
    }).collect();
    let mut best=Vec::new();let mut center=[0.0;2];
    for (_,p) in &small{
        let hits:Vec<_>=small.iter().filter(|(_,q)|(q[0]-p[0]).hypot(q[1]-p[1])<=radius).map(|(s,_)|s.a.seq).collect();
        if hits.len()>best.len(){best=hits;center=*p;}
    }
    if best.len()<MIN_STROKES||best.len()*2<strokes.len(){return None;}
    Some(signal("localized_activity",source,best,serde_json::json!({"center":center,"radius":radius,"meaning":"nearby stroke/drag centers, not confirmed redraws"})))
}
fn ratio_shift(a:Option<f64>,b:Option<f64>,floor:f64)->Option<(f64,f64)>{
    let (a,b)=(a?,b?);
    if a<floor||b<floor{return None;}
    (a/b>=1.8||b/a>=1.8).then_some((a,b))
}
pub fn analyze(actions:&[Action],generation:u64,now_ms:u64)->WorkContext{
    let mut all:Vec<_>=actions.iter().filter(|a|a.generation==generation&&a.at_ms<=now_ms&&now_ms-a.at_ms<WINDOW_MS+BASELINE_MS).collect();
    all.sort_by_key(|a|(a.at_ms,a.seq));
    let cutoff=now_ms.saturating_sub(WINDOW_MS);
    let recent:Vec<_>=all.iter().copied().filter(|a|a.at_ms>=cutoff).collect();
    let baseline:Vec<_>=all.iter().copied().filter(|a|a.at_ms<cutoff).collect();
    let r=groups(&recent);let b=groups(&baseline);
    let mut signals=Vec::new();
    let undo:Vec<_>=recent.iter().copied().filter(|a|a.kind=="ctrl_z_candidate"&&now_ms-a.at_ms<=15_000).collect();
    if undo.len()>=3{signals.push(signal("undo_shortcut_burst","shortcut",undo.iter().map(|a|a.seq),serde_json::json!({"window_ms":15000,"meaning":"Ctrl+Z candidates, not confirmed undo"})));}
    let recent_metrics:Vec<_>=r.iter().map(|(k,v)|metrics(k,v)).collect();
    let baseline_metrics:Vec<_>=b.iter().map(|(k,v)|metrics(k,v)).collect();
    for (source,strokes) in &r{
        if source=="tablet:unknown"{continue;}
        let m=metrics(source,strokes);
        let (short,long,_)=thresholds(source);
        if let Some(s)=localized(source,strokes){signals.push(s);}
        let small:Vec<_>=strokes.iter().filter(|s|s.length.is_some_and(|v|v<=short)).collect();
        if small.len()>=MIN_STROKES && small.len()*4>=strokes.len()*3 {
            signals.push(signal("short_stroke_cluster",source,small.iter().map(|s|s.a.seq),serde_json::json!({"short_threshold":short,"length_unit":m.length_unit})));
        }
        let Some(prev)=b.get(source) else{continue;};
        if strokes.len()<MIN_STROKES||prev.len()<MIN_STROKES{continue;}
        let old=metrics(source,prev);
        if m.long_count>=MIN_STROKES && m.long_count*2>=m.count && old.long_count*4<=old.count{
            signals.push(signal("long_strokes_increase",source,strokes.iter().filter(|s|s.length.is_some_and(|v|v>=long)).map(|s|s.a.seq),serde_json::json!({"recent_long":m.long_count,"baseline_long":old.long_count,"threshold":long})));
        }
        let (lf,sf)=if source=="pointer_window_px"{(3.0,10.0)}else{(0.005,0.01)};
        let eligible_length=|s:&&Stroke<'_>|s.length.is_some();
        if strokes.iter().filter(eligible_length).count()>=MIN_STROKES&&prev.iter().filter(eligible_length).count()>=MIN_STROKES{
            if let Some((a,z))=ratio_shift(m.length_median,old.length_median,lf){
                signals.push(signal("stroke_length_shift",source,strokes.iter().filter(eligible_length).map(|s|s.a.seq),serde_json::json!({"recent_median":a,"baseline_median":z,"unit":m.length_unit})));
            }
        }
        let speed_ok=|s:&&Stroke<'_>|s.length.is_some()&&s.a.duration_ms>=40&&s.a.samples>=2;
        if strokes.iter().filter(speed_ok).count()>=MIN_STROKES&&prev.iter().filter(speed_ok).count()>=MIN_STROKES{
            if let Some((a,z))=ratio_shift(m.speed_median_per_s,old.speed_median_per_s,sf){
                signals.push(signal("stroke_speed_shift",source,strokes.iter().filter(speed_ok).map(|s|s.a.seq),serde_json::json!({"recent_median":a,"baseline_median":z,"unit":format!("{}/s",m.length_unit)})));
            }
        }
        if m.pressure_strokes>=MIN_STROKES&&old.pressure_strokes>=MIN_STROKES{
            if let (Some(a),Some(z))=(m.pressure_median,old.pressure_median){
                if (a-z).abs()>=0.2{
                    signals.push(signal("pressure_shift",source,strokes.iter().filter(|s|s.pressure.is_some()).map(|s|s.a.seq),serde_json::json!({"recent_median":a,"baseline_median":z})));
                }
            }
        }
    }
    WorkContext{generation,as_of_ms:now_ms,recent_window_ms:WINDOW_MS,baseline_window_ms:BASELINE_MS,
        recent_count:recent.len(),baseline_count:baseline.len(),undo_candidates:recent.iter().filter(|a|a.kind=="ctrl_z_candidate").count(),
        redo_candidates:recent.iter().filter(|a|a.kind=="ctrl_y_candidate").count(),recent:recent_metrics,baseline:baseline_metrics,signals,
        observed_span_ms:all.first().map_or(0,|a|now_ms-a.at_ms),
        potentially_truncated:actions.len()>=256||all.windows(2).any(|w|w[1].seq.abs_diff(w[0].seq)>1),
        limitations:"Heuristic signals, not artistic diagnoses. Tablet logical coordinates and window-relative pixels are separate; neither is canvas coordinates. Viewport/tool/document changes within one window may be unobserved."}
}
fn groups<'a>(events:&[&'a Action])->BTreeMap<String,Vec<Stroke<'a>>>{
    let mut out=BTreeMap::<String,Vec<Stroke<'a>>>::new();
    for &a in events{if let Some((source,s))=stroke(a){out.entry(source).or_default().push(s);}}
    out
}
pub fn snapshot(s:&Snapshot,now:Instant)->WorkContext{
    let end=s.actions.iter().map(|a|a.at_ms).max().unwrap_or(0);
    let age=s.last_action.map_or(0,|t|now.saturating_duration_since(t).as_millis().min(u64::MAX as u128) as u64);
    analyze(&s.actions,s.generation,end.saturating_add(age))
}
/// Reasons for a paid review. Only fresh evidence may trigger a new review.
/// Cooldown, foreground, idle and budget checks are applied by the caller.
pub fn review_reason(c:&WorkContext,last_seq:u64)->Option<&'static str>{
    for kind in ["undo_shortcut_burst","localized_activity","pressure_shift","stroke_length_shift","stroke_speed_shift","long_strokes_increase","short_stroke_cluster"]{
        let minimum=if kind=="undo_shortcut_burst"{3}else{4};
        if c.signals.iter().any(|s|s.kind==kind&&s.evidence_seq.iter().filter(|seq|**seq>last_seq).count()>=minimum){return Some(kind);}
    }
    None
}

#[cfg(test)]
mod tests{
    use super::*;
    fn drag(seq:u64,at_ms:u64,x:i32,d:u64)->Action{
        Action{seq,at_ms,generation:1,kind:"pointer_drag_candidate".into(),bounds:Some([x,10,x+10,20]),duration_ms:100,samples:10,distance_px:d,pen:None}
    }
    #[test]fn localized_is_not_a_diagnosis_and_requires_fresh_evidence(){
        let a:Vec<_>=(1..=8).map(|i|drag(i,i*1000,10,20)).collect();
        let c=analyze(&a,1,9000);
        assert!(c.signals.iter().any(|s|s.kind=="localized_activity"));
        assert_eq!(review_reason(&c,0),Some("localized_activity"));
        assert_eq!(review_reason(&c,6),None);
        assert!(analyze(&a,2,9000).signals.is_empty());
        assert!(analyze(&a,1,90000).signals.is_empty());
    }
    #[test]fn detects_length_change_only_with_baseline(){
        let mut a:Vec<_>=(1..=8).map(|i|drag(i,i*1000,100*i as i32,20)).collect();
        a.extend((9..=16).map(|i|drag(i,30000+i*1000,100*i as i32,200)));
        let c=analyze(&a,1,50000);
        assert!(c.signals.iter().any(|s|s.kind=="long_strokes_increase"));
        assert!(c.signals.iter().any(|s|s.kind=="stroke_length_shift"));
        assert!(!analyze(&a[8..],1,50000).signals.iter().any(|s|s.kind=="stroke_length_shift"));
    }
    #[test]fn undo_burst_and_no_cross_generation(){
        let mut a:Vec<_>=(1..=3).map(|i|drag(i,i*1000,0,0)).collect();
        for e in &mut a{e.kind="ctrl_z_candidate".into();e.bounds=None;}
        assert_eq!(review_reason(&analyze(&a,1,5000),0),Some("undo_shortcut_burst"));
        a[0].generation=2;
        assert!(review_reason(&analyze(&a,1,5000),0).is_none());
    }
    #[test]fn pressure_changes_require_same_device_and_valid_samples(){
        let mut a=Vec::new();
        for i in 1..=12{
            let mut p=crate::pen::Accumulator::new(1,0);
            let value=if i<=6{0.2}else{0.7};
            for t in [0,10]{p.sample(crate::pen::Sample{device:1,tip:true,xy:Some([0.2,0.3]),pressure:Some(value),tilt_raw:[None;2]},t);}
            let mut pen=p.finish();pen.device_slot=Some(1);
            let mut e=drag(i,if i<=6{i*1000}else{30_000+i*1000},10,0);e.kind="pen_stroke".into();e.pen=Some(pen);a.push(e);
        }
        assert!(analyze(&a,1,45000).signals.iter().any(|s|s.kind=="pressure_shift"));
        for e in &mut a[6..]{e.pen.as_mut().unwrap().device_slot=Some(2);}
        assert!(!analyze(&a,1,45000).signals.iter().any(|s|s.kind=="pressure_shift"));
    }
    #[test]fn pen_devices_and_pointer_are_not_combined(){
        let mut a=Vec::new();
        for i in 1..=12{
            let mut p=crate::pen::Accumulator::new(1,0);
            for t in [0,10]{p.sample(crate::pen::Sample{device:1,tip:true,xy:Some([0.2,0.3]),pressure:Some(0.4),tilt_raw:[None;2]},t);}
            let mut pen=p.finish();pen.device_slot=Some((i%3) as u32);
            let mut e=drag(i,i*1000,10,0);e.kind="pen_stroke".into();e.pen=Some(pen);a.push(e);
        }
        let c=analyze(&a,1,13000);
        assert_eq!(c.recent.len(),3);assert!(c.signals.is_empty());
    }
}
