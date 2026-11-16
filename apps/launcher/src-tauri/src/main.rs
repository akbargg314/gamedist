// GameDist launcher: Tauri GUI shell around the `gamedist-launcher` engine.
//
// `gamedist-launcher --headless <command> …` runs the engine's command-line
// front end instead of the GUI (CI, integration tests, servers without a
// display).

// Hide the console window for GUI launches of release builds on Windows.
// (Headless mode still returns a correct exit code; redirect output to a file
// if you need its logs there, or use a debug build.)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use std::process::ExitCode;

fn main() -> ExitCode {
    if std::env::args().skip(1).any(|a| a == "--headless") {
        return gamedist_launcher::headless::main_from_args();
    }
    match commands::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("gamedist-launcher: {e}");
            ExitCode::FAILURE
        }
    }
}
