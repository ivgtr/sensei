//! In-memory, foreground-only Windows action sampling. No key text, no screenshots
//! during monitoring, no pressure or claims of actual canvas changes.
use anyhow::{bail, Result};
use serde::Serialize;
use std::{collections::VecDeque, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}, thread, time::{Duration, Instant}};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target { pub hwnd: usize, pub pid: u32, pub rect: [i32;4] }

#[derive(Clone, Debug, Serialize)]
pub struct Action {
    pub seq: u64,
    pub at_ms: u64,
    pub kind: &'static str,
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
}
impl State {
    fn add(&mut self, mut a: Action) {
        self.seq += 1;
        a.seq = self.seq;
        self.events.push_back(a);
        if self.events.len() > 256 { self.events.pop_front(); }
        self.last_action = Some(Instant::now());
    }
    fn target(&mut self, target: Target) {
        if self.target.map(|t| (t.hwnd,t.pid)) != Some((target.hwnd,target.pid)) {
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
    fn finish(self, ms:u64) -> Action { Action{seq:0,at_ms:ms,kind:if self.distance>=5 {"pointer_drag_candidate"} else {"click_candidate"},bounds:Some(self.bounds),duration_ms:ms.saturating_sub(self.start),samples:self.samples,distance_px:self.distance} }
}
fn simple(kind:&'static str, ms:u64) -> Action { Action{seq:0,at_ms:ms,kind,bounds:None,duration_ms:0,samples:0,distance_px:0} }

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
        Snapshot{actions:s.events.iter().cloned().collect(),target:s.target,active:s.active,drawing:s.drawing,seq:s.seq,generation:s.generation,last_action:s.last_action}
    }
    pub fn image(&self, target:Target) -> Result<String> { platform::capture(target,&self.app) }
}
impl Drop for Observer {
    fn drop(&mut self) { self.stop.store(true,Ordering::Relaxed); if let Some(t)=self.worker.take(){let _=t.join();} }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{io::Cursor, path::Path};
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
            let mut buf=vec![0u16;32768]; let mut len=buf.len() as u32;
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
        let started=Instant::now(); let mut stroke:Option<Stroke>=None;
        let mut previous:Option<Target>=None;
        let mut was_down=down(VK_LBUTTON as i32); let mut was_undo=false; let mut was_redo=false;
        while !stop.load(Ordering::Relaxed) {
            let ms=started.elapsed().as_millis() as u64;
            let target=identity(unsafe{GetForegroundWindow()},app);
            let pressed=down(VK_LBUTTON as i32);
            // Only fixed editing shortcut candidates, never characters or arbitrary keys.
            let undo=target.is_some() && down(VK_CONTROL as i32)&&down(b'Z' as i32);
            let redo=target.is_some() && down(VK_CONTROL as i32)&&down(b'Y' as i32);
            let mut s=state.lock().unwrap();
            if let Some(t)=target {
                s.target(t);
                if previous!=Some(t) {stroke=None;}
                let mut p:POINT=unsafe{std::mem::zeroed()};
                let inside=unsafe{GetCursorPos(&mut p)!=0 && GetAncestor(WindowFromPoint(p),GA_ROOT) as usize==t.hwnd};
                let point=[p.x-t.rect[0],p.y-t.rect[1]];
                if pressed && !was_down && inside && previous==Some(t) {stroke=Some(Stroke::new(point,ms));}
                if pressed && inside {if let Some(ref mut st)=stroke {st.sample(point);}}
                if !pressed && was_down {
                    if let Some(st)=stroke.take() {if inside{s.add(st.finish(ms));}}
                }
                if pressed && !inside {stroke=None;}
                if previous==Some(t) {
                    if undo && !was_undo {s.add(simple("ctrl_z_candidate",ms));}
                    if redo && !was_redo {s.add(simple("ctrl_y_candidate",ms));}
                }
                // Suppress automatic feedback even when a pen is not recognized as a stroke.
                s.drawing=pressed;
            } else {s.active=false;s.drawing=false;stroke=None;}
            previous=target;was_down=pressed;was_undo=undo;was_redo=redo;
            drop(s);thread::sleep(Duration::from_millis(25));
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
    #[test] fn window_change_resets_history() {
        let mut s=State::default();s.target(Target{hwnd:1,pid:1,rect:[0,0,10,10]});
        s.add(simple("ctrl_z_candidate",1));s.target(Target{hwnd:2,pid:1,rect:[0,0,10,10]});
        assert!(s.events.is_empty());assert_eq!(s.generation,2);assert_eq!(s.seq,1);
    }
}
