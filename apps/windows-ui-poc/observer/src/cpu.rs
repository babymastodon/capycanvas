use std::{fs::OpenOptions, io::Write, path::{Path, PathBuf}, sync::Mutex};
use windows::Win32::{Foundation::FILETIME, System::{Performance::{QueryPerformanceCounter, QueryPerformanceFrequency}, Threading::{GetCurrentThread, GetCurrentThreadId, GetThreadPriority, GetThreadTimes, SetThreadPriority, THREAD_PRIORITY}}};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
static OUTPUT: Mutex<()> = Mutex::new(());
fn qpc() -> i64 { let mut value = 0; unsafe { QueryPerformanceCounter(&mut value).expect("QPC"); } value }
fn cpu_ms() -> Result<f64> {
    let (mut created, mut exited, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetThreadTimes(GetCurrentThread(), &mut created, &mut exited, &mut kernel, &mut user)?; }
    let ticks = |time: FILETIME| ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64;
    Ok((ticks(kernel) + ticks(user)) as f64 / 10_000.0)
}
pub struct ThreadCpu { name: &'static str, path: PathBuf, start: i64, frequency: i64, baseline: f64, last: i64, priority: i32, restore: Option<THREAD_PRIORITY>, records: Vec<serde_json::Value>, finished: bool }
impl ThreadCpu {
    pub fn new(name: &'static str, path: &Path, priority: Option<THREAD_PRIORITY>) -> Result<Self> {
        let original = THREAD_PRIORITY(unsafe { GetThreadPriority(GetCurrentThread()) });
        if let Some(priority) = priority { unsafe { SetThreadPriority(GetCurrentThread(), priority)?; } }
        let mut frequency = 0; unsafe { QueryPerformanceFrequency(&mut frequency)?; }
        let now = qpc();
        Ok(Self { name, path: path.to_owned(), start: now, frequency, baseline: cpu_ms()?, last: now, priority: priority.unwrap_or(original).0, restore: priority.map(|_| original), records: Vec::new(), finished: false })
    }
    fn record(&mut self, now: i64, final_record: bool) -> Result<()> {
        self.records.push(serde_json::json!({"thread":self.name,"thread_id":unsafe{GetCurrentThreadId()},"qpc":now,"cpu_ms":cpu_ms()? - self.baseline,"wall_ms":(now-self.start) as f64*1000.0/self.frequency as f64,"priority":self.priority,"final":final_record}));
        self.last = now; Ok(())
    }
    pub fn sample(&mut self) -> Result<()> { let now = qpc(); if now - self.last >= self.frequency { self.record(now, false)?; } Ok(()) }
    pub fn finish(&mut self) -> Result<()> {
        if self.finished { return Ok(()); }
        self.record(qpc(), true)?;
        if let Some(priority) = self.restore.take() { unsafe { SetThreadPriority(GetCurrentThread(), priority)?; } }
        self.finished = true;
        let _lock = OUTPUT.lock().map_err(|_| "observer CPU output lock poisoned")?;
        let mut file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        for record in &self.records { serde_json::to_writer(&mut file, record)?; file.write_all(b"\n")?; }
        file.flush()?; Ok(())
    }
}
impl Drop for ThreadCpu { fn drop(&mut self) { if let Err(error) = self.finish() { eprintln!("observer CPU record failed for {}: {error}",self.name); } } }
