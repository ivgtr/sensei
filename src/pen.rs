//! Standard digitizer HID input (not vendor-specific reports).
//! Native tablet coordinates are NOT screen or canvas coordinates.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub device: usize,
    pub tip: bool,
    pub xy: Option<[f64;2]>,
    pub pressure: Option<f64>,
    pub tilt_raw: [Option<i32>;2],
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Point {
    pub at_ms: u64,
    #[serde(default)] pub sample_index: u32,
    pub xy: Option<[f64;2]>,
    pub pressure: Option<f64>,
    pub tilt_raw: [Option<i32>;2],
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PenStroke {
    pub samples: u32,
    pub tablet_bounds: Option<[f64;4]>,
    pub path_length_normalized: f64,
    pub pressure_samples: u32,
    pub pressure_mean: Option<f64>,
    pub pressure_min: Option<f64>,
    pub pressure_max: Option<f64>,
    pub tilt_raw_min: [Option<i32>;2],
    pub tilt_raw_max: [Option<i32>;2],
    pub trace: Vec<Point>,
    pub trace_stride: u32,
}
pub struct Accumulator {
    pub device: usize,
    pub start_ms: u64,
    pub last_ms: u64,
    out: PenStroke,
    pressure_sum: f64,
    last_xy: Option<[f64;2]>,
    last_point: Option<Point>,
}
impl Accumulator {
    pub fn new(device:usize,ms:u64)->Self{
        Self{device,start_ms:ms,last_ms:ms,pressure_sum:0.0,last_xy:None,last_point:None,
            out:PenStroke{samples:0,tablet_bounds:None,path_length_normalized:0.0,pressure_samples:0,
                pressure_mean:None,pressure_min:None,pressure_max:None,
                tilt_raw_min:[None;2],tilt_raw_max:[None;2],trace:Vec::new(),trace_stride:1}}
    }
    pub fn sample(&mut self,s:Sample,ms:u64){
        self.last_ms=ms;
        self.out.samples=self.out.samples.saturating_add(1);
        if let Some([x,y])=s.xy{
            let b=self.out.tablet_bounds.get_or_insert([x,y,x,y]);
            *b=[b[0].min(x),b[1].min(y),b[2].max(x),b[3].max(y)];
            if let Some([lx,ly])=self.last_xy {self.out.path_length_normalized+=(x-lx).hypot(y-ly);}
            self.last_xy=Some([x,y]);
        } else {self.last_xy=None;}
        if let Some(p)=s.pressure {
            self.out.pressure_samples=self.out.pressure_samples.saturating_add(1);
            self.pressure_sum+=p;
            self.out.pressure_min=Some(self.out.pressure_min.map_or(p,|v|v.min(p)));
            self.out.pressure_max=Some(self.out.pressure_max.map_or(p,|v|v.max(p)));
        }
        for i in 0..2 {
            if let Some(v)=s.tilt_raw[i]{
                self.out.tilt_raw_min[i]=Some(self.out.tilt_raw_min[i].map_or(v,|a|a.min(v)));
                self.out.tilt_raw_max[i]=Some(self.out.tilt_raw_max[i].map_or(v,|a|a.max(v)));
            }
        }
        let point=Point{at_ms:ms.saturating_sub(self.start_ms),sample_index:self.out.samples-1,xy:s.xy,pressure:s.pressure,tilt_raw:s.tilt_raw};
        if self.out.trace.len()>=128 {
            self.out.trace=self.out.trace.drain(..).step_by(2).collect();
            self.out.trace_stride=self.out.trace_stride.saturating_mul(2);
        }
        if (self.out.samples-1)%self.out.trace_stride==0 {self.out.trace.push(point.clone());}
        self.last_point=Some(point);
    }
    pub fn finish(mut self)->PenStroke{
        if self.out.pressure_samples>0{self.out.pressure_mean=Some(self.pressure_sum/self.out.pressure_samples as f64);}
        if let Some(last)=self.last_point{
            if self.out.trace.last().map(|p|p.sample_index)!=Some(last.sample_index) {
                if self.out.trace.len()>=128{self.out.trace.pop();}
                self.out.trace.push(last);
            }
        }
        self.out
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct DeviceInfo{
    pub vendor_id:u32,pub product_id:u32,pub usage:u16,
    pub pressure:bool,pub tablet_xy:bool,pub tilt_x:bool,pub tilt_y:bool,pub tip_switch:bool,
}

#[cfg(windows)]
pub mod native {
    use super::*;
    use std::{collections::HashMap, ffi::c_void, mem::{size_of,zeroed}, ptr::{null,null_mut}};
    use windows_sys::Win32::{
        Devices::HumanInterfaceDevice::*,
        Foundation::{HANDLE,HWND,LPARAM,LRESULT,WPARAM},
        System::{LibraryLoader::GetModuleHandleW,SystemInformation::GetTickCount},
        UI::{Input::*,WindowsAndMessaging::*},
    };

    const PAGE:u16=0x0d;
    const PEN:u16=0x02;
    const STYLUS:u16=0x20;
    const MAX_REPORT:usize=65536;
    fn is_pen(page:u16,usage:u16)->bool{page==PAGE && matches!(usage,PEN|STYLUS)}
    fn hid_info(device:HANDLE)->Option<RID_DEVICE_INFO_HID>{
        unsafe{
            let mut info:RID_DEVICE_INFO=zeroed();info.cbSize=size_of::<RID_DEVICE_INFO>() as u32;
            let mut len=info.cbSize;
            if GetRawInputDeviceInfoW(device,RIDI_DEVICEINFO,(&mut info as *mut RID_DEVICE_INFO).cast(),&mut len)==u32::MAX || info.dwType!=RIM_TYPEHID{return None;}
            let hid=info.Anonymous.hid;
            is_pen(hid.usUsagePage,hid.usUsage).then_some(hid)
        }
    }
    struct Descriptor {
        // Pointer-sized alignment for opaque Windows preparsed data.
        data:Vec<usize>,
        caps:HIDP_CAPS,
        info:RID_DEVICE_INFO_HID,
        fields:HashMap<(u16,u16),Vec<HIDP_VALUE_CAPS>>,
        tips:Vec<HIDP_BUTTON_CAPS>,
    }
    impl Descriptor {
        fn open(device:HANDLE)->Option<Self>{
            let info=hid_info(device)?;
            unsafe{
                let mut size=0u32;
                if GetRawInputDeviceInfoW(device,RIDI_PREPARSEDDATA,null_mut(),&mut size)==u32::MAX || size==0 || size as usize>MAX_REPORT {return None;}
                let mut data=vec![0usize;(size as usize).div_ceil(size_of::<usize>())];
                if GetRawInputDeviceInfoW(device,RIDI_PREPARSEDDATA,data.as_mut_ptr().cast(),&mut size)==u32::MAX{return None;}
                let mut caps:HIDP_CAPS=zeroed();
                if HidP_GetCaps(data.as_ptr() as isize,&mut caps)!=HIDP_STATUS_SUCCESS || !is_pen(caps.UsagePage,caps.Usage) {return None;}
                let mut d=Self{data,caps,info,fields:HashMap::new(),tips:Vec::new()};
                for (page,usage) in [(1,0x30),(1,0x31),(PAGE,0x30),(PAGE,0x3d),(PAGE,0x3e)] {
                    let caps=d.read_values(page,usage);d.fields.insert((page,usage),caps);
                }
                d.tips=d.read_tip_caps();
                Some(d)
            }
        }
        fn ptr(&self)->PHIDP_PREPARSED_DATA{self.data.as_ptr() as isize}
        fn read_values(&self,page:u16,usage:u16)->Vec<HIDP_VALUE_CAPS>{
            let mut count=self.caps.NumberInputValueCaps.min(256);
            if count==0{return Vec::new();}
            let mut caps:Vec<HIDP_VALUE_CAPS>=vec![unsafe{zeroed()};count as usize];
            let status=unsafe{HidP_GetSpecificValueCaps(HidP_Input,page,0,usage,caps.as_mut_ptr(),&mut count,self.ptr())};
            if status!=HIDP_STATUS_SUCCESS || count as usize>caps.len(){return Vec::new();}
            caps.truncate(count as usize);caps
        }
        fn read_tip_caps(&self)->Vec<HIDP_BUTTON_CAPS>{
            let mut count=self.caps.NumberInputButtonCaps.min(256);
            if count==0{return Vec::new();}
            let mut caps:Vec<HIDP_BUTTON_CAPS>=vec![unsafe{zeroed()};count as usize];
            let status=unsafe{HidP_GetSpecificButtonCaps(HidP_Input,PAGE,0,0x42,caps.as_mut_ptr(),&mut count,self.ptr())};
            if status!=HIDP_STATUS_SUCCESS || count as usize>caps.len(){return Vec::new();}
            caps.truncate(count as usize);caps
        }
        fn values(&self,page:u16,usage:u16)->&[HIDP_VALUE_CAPS]{
            self.fields.get(&(page,usage)).map(Vec::as_slice).unwrap_or(&[])
        }
        fn info(&self)->DeviceInfo{
            DeviceInfo{vendor_id:self.info.dwVendorId,product_id:self.info.dwProductId,usage:self.info.usUsage,
                pressure:!self.values(PAGE,0x30).is_empty(),tablet_xy:!self.values(1,0x30).is_empty()&&!self.values(1,0x31).is_empty(),
                tilt_x:!self.values(PAGE,0x3d).is_empty(),tilt_y:!self.values(PAGE,0x3e).is_empty(),tip_switch:!self.tips.is_empty()}
        }
        fn value(&self,report:&[u8],page:u16,usage:u16)->Option<(i32,i32,i32)>{
            let cap=self.values(page,usage).iter().find(|c|c.ReportID==report[0] && c.ReportCount==1 && (1..=32).contains(&c.BitSize))?;
            let mut v=0;
            if unsafe{HidP_GetUsageValue(HidP_Input,page,cap.LinkCollection,usage,&mut v,self.ptr(),report.as_ptr(),report.len() as u32)}!=HIDP_STATUS_SUCCESS{return None;}
            let raw=if cap.LogicalMin<0 {
                let shift=32-u32::from(cap.BitSize);((v<<shift) as i32)>>shift
            }else{i32::try_from(v).ok()?};
            if raw<cap.LogicalMin||raw>cap.LogicalMax{return None;}
            Some((raw,cap.LogicalMin,cap.LogicalMax))
        }
        fn normalized(&self,report:&[u8],page:u16,usage:u16)->Option<f64>{
            let (v,min,max)=self.value(report,page,usage)?;
            (max>min).then(||(f64::from(v)-f64::from(min))/(f64::from(max)-f64::from(min)))
        }
        fn decode(&self,device:usize,report:&mut[u8])->Option<Sample>{
            if report.is_empty() || report.len()!=self.caps.InputReportByteLength as usize{return None;}
            let tip=self.tips.iter().find(|c|c.ReportID==report[0])?;
            let mut usages=[0u16;128];let mut count=usages.len() as u32;
            if unsafe{HidP_GetUsages(HidP_Input,PAGE,tip.LinkCollection,usages.as_mut_ptr(),&mut count,self.ptr(),report.as_mut_ptr(),report.len() as u32)}!=HIDP_STATUS_SUCCESS || count as usize>usages.len(){return None;}
            let contact=usages[..count as usize].contains(&0x42);
            let xy=self.normalized(report,1,0x30).zip(self.normalized(report,1,0x31)).map(|(x,y)|[x,y]);
            Some(Sample{device,tip:contact,xy,pressure:self.normalized(report,PAGE,0x30),
                tilt_raw:[self.value(report,PAGE,0x3d).map(|v|v.0),self.value(report,PAGE,0x3e).map(|v|v.0)]})
        }
    }
    pub fn devices()->anyhow::Result<Vec<DeviceInfo>>{
        unsafe{
            let mut count=0u32;
            if GetRawInputDeviceList(null_mut(),&mut count,size_of::<RAWINPUTDEVICELIST>() as u32)==u32::MAX || count>4096 {anyhow::bail!("Raw Input enumeration failed");}
            let mut list:Vec<RAWINPUTDEVICELIST>=vec![zeroed();count as usize];
            let n=GetRawInputDeviceList(list.as_mut_ptr(),&mut count,size_of::<RAWINPUTDEVICELIST>() as u32);
            if n==u32::MAX || n as usize>list.len(){anyhow::bail!("Raw Input device list changed; retry");}
            Ok(list[..n as usize].iter().filter(|d|d.dwType==RIM_TYPEHID)
                .filter_map(|d|Descriptor::open(d.hDevice)).map(|d|d.info()).collect())
        }
    }

    unsafe extern "system" fn wndproc(hwnd:HWND,msg:u32,w:WPARAM,l:LPARAM)->LRESULT{unsafe{DefWindowProcW(hwnd,msg,w,l)}}
    pub struct Pump{hwnd:HWND,instance:windows_sys::Win32::Foundation::HINSTANCE,class:Vec<u16>,descriptors:HashMap<usize,Descriptor>,usages:Vec<u16>}
    impl Pump{
        pub fn new()->anyhow::Result<Self>{
            unsafe{
                let class:Vec<u16>="SenseiPenObserverV04\0".encode_utf16().collect();
                let instance=GetModuleHandleW(null());
                let mut wc:WNDCLASSW=zeroed();wc.lpfnWndProc=Some(wndproc);wc.hInstance=instance;wc.lpszClassName=class.as_ptr();
                if RegisterClassW(&wc)==0{anyhow::bail!("Cannot register pen observer window");}
                let hwnd=CreateWindowExW(0,class.as_ptr(),null(),0,0,0,0,0,HWND_MESSAGE,null_mut(),instance,null());
                if hwnd.is_null(){UnregisterClassW(class.as_ptr(),instance);anyhow::bail!("Cannot create pen observer window");}
                let mut usages=Vec::new();
                for usage in [PEN,STYLUS]{
                    let device=RAWINPUTDEVICE{usUsagePage:PAGE,usUsage:usage,dwFlags:RIDEV_INPUTSINK|RIDEV_DEVNOTIFY,hwndTarget:hwnd};
                    if RegisterRawInputDevices(&device,1,size_of::<RAWINPUTDEVICE>() as u32)!=0{usages.push(usage);}
                }
                let pump=Self{hwnd,instance,class,descriptors:HashMap::new(),usages};
                if pump.usages.is_empty(){anyhow::bail!("Pen Raw Input registration failed");}
                Ok(pump)
            }
        }
        fn read(&mut self,handle:HRAWINPUT)->Vec<Sample>{
            unsafe{
                let mut size=0u32;
                let header=size_of::<RAWINPUTHEADER>() as u32;
                if GetRawInputData(handle,RID_INPUT,null_mut(),&mut size,header)!=0 || size<header+8 || size as usize>MAX_REPORT {return Vec::new();}
                let mut storage=vec![0usize;(size as usize).div_ceil(size_of::<usize>())];
                let n=GetRawInputData(handle,RID_INPUT,storage.as_mut_ptr().cast::<c_void>(),&mut size,header);
                if n==u32::MAX || n>size || n<header+8{return Vec::new();}
                let ptr=storage.as_mut_ptr().cast::<u8>();
                let h=std::ptr::read_unaligned(ptr.cast::<RAWINPUTHEADER>());
                if h.dwType!=RIM_TYPEHID || h.dwSize>n || h.dwSize<header+8 || h.hDevice.is_null(){return Vec::new();}
                let device=h.hDevice as usize;
                if !self.descriptors.contains_key(&device){
                    if self.descriptors.len()>=32{return Vec::new();}
                    let Some(d)=Descriptor::open(h.hDevice) else{return Vec::new();};
                    self.descriptors.insert(device,d);
                }
                let Some(d)=self.descriptors.get(&device) else{return Vec::new();};
                let bytes=std::slice::from_raw_parts_mut(ptr.add(header as usize),h.dwSize as usize-header as usize);
                let size=u32::from_ne_bytes(bytes[..4].try_into().unwrap()) as usize;
                let count=u32::from_ne_bytes(bytes[4..8].try_into().unwrap()) as usize;
                if size==0 || count>1024 || size.checked_mul(count).is_none_or(|n|n>bytes.len()-8){return Vec::new();}
                bytes[8..8+size*count].chunks_exact_mut(size).filter_map(|r|d.decode(device,r)).collect()
            }
        }
        // Drain even outside the target, but never decode/store those reports.
        // Reject stale messages to reduce misattribution after focus changes.
        pub fn poll(&mut self,accept:bool)->(Vec<Sample>,bool){
            let mut out=Vec::new();let mut reset=false;
            unsafe{
                let mut msg:MSG=zeroed();
                for _ in 0..512{
                    if PeekMessageW(&mut msg,self.hwnd,0,0,PM_REMOVE)==0{break;}
                    if msg.message==WM_INPUT_DEVICE_CHANGE {self.descriptors.clear();reset=true;}
                    if msg.message==WM_INPUT && accept && GetTickCount().wrapping_sub(msg.time)<=100{
                        out.extend(self.read(msg.lParam as HRAWINPUT));
                    }
                    DispatchMessageW(&msg);
                }
            }
            (out,reset)
        }
    }
    impl Drop for Pump{
        fn drop(&mut self){unsafe{
            for &usage in &self.usages{
                let device=RAWINPUTDEVICE{usUsagePage:PAGE,usUsage:usage,dwFlags:RIDEV_REMOVE,hwndTarget:null_mut()};
                RegisterRawInputDevices(&device,1,size_of::<RAWINPUTDEVICE>() as u32);
            }
            DestroyWindow(self.hwnd);UnregisterClassW(self.class.as_ptr(),self.instance);
        }}
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn aggregates_pressure_and_bounds_without_inventing_missing_fields(){
        let mut a=Accumulator::new(1,100);
        a.sample(Sample{device:1,tip:true,xy:Some([0.2,0.3]),pressure:Some(0.2),tilt_raw:[Some(-5),None]},100);
        a.sample(Sample{device:1,tip:true,xy:Some([0.5,0.6]),pressure:Some(0.8),tilt_raw:[Some(7),None]},110);
        a.sample(Sample{device:1,tip:true,xy:None,pressure:None,tilt_raw:[None,None]},120);
        let s=a.finish();assert_eq!(s.samples,3);assert_eq!(s.pressure_samples,2);assert_eq!(s.pressure_mean,Some(0.5));
        assert_eq!(s.tablet_bounds,Some([0.2,0.3,0.5,0.6]));assert_eq!(s.tilt_raw_min,[Some(-5),None]);assert_eq!(s.tilt_raw_max,[Some(7),None]);
    }
    #[test]fn keeps_last_sample_when_reports_share_a_timestamp(){
        let mut a=Accumulator::new(1,0);
        for i in 0..1000{a.sample(Sample{device:1,tip:true,xy:Some([i as f64/1000.0,0.0]),pressure:None,tilt_raw:[None;2]},10);}
        let s=a.finish();assert_eq!(s.trace.last().unwrap().sample_index,999);
        assert_eq!(s.trace.last().unwrap().xy,Some([0.999,0.0]));
    }
    #[test]fn trace_bounded_and_summary_preserved(){
        let mut a=Accumulator::new(1,0);
        for i in 0..5000{a.sample(Sample{device:1,tip:true,xy:None,pressure:None,tilt_raw:[None;2]},i);}
        let s=a.finish();assert_eq!(s.samples,5000);assert!(s.trace.len()<=128);assert_eq!(s.trace.last().unwrap().at_ms,4999);assert_eq!(s.pressure_mean,None);
    }
}
