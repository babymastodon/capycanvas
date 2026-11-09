use crate::{inject::Injector,launch,run::{Result,qpc,frequency,write_line,Options}};
use std::{collections::BTreeMap,fs::File,io::{BufWriter,Write},path::Path,sync::{Arc,atomic::{AtomicBool,Ordering}}};
use poc_observer::{clock::{Clock,Tick},cpu::ThreadCpu};
use windows::Win32::System::Threading::THREAD_PRIORITY_TIME_CRITICAL;
use windows::{Win32::{Foundation::*,System::{Com::*,Variant::*},UI::{Accessibility::*,WindowsAndMessaging::*}}};

pub struct Targets {pub divider:RECT,pub list:RECT,pub slider:RECT,pub drawer:RECT,pub zen:RECT,pub text:RECT}
impl Targets {
    pub fn resolve(hwnd:HWND,workload:&str)->Result<Self> {
        let mut result=Self {divider:RECT::default(),list:RECT::default(),slider:RECT::default(),drawer:RECT::default(),zen:RECT::default(),text:RECT::default()};
        if matches!(workload,"startup"|"paint-still") {return Ok(result);}
        unsafe {CoInitializeEx(None,COINIT_MULTITHREADED).ok()?;}
        let automation:IUIAutomation=unsafe {CoCreateInstance(&CUIAutomation,None,CLSCTX_INPROC_SERVER)?};
        let root=unsafe {automation.ElementFromHandle(hwnd)?};
        let find=|id:&str,focus:bool|->Result<RECT> {
            let condition=unsafe {automation.CreatePropertyCondition(UIA_AutomationIdPropertyId,&VARIANT::from(id))?};
            let element=unsafe {root.FindFirst(TreeScope_Descendants,&condition)?};
            if focus {unsafe {element.SetFocus()?;}}
            Ok(unsafe {element.CurrentBoundingRectangle()?})
        };
        match workload {
            "divider"|"slowcanvas-divider"=>result.divider=find("divider",false)?,
            "scroll-wheel"|"scroll-touch"|"stall-scroll"=>result.list=find("layers-list",false)?,
            "drawer"|"stall-drawer"|"slowcanvas-drawer"=>result.drawer=find("drawer-toggle",false)?,
            "zen"=>result.zen=find("zen-toggle",false)?,
            "scrub-while-painting"=>result.slider=find("slider-size",false)?,
            "typing"=>result.text=find("text-field",true)?,
            "resize-programmatic"|"resize-native"=>{},
            _=>return Err(format!("unknown workload: {workload}").into()),
        }
        drop(root);drop(automation);unsafe {CoUninitialize();}
        Ok(result)
    }
}
#[derive(Clone)]
struct Event {seconds:f64,kind:&'static str,x:i32,y:i32,value:Option<f64>,state:Option<u16>,segment:u32,phase:u32,axes:[f64;4],key:u16,index:usize,measured:bool}
struct DragGeometry {base:f64,amplitude:f64,origin:i32}
fn drag_geometry(workload:&str,targets:&Targets,client:RECT,origin:POINT,scale:f64)->DragGeometry {
    if workload.starts_with("resize") {DragGeometry {base:client.right as f64,amplitude:200.0*scale,origin:0}}
    else if workload=="scrub-while-painting" {DragGeometry {base:center(targets.slider).x as f64,amplitude:(targets.slider.right-targets.slider.left) as f64*0.4,origin:origin.x}}
    else {DragGeometry {base:center(targets.divider).y as f64,amplitude:60.0*scale,origin:origin.y}}
}
pub fn calibration(options:&Options,targets:&Targets,client:RECT,origin:POINT,scale:f64)->Option<f64> {
    if !poc_observer::trajectory::is_drag(&options.workload)||options.warmup<=0.0{return None;}
    let geometry=drag_geometry(&options.workload,targets,client,origin,scale);
    Some(geometry.base-geometry.amplitude-geometry.origin as f64)
}
fn center(r:RECT)->POINT {POINT{x:(r.left+r.right)/2,y:(r.top+r.bottom)/2}}
pub fn run(options:&Options,owner:(usize,u32),targets:Targets,start:i64,path:&Path,stop:Arc<AtomicBool>,contact:u32)->Result<()> {
    let mut cpu=ThreadCpu::new("injection",&path.with_file_name("observer-cpu.jsonl"),Some(THREAD_PRIORITY_TIME_CRITICAL))?;
    let (hwnd,pid)=owner;
    let hwnd=HWND(hwnd as _);
    let (client,origin,scale)=launch::bounds(hwnd)?;
    let total=options.warmup+options.seconds;
    let mut events=Vec::new();
    let mut add=|seconds:f64,kind,x,y,value,state,segment,phase,axes,key,index|events.push(Event {seconds,kind,x,y,value,state,segment,phase,axes,key,index,measured:seconds>=options.warmup});
    let painting=matches!(options.workload.as_str(),"paint-still"|"scrub-while-painting");
    if painting {
        let count=(total*240.0).round() as usize;
        for i in 0..count {add(i as f64/240.0,"pen",0,0,None,None,0,1,[0.0;4],0,i);}
    }
    match options.workload.as_str() {
        "startup"|"paint-still"=>{},
        "divider"|"slowcanvas-divider"|"scrub-while-painting"|"resize-programmatic"|"resize-native"=>{
            let hz=if options.workload.starts_with("resize"){120.0}else{240.0};let count=(total*hz).round() as usize;
            for i in 0..count {add(i as f64/hz,if options.workload=="resize-programmatic"{"resize"}else{"mouse-move"},0,0,None,None,0,1,[0.0;4],0,i);}
        },
        "drawer"|"stall-drawer"|"slowcanvas-drawer"|"zen"=>{
            let at=center(if options.workload=="zen"{targets.zen}else{targets.drawer});
            for i in 0..(total/0.4).ceil() as usize {add(i as f64*0.4,"click",at.x,at.y,None,None,0,0,[0.0;4],0,i);}
        },
        "scroll-wheel"|"stall-scroll"=>{
            let at=center(targets.list);
            for i in 0..(total*60.0).round() as usize {let t=i as f64/60.0;add(t,"wheel",at.x,at.y,None,None,0,0,[0.0;4],0,i);}
        },
        "scroll-touch"=>{
            let mut index=0;
            for i in 0..total.ceil() as usize {for j in 0..=10 {add(i as f64+j as f64*0.004,if j==10{"touch-up"}else{"touch-move"},0,0,None,None,i as u32,if j==10{2}else{1},[0.5,0.0,0.0,0.0],0,index);index+=1;}}
        },
        "typing"=>{
            let text="Layer abc 日本語 layers drawing art brush opacity size flow spacing ";let chars:Vec<u16>=text.encode_utf16().collect();
            for i in 0..(total*10.0).round() as usize {add(i as f64/10.0,"key",0,0,None,None,0,0,[0.0;4],chars[i%chars.len()],i);}
        },
        _=>return Err(format!("unsupported workload {}",options.workload).into()),
    }
    events.retain(|event|event.seconds<total);
    let mut grouped:BTreeMap<&str,Vec<Event>>=BTreeMap::new();
    for event in events {grouped.entry(stream_name(event.kind)).or_default().push(event);}
    let mut streams:Vec<_>=grouped.into_iter().map(|(name,events)|Stream::new(name,events,options,total)).collect();
    let mut injector=Injector::new(pid)?;
    if painting {injector.prepare_pointer(true)?;}
    if options.workload=="scroll-touch" {injector.prepare_pointer(false)?;}
    let mut outer=RECT::default();unsafe{GetWindowRect(hwnd,&mut outer)?;}
    let geometry=drag_geometry(&options.workload,&targets,client,origin,scale);
    let context=Context {start,contact,options,targets:&targets,client,origin,scale,outer,geometry};
    let mut records=Vec::new();
    let result=(||->Result<()> {
        loop {
            if stop.load(Ordering::Acquire){return Err("observer canceled injection".into());}
            if options.samples.is_some_and(|target|streams.iter().any(|stream|stream.name=="pen"&&poc_observer::diagnostic::data_complete(target,stream.accepted_total))){break;}
            cpu.sample()?;
            let Some((stream_index,deadline))=streams.iter().enumerate().filter_map(|(index,stream)|stream.clock.next_deadline().map(|deadline|(index,deadline))).min_by(|a,b|a.1.total_cmp(&b.1)) else {break;};
            injector.wait(start+(deadline*frequency() as f64).ceil() as i64)?;
            let stamp=qpc();let actual=(stamp-start) as f64/frequency() as f64;
            let stream=&mut streams[stream_index];
            match stream.clock.take_due(actual) {
                Tick::Wait=>continue,
                Tick::Done {skipped}=>stream.skip(skipped),
                Tick::Due {slot,skipped,actual}=>{
                    stream.skip(skipped);
                    let mut event=stream.events[slot].clone();
                    if event.kind.starts_with("touch-") && injector.pointer_active(false) && stream.touch_segment!=Some(event.segment) {
                        stream.close(&mut injector,stamp,&context,&mut records)?;
                        stream.skip(slot..slot+1);continue;
                    }
                    if event.kind=="touch-up" {
                        stream.close(&mut injector,stamp,&context,&mut records)?;continue;
                    }
                    context.evaluate(&mut event,actual,stream.accepted_total);
                    if event.kind=="mouse-move" && !injector.mouse_active() {
                        let point=if context.options.workload=="resize-native" {POINT{x:outer.right-1,y:(outer.top+outer.bottom)/2}}else{center(if context.options.workload=="scrub-while-painting" {targets.slider}else{targets.divider})};
                        injector.mouse(point,Some(true),0,true).map_err(|error|format!("mouse capture down before source_index={}: {error}",event.index))?;
                        event.kind="mouse-down";event.x=point.x;event.y=point.y;event.value=None;event.state=None;event.phase=0;
                        stream.setup_samples+=1;stream.skip(slot..slot+1);stream.push(event,stamp,&context,false,&mut records);continue;
                    }
                    if event.kind=="click" {event.value=Some(if context.options.workload=="zen" {if stream.accepted_total.is_multiple_of(2) {0.0}else{1.0}}else if stream.accepted_total.is_multiple_of(2) {280.0*scale}else{0.0});}
                    if event.kind=="key" {event.state=Some((stream.accepted_total+1) as u16);event.key=stream.events[stream.accepted_total].key;}
                    if event.kind=="pen" {event.phase=u32::from(injector.pointer_active(true));}
                    if event.kind.starts_with("touch-") {event.phase=u32::from(injector.pointer_active(false));event.kind=if event.phase==0 {"touch-down"}else{"touch-move"};stream.touch_segment=Some(event.segment);}
                    let point=POINT{x:event.x,y:event.y};
                    let outcome=match event.kind {
                        "pen"=>injector.pointer(true,point,event.phase,event.axes,stamp),
                        "touch-down"|"touch-move"=>injector.pointer(false,point,event.phase,event.axes,stamp),
                        "mouse-move"=>injector.mouse(point,None,0,false),
                        "wheel"=>injector.mouse(point,None,event.value.unwrap() as i32,true),
                        "click"=>(||{injector.mouse(point,Some(true),0,true)?;injector.mouse(point,Some(false),0,true)})(),
                        "key"=>injector.key(event.key),
                        "resize"=>unsafe {SetWindowPos(hwnd,None,outer.left,outer.top,event.value.unwrap().round() as i32+(outer.right-outer.left-client.right),outer.bottom-outer.top,SWP_NOZORDER|SWP_NOACTIVATE)}.map_err(|error|format!("SetWindowPos(resize,client_width={:?}): {error}",event.value).into()),
                        _=>unreachable!(),
                    };
                    outcome.map_err(|error|format!("injection stream={} accepted_index={} source_index={} actual_seconds={actual:.9}: {error}",stream.name,stream.next_index,event.index))?;
                    stream.accepted_total+=1;
                    if actual>=options.warmup {stream.accepted_measured+=1;}
                    stream.push(event,stamp,&context,false,&mut records);
                }
            }
        }
        if options.samples.is_none(){injector.wait(start+(total*frequency() as f64).ceil() as i64)?;}
        for stream in &mut streams {
            if matches!(stream.name,"pen"|"touch"|"mouse") && stream.last_qpc.is_some() {
                let deadline=stream.last_finish_qpc+(frequency() as f64/stream.nominal_hz).ceil() as i64;
                injector.wait(deadline)?;
                stream.close(&mut injector,qpc(),&context,&mut records)?;
            }
        }
        if let Some(target)=options.samples {
            let accepted=streams.iter().find(|stream|stream.name=="pen").map_or(0,|stream|stream.next_index);
            if accepted!=target as usize{return Err(format!("pen-check reached its 60 second deadline with {accepted} accepted samples; target was {target}").into());}
        }
        Ok(())
    })();
    cpu.finish()?;
    drop(injector);
    let mut file=BufWriter::new(File::create(path)?);
    for record in records {write_line(&mut file,&record)?;}file.flush()?;
    let active_start=streams.iter().filter_map(|stream|stream.first_qpc).min();let active_end=streams.iter().filter_map(|stream|stream.last_qpc).max();
    let measured_seconds=if options.samples.is_some(){active_start.zip(active_end).map_or(0.0,|(first,last)|(last-first) as f64/frequency() as f64)}else{options.seconds};
    if options.samples.is_some(){for stream in &mut streams{stream.measured_seconds=measured_seconds;}}
    let input_limited=streams.iter().any(|stream|stream.input_limited());
    let summary=serde_json::json!({"measured_seconds":measured_seconds,"maximum_seconds":options.samples.map(|_|poc_observer::diagnostic::MAX_SECONDS),"requested_samples":options.samples,"active_start_qpc":active_start,"active_end_qpc":active_end,"warmup_seconds":options.warmup,"input_limited":input_limited,"streams":streams.iter().map(|stream|(stream.name,stream.summary())).collect::<BTreeMap<_,_>>(),"failure":result.as_ref().err().map(ToString::to_string)});
    serde_json::to_writer_pretty(File::create(path.with_file_name("injection.json"))?,&summary)?;
    result
}
fn stream_name(kind:&str)->&str {match kind {"pen"=>"pen","mouse-move"=>"mouse","touch-down"|"touch-move"|"touch-up"=>"touch","wheel"=>"wheel","click"=>"click","key"=>"key","resize"=>"resize",_=>unreachable!()}}
struct Context<'a> {start:i64,contact:u32,options:&'a Options,targets:&'a Targets,client:RECT,origin:POINT,scale:f64,outer:RECT,geometry:DragGeometry}
impl Context<'_> {
    fn evaluate(&self,event:&mut Event,actual:f64,accepted_index:usize) {
        event.measured=actual>=self.options.warmup;
        if event.kind=="pen" {
            let angle=actual*std::f64::consts::PI;let progress=self.options.samples.map_or_else(||(actual/(self.options.warmup+self.options.seconds)).clamp(0.0,1.0),|target|poc_observer::diagnostic::axis_progress(target,accepted_index));
            event.x=(self.origin.x as f64+self.client.right as f64-540.0*self.scale+150.0*self.scale*angle.cos()).round() as i32;
            event.y=(self.origin.y as f64+500.0*self.scale+150.0*self.scale*angle.sin()).round() as i32;
            event.axes=[0.2+0.7*progress,30.0*angle.sin(),30.0*angle.cos(),(progress*359.0).round()];
        } else if matches!(event.kind,"mouse-move"|"resize") {
            let resize=self.options.workload.starts_with("resize");let scrub=self.options.workload=="scrub-while-painting";
            let sample=poc_observer::trajectory::drag(actual,self.options.warmup,self.geometry.base,self.geometry.amplitude);
            let at=center(if scrub {self.targets.slider}else{self.targets.divider});
            event.x=if resize {self.outer.right-1+(sample.value-self.geometry.base).round() as i32}else if scrub {sample.value.round() as i32}else{at.x};
            event.y=if self.options.workload=="resize-native" {(self.outer.top+self.outer.bottom)/2}else if resize {self.origin.y}else if scrub {at.y}else{sample.value.round() as i32};
            event.value=Some(sample.value-self.geometry.origin as f64);event.state=if resize {Some(sample.value.round() as u16)}else{None};event.segment=sample.segment;
        } else if event.kind.starts_with("touch-") {
            let at=center(self.targets.list);let progress=((actual-event.segment as f64)/0.04).clamp(0.0,1.0);
            event.x=at.x;event.y=at.y+(200.0*self.scale-400.0*self.scale*progress).round() as i32;event.value=Some(400.0*self.scale*progress);
        } else if event.kind=="wheel" {event.value=Some(if actual-self.options.warmup<self.options.seconds/2.0 {-120.0}else{120.0});}
    }
}
struct Stream {name:&'static str,events:Vec<Event>,clock:Clock,nominal_hz:f64,measured_seconds:f64,next_index:usize,accepted_total:usize,accepted_measured:usize,skipped_slots:usize,skipped_measured_slots:usize,terminal_samples:usize,setup_samples:usize,first_qpc:Option<i64>,last_qpc:Option<i64>,last_finish_qpc:i64,last_event:Option<Event>,touch_segment:Option<u32>}
impl Stream {
    fn new(name:&str,events:Vec<Event>,options:&Options,total:f64)->Self {
        let name=match name {"pen"=>"pen","mouse"=>"mouse","touch"=>"touch","wheel"=>"wheel","click"=>"click","key"=>"key","resize"=>"resize",_=>unreachable!()};
        let nominal_hz=poc_observer::clock::nominal_hz(&options.workload,name);
        let measured_seconds=if name=="touch" {(0..total.ceil() as usize).map(|i|((i as f64+0.04).min(total)-(i as f64).max(options.warmup)).max(0.0)).sum()}else{options.seconds};
        let clock=Clock::new(events.iter().map(|event|event.seconds).collect(),total,1.0/nominal_hz);
        Self {name,events,clock,nominal_hz,measured_seconds,next_index:0,accepted_total:0,accepted_measured:0,skipped_slots:0,skipped_measured_slots:0,terminal_samples:0,setup_samples:0,first_qpc:None,last_qpc:None,last_finish_qpc:0,last_event:None,touch_segment:None}
    }
    fn skip(&mut self,range:std::ops::Range<usize>) {for event in &self.events[range] {if event.kind!="touch-up" {self.skipped_slots+=1;if event.measured {self.skipped_measured_slots+=1;}}}}
    fn input_limited(&self)->bool {self.measured_seconds>0.0 && self.accepted_measured as f64/self.measured_seconds<0.9*self.nominal_hz}
    fn summary(&self)->serde_json::Value {serde_json::json!({"stream":self.name,"nominal_hz":self.nominal_hz,"measured_seconds":self.measured_seconds,"accepted_total":self.accepted_total,"accepted_measured":self.accepted_measured,"input_records":self.next_index,"terminal_samples":self.terminal_samples,"setup_samples":self.setup_samples,"skipped_slots":self.skipped_slots,"skipped_measured_slots":self.skipped_measured_slots,"achieved_hz":if self.measured_seconds>0.0 {Some(self.accepted_measured as f64/self.measured_seconds)}else{None},"input_limited":self.input_limited()})}
    fn push(&mut self,event:Event,stamp:i64,context:&Context<'_>,terminal:bool,records:&mut Vec<serde_json::Value>) {
        let start=context.start;let contact=context.contact;
        let after=qpc();
        let actual=(stamp-start) as f64/frequency() as f64;let scheduled=start+(event.seconds*frequency() as f64).round() as i64;
        records.push(serde_json::json!({"qpc":stamp,"qpc_after":after,"scheduled_qpc":scheduled,"index":self.next_index,"source_index":if terminal||event.kind=="mouse-down"{None}else{Some(event.index)},"setup":event.kind=="mouse-down","stream":self.name,"kind":event.kind,"expected_state":event.state,"expected_value":event.value,"segment":event.segment,"contact":contact,"measured":actual>=context.options.warmup&&actual<context.options.warmup+context.options.seconds,"warmup":actual<context.options.warmup,"terminal":terminal,"phase":event.phase,"actual_time_seconds":actual,"resting_calibration":poc_observer::trajectory::is_drag(&context.options.workload)&&matches!(event.kind,"mouse-move"|"resize")&&poc_observer::trajectory::drag(actual,context.options.warmup,0.0,0.0).resting,"timestamp":stamp,"x":event.x-context.origin.x,"y":event.y-context.origin.y,"pressure":event.axes[0],"tilt_x":event.axes[1],"tilt_y":event.axes[2],"twist":event.axes[3]}));
        self.next_index+=1;self.first_qpc.get_or_insert(stamp);self.last_qpc=Some(stamp);self.last_finish_qpc=after;self.last_event=Some(event);
    }
    fn close(&mut self,injector:&mut Injector,stamp:i64,context:&Context<'_>,records:&mut Vec<serde_json::Value>)->Result<()> {
        let active=if self.name=="mouse" {injector.mouse_active()}else{injector.pointer_active(self.name=="pen")};
        if !active {return Ok(());}
        let Some(mut event)=self.last_event.clone() else {return Ok(());};
        let point=if self.name=="mouse" {POINT{x:event.x,y:event.y}}else{injector.last_pointer(self.name=="pen")};
        event.x=point.x;event.y=point.y;event.phase=2;event.axes=[0.0;4];event.seconds=(stamp-context.start) as f64/frequency() as f64;
        if self.name=="mouse" {event.kind="mouse-up";injector.mouse(point,Some(false),0,false)?;}else{event.kind=if self.name=="pen" {"pen"}else{"touch-up"};injector.pointer(self.name=="pen",point,2,event.axes,stamp)?;}
        self.terminal_samples+=1;self.push(event,stamp,context,true,records);Ok(())
    }
}
