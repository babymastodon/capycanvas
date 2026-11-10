use crate::{cpu::ForeignCpu,run::{Result,qpc,frequency}};
use serde::Deserialize;
use std::{collections::HashSet,fs::{self,File},path::{Path,PathBuf},time::{Duration,Instant}};
use windows::{core::PWSTR,Win32::{Foundation::*,System::Threading::*,UI::WindowsAndMessaging::GetWindowThreadProcessId}};
#[derive(Deserialize)]
struct Plan {executable:String,registry_directory:PathBuf,stop:PathBuf,#[serde(default)]profile:bool}
fn registries(directory:&Path)->Result<Vec<PathBuf>> {
    Ok(fs::read_dir(directory)?.filter_map(|entry|entry.ok()).map(|entry|entry.path()).filter(|p|p.file_name().and_then(|n|n.to_str()).is_some_and(|n|n.starts_with("windows-")&&n.ends_with(".json"))).collect())
}
pub fn run(plan:&Path,out:&Path)->Result<()> {
    let plan:Plan=serde_json::from_reader(File::open(plan)?)?;fs::create_dir(out)?;
    let old:HashSet<_>=registries(&plan.registry_directory)?.into_iter().collect();
    fs::write(out.join("ready.json"),b"{\"ready\":true}")?;let deadline=Instant::now()+Duration::from_secs(240);
    let found=(||->Result<(u32,u32)>{loop{
        if plan.stop.exists()||Instant::now()>deadline{return Err("No newly launched owned pen window before monitor deadline".into());}
        for path in registries(&plan.registry_directory)?.into_iter().filter(|p|!old.contains(p)){
            let registry:serde_json::Value=serde_json::from_reader(File::open(&path)?)?;
            let pid=registry["process_id"].as_u64().ok_or("registry has no process ID")? as u32;
            for window in registry["windows"].as_array().ok_or("registry has no windows")?{
                if window["ready"]!=true{continue;}
                let hwnd=HWND(window["hwnd"].as_u64().ok_or("registry has no HWND")? as usize as _);let mut owner=0;
                let tid=unsafe{GetWindowThreadProcessId(hwnd,Some(&mut owner))};if owner!=pid||tid==0{continue;}
                let handle=unsafe{OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,false,pid)?};let mut buffer=vec![0u16;32768];let mut length=buffer.len() as u32;
                let queried=unsafe{QueryFullProcessImageNameW(handle,PROCESS_NAME_WIN32,PWSTR(buffer.as_mut_ptr()),&mut length)};unsafe{let _=CloseHandle(handle);}queried?;
                if String::from_utf16_lossy(&buffer[..length as usize]).eq_ignore_ascii_case(&plan.executable){return Ok((pid,tid));}
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }})();
    let (pid,tid)=found?;let sampler=ForeignCpu::new(pid,tid)?;let process=unsafe{OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION|PROCESS_SYNCHRONIZE,false,pid)?};
    let marker=if plan.profile{Some(crate::profile::Marker::new()?)}else{None};let begin=marker.as_ref().map(|m|m.write("pen-begin")).transpose()?;
    let mut samples=Vec::new();let result=(||->Result<()>{
        while !plan.stop.exists()&&Instant::now()<deadline{
            if unsafe{WaitForSingleObject(process,0)}==WAIT_OBJECT_0{break;}
            match sampler.sample(){Ok(sample)=>samples.push(sample),Err(error)=>{if unsafe{WaitForSingleObject(process,0)}!=WAIT_OBJECT_0{return Err(error);}}}
            std::thread::sleep(Duration::from_millis(100));
        }Ok(())
    })();let end=marker.as_ref().map(|m|m.write("pen-end")).transpose()?;unsafe{let _=CloseHandle(process);}
    fs::write(out.join("profile-markers.json"),serde_json::to_vec(&serde_json::json!({"begin":begin,"end":end}))?)?;
    fs::write(out.join("cpu.json"),serde_json::to_vec(&samples)?)?;
    fs::write(out.join("meta.json"),serde_json::to_vec_pretty(&serde_json::json!({"process_id":pid,"ui_thread_id":tid,"qpc_frequency":frequency(),"finished_qpc":qpc(),"failure":result.as_ref().err().map(ToString::to_string)}))?)?;result
}
