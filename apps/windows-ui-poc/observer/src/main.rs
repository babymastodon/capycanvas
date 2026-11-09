#[cfg(windows)] mod launch;
#[cfg(windows)] mod capture;
#[cfg(windows)] mod inject;
#[cfg(windows)] mod workloads;
#[cfg(windows)] mod run;
#[cfg(any(windows,test))] mod snapshot;

fn main() {
    #[cfg(windows)]
    if let Err(error) = if std::env::args().nth(1).as_deref()==Some("snapshot") {snapshot::run()}else{run::main()} { eprintln!("{error:#}"); std::process::exit(1); }
    #[cfg(not(windows))]
    { eprintln!("poc-observer requires an interactive Windows desktop"); std::process::exit(1); }
}
