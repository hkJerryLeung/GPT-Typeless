// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--diagnose-permissions") {
        if let Err(error) = gptservice_tauri_lib::diagnose_permissions() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if args.get(1).map(String::as_str) == Some("--diagnose-insertion") {
        if let Err(error) = gptservice_tauri_lib::diagnose_insertion() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if args.get(1).map(String::as_str) == Some("--diagnose-audio") {
        let hold_ms = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2000);
        let directory = args
            .get(3)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("gpttypeless-audio-diagnostic"));
        if let Err(error) = gptservice_tauri_lib::diagnose_audio(hold_ms, directory) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    gptservice_tauri_lib::run()
}
