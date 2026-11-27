//! In-memory, foreground-only Windows action sampling. No key text, no screenshots
//! during monitoring. Native pen input is optional and distinct from mouse candidates.
use anyhow::{bail, Result};
use serde::{Serialize,Deserialize};
use crate::pen::PenStroke;
#[cfg(windows)] use crate::pen;
use std::{collections::VecDeque, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}, thread, time::{Duration, Instant}};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target { pub hwnd: usize, pub pid: u32, pub rect: [i32;4] }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Action {
    pub seq: u64,
    pub at_ms: u64,
    pub kind: String,
    #[serde(default)] pub generation: u64,
    #[serde(default,skip_serializing_if="Option::is_none")] pub pen: Option<PenStroke>,
    pub bounds: Option<[i32;4]>,
    pub duration_ms: u64,
    pub samples: u32,
    pub distance_px: u64,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub actions: Vec<Action>,
    pub target: Option<Target>,
    pub active: bool,
    pub drawing: bool,
    pub seq: u64,
    pub generation: u64,
    pub last_action: Option<Instant>,
    pub native_registered: bool,
    pub native_reports: u64,
    pub native_strokes: u64,
}
#[derive(Default)]
struct State {
    events: VecDeque<Action>,
    target: Option<Target>,
    active: bool,
    drawing: bool,
    seq: u64,
    generation: u64,
    last_action: Option<Instant>,
    native_registered:bool,
    native_reports:u64,
    native_strokes:u64,
}
impl State {
    fn add(&mut self, mut a: Action) {
        self.seq += 1;
        a.seq = self.seq;
        a.generation=self.generation;
        if a.pen.is_some(){self.native_strokes+=1;}
        self.events.push_back(a);
        if self.events.len() > 256 { self.events.pop_front(); }
        self.last_action = Some(Instant::now());
    }
    fn target(&mut self, target: Target) {
        if self.target != Some(target) {
            self.events.clear();
            self.generation += 1;
            self.last_action = None;
        }
        self.target = Some(target);
        self.active = true;
    }
}

