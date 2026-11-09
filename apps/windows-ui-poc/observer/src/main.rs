#[cfg(windows)] mod launch;
#[cfg(windows)] mod capture;
#[cfg(windows)] mod inject;
#[cfg(windows)] mod workloads;
#[cfg(windows)] mod run;

fn main() {
    #[cfg(windows)]
    if let Err(error) = run::main() { eprintln!("{error:#}"); std::process::exit(1); }
    #[cfg(not(windows))]
    { eprintln!("poc-observer requires an interactive Windows desktop"); std::process::exit(1); }
}
