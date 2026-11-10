fn main() {
    #[cfg(windows)] if let Err(error)=windows_ui_motion::run::main(){eprintln!("{error}");std::process::exit(1);}
    #[cfg(not(windows))] { eprintln!("Capture requires Windows; probe tests run on every host.");std::process::exit(1); }
}
