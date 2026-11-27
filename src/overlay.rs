//! Transient click-through, no-activate pointer marker. Never sends input,
//! modifies the canvas, or changes the foreground window.
use crate::desktop::{Observer,Target};
use anyhow::{bail,Result};
use std::{sync::{Arc,atomic::{AtomicBool,Ordering}},thread,time::{Duration,Instant}};

pub fn map_point(rect:[i32;4],point:[f64;2])->Option<[i32;2]>{
    if !point.iter().all(|v|v.is_finite()&&(0.0..=1.0).contains(v)){return None;}
    let w=i64::from(rect[2])-i64::from(rect[0]);let h=i64::from(rect[3])-i64::from(rect[1]);
    if w<1||h<1||w>100_000||h>100_000{return None;}
    Some([rect[0]+(point[0]*(w-1) as f64).round() as i32,rect[1]+(point[1]*(h-1) as f64).round() as i32])
}
#[derive(Clone,Copy)]
pub struct Request{pub target:Target,pub client_rect:[i32;4],pub point:[f64;2],pub generation:u64,pub seq:u64}
pub struct Marker{stop:Arc<AtomicBool>,worker:Option<thread::JoinHandle<()>>}
impl Marker{
    pub fn show(obs:Arc<Observer>,req:Request)->Result<Self>{
        if map_point(req.client_rect,req.point).is_none(){bail!("Invalid pointer coordinates");}
        let stop=Arc::new(AtomicBool::new(false));let flag=stop.clone();
        let worker=thread::spawn(move||{if let Err(e)=platform::show(&obs,req,&flag){eprintln!("Sensei pointer: {e}");}});
        Ok(Self{stop,worker:Some(worker)})
    }
}
impl Drop for Marker{fn drop(&mut self){self.stop.store(true,Ordering::Relaxed);if let Some(w)=self.worker.take(){let _=w.join();}}}
#[cfg(windows)]
mod platform{
    use super::*;
    use windows_sys::Win32::{
        Foundation::{HWND,LPARAM,LRESULT,WPARAM},
        Graphics::Gdi::{CreateSolidBrush,DeleteObject,CreateEllipticRgn,CombineRgn,RGN_DIFF,SetWindowRgn},
        System::LibraryLoader::GetModuleHandleW,
        UI::{HiDpi::{SetThreadDpiAwarenessContext,DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},WindowsAndMessaging::*},
    };
    unsafe extern "system" fn proc(hwnd:HWND,msg:u32,w:WPARAM,l:LPARAM)->LRESULT{
        if msg==WM_NCHITTEST{return HTTRANSPARENT as LRESULT;}
        if msg==WM_MOUSEACTIVATE{return MA_NOACTIVATE as LRESULT;}
        unsafe{DefWindowProcW(hwnd,msg,w,l)}
    }
    fn fresh(obs:&Observer,req:Request)->bool{
        obs.marker_fresh(req.target,req.generation,req.seq)&&obs.client_rect(req.target)==Some(req.client_rect)
    }
    pub(super) fn show(obs:&Observer,req:Request,stop:&AtomicBool)->Result<()>{
        unsafe{
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            if !fresh(obs,req){bail!("Target changed or not foreground");}
            let [x,y]=map_point(req.client_rect,req.point).unwrap();
            let class:Vec<u16>="SenseiPointerV06\0".encode_utf16().collect();
            let inst=GetModuleHandleW(std::ptr::null());
            let brush=CreateSolidBrush(0x003060ff); // COLORREF red/orange
            if brush.is_null(){bail!("Cannot create marker brush");}
            let mut wc:WNDCLASSW=std::mem::zeroed();
            wc.lpfnWndProc=Some(proc);wc.hInstance=inst;wc.hbrBackground=brush;wc.lpszClassName=class.as_ptr();
            if RegisterClassW(&wc)==0{DeleteObject(brush);bail!("Cannot register marker window");}
            let hwnd=CreateWindowExW(WS_EX_LAYERED|WS_EX_TRANSPARENT|WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW|WS_EX_TOPMOST,
                class.as_ptr(),std::ptr::null(),WS_POPUP,x-26,y-26,52,52,std::ptr::null_mut(),std::ptr::null_mut(),inst,std::ptr::null());
            let result=(||->Result<()>{
                if hwnd.is_null(){bail!("Cannot create marker window");}
                let outer=CreateEllipticRgn(0,0,52,52);let inner=CreateEllipticRgn(5,5,47,47);
                if outer.is_null()||inner.is_null(){if !outer.is_null(){DeleteObject(outer);}if !inner.is_null(){DeleteObject(inner);}bail!("Cannot create marker region");}
                let combined=CombineRgn(outer,outer,inner,RGN_DIFF);DeleteObject(inner);
                if combined==0||SetWindowRgn(hwnd,outer,1)==0{DeleteObject(outer);bail!("Cannot set marker region");}
                // The system owns outer after successful SetWindowRgn.
                if SetLayeredWindowAttributes(hwnd,0,235,LWA_ALPHA)==0{bail!("Cannot set marker transparency");}
                let _=SetWindowDisplayAffinity(hwnd,0x11); // best effort exclusion from capture
                if !fresh(obs,req)||stop.load(Ordering::Relaxed){return Ok(());}
                if SetWindowPos(hwnd,HWND_TOPMOST,x-26,y-26,52,52,SWP_NOACTIVATE|SWP_SHOWWINDOW)==0{bail!("Cannot show marker");}
                let deadline=Instant::now()+Duration::from_secs(4);
                let mut msg:MSG=std::mem::zeroed();
                while Instant::now()<deadline&&!stop.load(Ordering::Relaxed)&&fresh(obs,req){
                    while PeekMessageW(&mut msg,hwnd,0,0,PM_REMOVE)!=0{DispatchMessageW(&msg);}
                    thread::sleep(Duration::from_millis(30));
                }
                Ok(())
            })();
            if !hwnd.is_null(){DestroyWindow(hwnd);}
            UnregisterClassW(class.as_ptr(),inst);DeleteObject(brush);
            result
        }
    }
}
#[cfg(not(windows))]
mod platform{
    use super::*;
    pub(super) fn show(_:&Observer,_:Request,_:&AtomicBool)->Result<()>{bail!("Windows only")}
}
pub async fn test(obs:Arc<Observer>)->Result<()>{
    eprintln!("10秒以内に対象アプリを前面にし、ペンを離してください。中央に4秒間リングを表示します。");
    let deadline=Instant::now()+Duration::from_secs(10);
    loop{
        let s=obs.snapshot();
        if s.active&&!s.drawing{
            if let Some(target)=s.target{if let Some(client_rect)=obs.client_rect(target){
                let _marker=Marker::show(obs.clone(),Request{target,client_rect,point:[0.5,0.5],generation:s.generation,seq:s.seq})?;
                tokio::select!{_=tokio::signal::ctrl_c()=>{},_=tokio::time::sleep(Duration::from_secs(4))=>{}}
                return Ok(());
            }}
        }
        if Instant::now()>=deadline{bail!("No foreground target");}
        tokio::select!{_=tokio::signal::ctrl_c()=>return Ok(()),_=tokio::time::sleep(Duration::from_millis(100))=>{}}
    }
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn maps_client_image_without_canvas_or_dpi_assumptions(){
        assert_eq!(map_point([-100,20,100,220],[0.0,0.0]),Some([-100,20]));
        assert_eq!(map_point([-100,20,100,220],[1.0,1.0]),Some([99,219]));
        assert!(map_point([0,0,10,10],[1.1,0.2]).is_none());
        assert!(map_point([0,0,0,10],[0.5,0.5]).is_none());
    }
}