struct Stroke { start: u64, bounds: [i32;4], last: [i32;2], samples:u32, distance:u64 }
impl Stroke {
    fn new(p: [i32;2], ms:u64) -> Self { Self { start:ms, bounds:[p[0],p[1],p[0],p[1]],last:p,samples:1,distance:0 } }
    fn sample(&mut self, p:[i32;2]) {
        self.bounds=[self.bounds[0].min(p[0]),self.bounds[1].min(p[1]),self.bounds[2].max(p[0]),self.bounds[3].max(p[1])];
        self.distance += ((p[0] as f64-self.last[0] as f64).hypot(p[1] as f64-self.last[1] as f64)) as u64;
        self.samples += 1; self.last=p;
    }
    fn finish(self, ms:u64) -> Action { Action{seq:0,at_ms:ms,kind:if self.distance>=5 {"pointer_drag_candidate".into()} else {"click_candidate".into()},generation:0,pen:None,bounds:Some(self.bounds),duration_ms:ms.saturating_sub(self.start),samples:self.samples,distance_px:self.distance} }
}
fn simple(kind:&'static str, ms:u64) -> Action { Action{seq:0,at_ms:ms,kind:kind.into(),generation:0,pen:None,bounds:None,duration_ms:0,samples:0,distance_px:0} }

pub struct Observer { state: Arc<Mutex<State>>, stop:Arc<AtomicBool>, worker:Option<thread::JoinHandle<()>>, app:String }
impl Observer {
    pub fn start(app:String) -> Self {
        let state=Arc::new(Mutex::new(State::default()));
        let stop=Arc::new(AtomicBool::new(false));
        let (s,q,a)=(state.clone(),stop.clone(),app.clone());
        let worker=thread::spawn(move || platform::observe(s,q,&a));
        Self{state,stop,worker:Some(worker),app}
    }
    pub fn snapshot(&self) -> Snapshot {
        let s=self.state.lock().unwrap();
        Snapshot{actions:s.events.iter().cloned().collect(),target:s.target,active:s.active,drawing:s.drawing,seq:s.seq,generation:s.generation,last_action:s.last_action,native_registered:s.native_registered,native_reports:s.native_reports,native_strokes:s.native_strokes}
    }
    pub fn image(&self, target:Target) -> Result<String> { platform::capture(target,&self.app) }
}
impl Drop for Observer {
    fn drop(&mut self) { self.stop.store(true,Ordering::Relaxed); if let Some(t)=self.worker.take(){let _=t.join();} }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{io::Cursor, path::Path,collections::{HashSet,HashMap}};
    use crate::pen::native::Pump;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HWND, POINT, RECT},
        System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION},
        UI::{Input::KeyboardAndMouse::{GetAsyncKeyState,VK_LBUTTON,VK_CONTROL},
            WindowsAndMessaging::{GetForegroundWindow,GetWindowThreadProcessId,GetWindowRect,GetCursorPos,WindowFromPoint,GetAncestor,GA_ROOT,IsWindow,IsIconic},
            HiDpi::{SetThreadDpiAwarenessContext,DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2}}
    };
    fn dpi() { unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2); } }
    fn identity(hwnd: HWND, app:&str) -> Option<Target> {
        unsafe {
            if hwnd.is_null() || IsWindow(hwnd)==0 || IsIconic(hwnd)!=0 { return None; }
            let mut pid=0;
            GetWindowThreadProcessId(hwnd,&mut pid);
            let h=OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,0,pid);
            if h.is_null() {return None;}
            let mut buf=[0u16;32768]; let mut len=buf.len() as u32;
            let ok=QueryFullProcessImageNameW(h,0,buf.as_mut_ptr(),&mut len);
            CloseHandle(h);
            if ok==0 {return None;}
            let path=String::from_utf16_lossy(&buf[..len as usize]);
            if !Path::new(&path).file_name()?.to_str()?.eq_ignore_ascii_case(app) {return None;}
            let mut r:RECT=std::mem::zeroed();
            if GetWindowRect(hwnd,&mut r)==0 || r.right<=r.left || r.bottom<=r.top {return None;}
            Some(Target{hwnd:hwnd as usize,pid,rect:[r.left,r.top,r.right,r.bottom]})
        }
    }
    fn down(vk:i32)->bool { unsafe {GetAsyncKeyState(vk)<0} }
    pub(super) fn observe(state:Arc<Mutex<State>>,stop:Arc<AtomicBool>,app:&str) {
        dpi();
        let started=Instant::now();
        let mut stroke:Option<Stroke>=None;
        let mut pending:VecDeque<(Action,u64)>=VecDeque::new();
        let mut pump=match Pump::new(){Ok(p)=>Some(p),Err(e)=>{eprintln!("Native pen unavailable: {e}; using pointer fallback");None}};
        let mut native:Option<pen::Accumulator>=None;
        let mut armed=HashSet::new();
        let mut slots=HashMap::<usize,u32>::new();let mut next_slot=0u32;
        let mut last_native=None::<u64>;
        let mut previous:Option<Target>=None;
        let mut was_down=down(VK_LBUTTON as i32); let mut was_undo=false; let mut was_redo=false;
        state.lock().unwrap().native_registered=pump.is_some();
        while !stop.load(Ordering::Relaxed) {
            let ms=started.elapsed().as_millis() as u64;
            let target=identity(unsafe{GetForegroundWindow()},app);
            let mut p:POINT=unsafe{std::mem::zeroed()};
            let inside=target.is_some_and(|t|unsafe{GetCursorPos(&mut p)!=0 && GetAncestor(WindowFromPoint(p),GA_ROOT) as usize==t.hwnd});
            let stable=target.is_some() && previous==target;
            let (reports,reset)=pump.as_mut().map(|p|p.poll(stable&&inside)).unwrap_or_default();
            let still_target=target.is_some_and(|t|unsafe{GetForegroundWindow() as usize==t.hwnd});
            let pressed=down(VK_LBUTTON as i32);
            let undo=target.is_some() && down(VK_CONTROL as i32)&&down(b'Z' as i32);
            let redo=target.is_some() && down(VK_CONTROL as i32)&&down(b'Y' as i32);
            let mut s=state.lock().unwrap();
            if reset{slots.clear();}
            if !stable || !still_target || reset || !inside {
                stroke=None;pending.clear();native=None;armed.clear();last_native=None;
            }
            if let Some(t)=target.filter(|_|still_target) {
                s.target(t);
                let point=[p.x-t.rect[0],p.y-t.rect[1]];
                if stable&&inside&&!reset {
                    for report in reports{
                        s.native_reports=s.native_reports.saturating_add(1);
                        if !report.tip{
                            armed.insert(report.device);
                            if native.as_ref().is_some_and(|a|a.device==report.device){
                                if let Some(a)=native.take(){
                                    let duration=ms.saturating_sub(a.start_ms);
                                    let slot=*slots.entry(a.device).or_insert_with(||{next_slot=next_slot.saturating_add(1);next_slot});
                                    let mut pen=a.finish();pen.device_slot=Some(slot);
                                    s.add(Action{seq:0,at_ms:ms,generation:0,kind:"pen_stroke".into(),bounds:None,
                                        duration_ms:duration,samples:pen.samples,distance_px:0,pen:Some(pen)});
                                    last_native=Some(ms);stroke=None;pending.clear();
                                }
                            }
                        }else if armed.contains(&report.device){
                            if native.is_none(){native=Some(pen::Accumulator::new(report.device,ms));}
                            if let Some(a)=native.as_mut().filter(|a|a.device==report.device){
                                a.sample(report,ms);last_native=Some(ms);stroke=None;pending.clear();
                            }
                        }
                    }
                    // Incomplete strokes are discarded, not synthesized as valid contacts.
                    if native.as_ref().is_some_and(|a|ms.saturating_sub(a.last_ms)>750){
                        if let Some(a)=native.take(){armed.remove(&a.device);}
                    }
                    let native_recent=native.is_some() || last_native.is_some_and(|last|ms.saturating_sub(last)<250);
                    if !native_recent{
                        if pressed&&!was_down{stroke=Some(Stroke::new(point,ms));}
                        if pressed{if let Some(st)=stroke.as_mut(){st.sample(point);}}
                        if !pressed&&was_down{if let Some(st)=stroke.take(){pending.push_back((st.finish(ms),ms));}}
                        while pending.front().is_some_and(|(_,at)|ms.saturating_sub(*at)>=150){
                            if let Some((a,_))=pending.pop_front(){s.add(a);}
                        }
                    }else{stroke=None;pending.clear();}
                    if undo&&!was_undo{s.add(simple("ctrl_z_candidate",ms));}
                    if redo&&!was_redo{s.add(simple("ctrl_y_candidate",ms));}
                }
                s.drawing=pressed||native.is_some();
            }else{s.active=false;s.drawing=false;stroke=None;pending.clear();native=None;armed.clear();last_native=None;}
            previous=if still_target{target}else{None};
            was_down=pressed;was_undo=undo;was_redo=redo;
            drop(s);
            thread::sleep(Duration::from_millis(8));
        }
    }
    pub(super) fn capture(target:Target,app:&str)->Result<String> {
        dpi();
        let hwnd=target.hwnd as HWND;
        let before=identity(hwnd,app).ok_or_else(||anyhow::anyhow!("Target window no longer available"))?;
        if before.pid!=target.pid {bail!("Target changed");}
        let window=xcap::Window::all()?.into_iter().find(|w|w.id()==target.hwnd as u32 && w.process_id()==target.pid && !w.is_minimized())
            .ok_or_else(||anyhow::anyhow!("No capturable target window"))?;
        let image=window.capture_image()?;
        if identity(hwnd,app)!=Some(before) {bail!("Window changed during capture");}
        if image.width()==0 || image.height()==0 {bail!("Empty window image");}
        let image=image::DynamicImage::ImageRgba8(image).thumbnail(1280,1280);
        let mut out=Cursor::new(Vec::new());image.write_to(&mut out,image::ImageFormat::Png)?;
        if out.get_ref().len()>8*1024*1024 {bail!("Image too large");}
        Ok(STANDARD.encode(out.into_inner()))
    }
}
#[cfg(not(windows))]
mod platform {
    use super::*;
    pub(super) fn observe(_:Arc<Mutex<State>>,_:Arc<AtomicBool>,_:&str){}
    pub(super) fn capture(_:Target,_:&str)->Result<String>{bail!("Windows only")}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn aggregates_drag() {
        let mut s=Stroke::new([1,2],10);s.sample([4,6]);s.sample([0,9]);
        let a=s.finish(70);assert_eq!(a.bounds,Some([0,2,4,9]));assert_eq!(a.duration_ms,60);
        assert_eq!(a.kind,"pointer_drag_candidate");assert_eq!(a.samples,3);
    }
    #[test] fn action_serializes_generation() {
        let mut s=State::default();s.target(Target{hwnd:1,pid:1,rect:[0,0,10,10]});s.add(simple("ctrl_z_candidate",1));
        let a:Action=serde_json::from_value(serde_json::to_value(&s.events[0]).unwrap()).unwrap();assert_eq!(a.generation,1);
    }
    #[test] fn geometry_change_resets_history(){
        let mut s=State::default();s.target(Target{hwnd:1,pid:1,rect:[0,0,10,10]});s.add(simple("ctrl_z_candidate",1));
        s.target(Target{hwnd:1,pid:1,rect:[0,0,20,20]});assert!(s.events.is_empty());assert_eq!(s.generation,2);
    }
    #[test] fn window_change_resets_history() {
        let mut s=State::default();s.target(Target{hwnd:1,pid:1,rect:[0,0,10,10]});
        s.add(simple("ctrl_z_candidate",1));s.target(Target{hwnd:2,pid:1,rect:[0,0,10,10]});
        assert!(s.events.is_empty());assert_eq!(s.generation,2);assert_eq!(s.seq,1);
    }
}
