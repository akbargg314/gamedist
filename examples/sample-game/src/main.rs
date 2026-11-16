//! A tiny terminal game ("guess the number") used to exercise GameDist end to
//! end. `--selftest` checks that the installed tree is complete and exits 0,
//! which is what the integration tests launch.

use std::{
    io::{BufRead, Write},
    path::PathBuf,
    process::ExitCode,
};

/// Game data lives next to the executable, as installed by the launcher.
fn data_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent().map(|p| p.join("data")).unwrap_or_default()
}

fn read_data(name: &str) -> Result<String, String> {
    let path = data_dir().join(name);
    std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
}

fn selftest() -> Result<String, String> {
    let version = read_data("version.txt")?.trim().to_string();
    if version.is_empty() {
        return Err("data/version.txt is empty".into());
    }
    let words = read_data("words.txt")?;
    if words.lines().count() < 3 {
        return Err("data/words.txt is incomplete".into());
    }
    Ok(version)
}

/// Small deterministic generator so the game needs no dependencies.
fn pseudo_random(seed: u64) -> u64 {
    let mut x = seed ^ 0x9E37_79B9_7F4A_7C15;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

fn play() -> ExitCode {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(42);
    let secret = pseudo_random(seed) % 100 + 1;
    println!("I am thinking of a number between 1 and 100.");
    let stdin = std::io::stdin();
    for attempt in 1.. {
        print!("Guess #{attempt}: ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
            println!("\nBye!");
            return ExitCode::SUCCESS;
        }
        match line.trim().parse::<u64>() {
            Ok(guess) if guess == secret => {
                println!("Correct, in {attempt} guesses!");
                return ExitCode::SUCCESS;
            }
            Ok(guess) if guess < secret => println!("Higher."),
            Ok(_) => println!("Lower."),
            Err(_) => println!("Please type a number."),
        }
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--selftest") {
        return match selftest() {
            Ok(version) => {
                println!("sample-game {version} selftest ok");
                ExitCode::SUCCESS
            }
            Err(reason) => {
                eprintln!("selftest failed: {reason}");
                ExitCode::from(3)
            }
        };
    }
    if args.iter().any(|a| a == "--version") {
        let version = read_data("version.txt").unwrap_or_else(|_| "unknown".into());
        println!("{}", version.trim());
        return ExitCode::SUCCESS;
    }
    play()
}
