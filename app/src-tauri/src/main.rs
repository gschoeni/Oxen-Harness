// Prevents an extra console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Err(error) = oxen_harness_app_lib::run() {
        eprintln!("Could not start oxen-harness desktop app: {error}");
        std::process::exit(1);
    }
}
