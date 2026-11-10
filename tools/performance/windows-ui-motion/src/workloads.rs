use crate::{run::{Plan,Result,qpc,frequency},inject::Injector};
use std::{fs::File,io::{BufWriter,Write},path::Path};
use windows::Win32::{Foundation::*,System::Threading::THREAD_PRIORITY_TIME_CRITICAL,UI::{Input::KeyboardAndMouse::GetDoubleClickTime,WindowsAndMessaging::*}};
use crate::{clock::{Clock,Tick},trajectory};
pub fn nominal_hz(plan:&Plan)->f64 {
    match plan.workload.as_str(){"resize-native"|"resize-programmatic"=>120.,"wheel"=>60.,"drawer"=>1./(unsafe{GetDoubleClickTime()} as f64/1000.+0.1).max(0.4),"expansion"=>1./1.2,"typing"=>10.,_=>240.}
}
pub fn run(plan:&Plan,start:i64,path:&Path)->Result<()> {
    let mut cpu=crate::cpu::ThreadCpu::new("injection",&path.with_file_name("observer-cpu.jsonl"),Some(THREAD_PRIORITY_TIME_CRITICAL))?;
    let mut injector=Injector::new(plan.process_id)?;let hwnd=HWND(plan.hwnd as _);let mut records=Vec::new();
    let hz=nominal_hz(plan);
    let mut clock=Clock::new((0..(plan.seconds*hz).ceil() as usize).map(|i|i as f64/hz).collect(),plan.seconds,1./hz);let mut down=false;
    let result=(||->Result<()>{while let Some(deadline)=clock.next_deadline(){
        injector.wait(start+(deadline*frequency() as f64) as i64)?;
        let now=qpc();let elapsed=(now-start) as f64/frequency() as f64;let Tick::Due{slot,skipped,actual}=clock.take_due(elapsed)else{continue;};
        cpu.sample()?;let mut point=POINT{x:plan.point[0],y:plan.point[1]};
        let motion=trajectory::centred(actual,0.,plan.amplitude);let displacement=motion.value;let mut event_at=now;
        if plan.axis==0{point.x+=displacement.round() as i32;}else{point.y+=displacement.round() as i32;}
        let expected=match plan.workload.as_str(){
            "observe"=>None,
            "divider"|"column"|"slider"|"resize-native"=>{injector.mouse(point,if down{None}else{Some(true)},0,!down)?;down=true;Some(if plan.axis==0{point.x}else{point.y})},
            "resize-programmatic"=>{injector.check_window(hwnd)?;let [x,y,w,h]=plan.window;unsafe{SetWindowPos(hwnd,None,x,y,w+displacement.round() as i32,h,SWP_NOZORDER|SWP_NOACTIVATE|SWP_ASYNCWINDOWPOS)?;}Some(plan.point[0]+displacement.round() as i32)},
            "wheel"=>{injector.mouse(POINT{x:plan.point[0],y:plan.point[1]},None,if (slot/8)%2==0{-120}else{120},true)?;None},
            "drawer"=>{let p=POINT{x:plan.point[0],y:plan.point[1]};injector.mouse(p,Some(true),0,true)?;injector.mouse(p,Some(false),0,true)?;None},
            "expansion"=>{if slot%2==0{injector.mouse(POINT{x:plan.point[0],y:plan.point[1]},None,0,true)?;injector.virtual_key(0x10,false)?;injector.virtual_key(0x79,false)?;injector.virtual_key(0x79,true)?;injector.virtual_key(0x10,true)?;injector.wait(qpc()+frequency()/10)?;let p=POINT{x:plan.menu[0],y:plan.menu[1]};event_at=qpc();injector.mouse(p,Some(true),0,true)?;injector.mouse(p,Some(false),0,true)?;}else{event_at=qpc();injector.virtual_key(0x1b,false)?;injector.virtual_key(0x1b,true)?;}None},
            "typing"=>{let phrase="pencil";let phase=slot%(phrase.len()*2);if phase<phrase.len(){injector.key(phrase.as_bytes()[phase] as u16)?;}else{injector.virtual_key(8,false)?;injector.virtual_key(8,true)?;}None},
            other=>return Err(format!("unsupported workload: {other}").into())
        };
        records.push(serde_json::json!({"qpc":event_at,"setup_qpc":now,"end_qpc":qpc(),"slot":slot,"segment":motion.segment,"skipped":skipped.len(),"expected":expected,"kind":plan.workload}));
    }injector.wait(start+(plan.seconds*frequency() as f64) as i64)?;Ok(())})();
    let terminal=qpc();
    if down{let origin=POINT{x:plan.point[0],y:plan.point[1]};injector.mouse(origin,None,0,false)?;injector.mouse(origin,Some(false),0,false)?;}
    if plan.workload=="resize-programmatic"{injector.check_window(hwnd)?;let [x,y,w,h]=plan.window;unsafe{SetWindowPos(hwnd,None,x,y,w,h,SWP_NOZORDER|SWP_NOACTIVATE)?;}}
    std::fs::write(path.with_file_name("terminal.json"),serde_json::to_vec(&serde_json::json!({"qpc":terminal,"expected":plan.point[plan.axis]}))?)?;
    let mut output=BufWriter::new(File::create(path)?);for record in records{crate::run::write_line(&mut output,&record)?;}output.flush()?;cpu.finish()?;result
}
