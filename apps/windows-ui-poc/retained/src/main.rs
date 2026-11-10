#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
mod native;
#[cfg(windows)]
mod render;
mod scene;
#[cfg(windows)]
mod text;
#[cfg(windows)]
fn main() {
    if let Err(error) = native::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
#[cfg(not(windows))]
fn main() {
    eprintln!("poc-retained requires Windows");
}
