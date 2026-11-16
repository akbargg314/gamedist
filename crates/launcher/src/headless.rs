//! Headless command-line front end (`gamedist-launcher --headless …`), used
//! for CI, servers and integration tests.

use crate::{Error, InstallOptions, Launcher, Result};
use clap::{Args, Parser, Subcommand};
use gamedist_core::progress::{Phase, Progress, ProgressFn, Source};
use std::{
    io::Write,
    path::PathBuf,
    process::ExitCode,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

#[derive(Parser)]
#[command(
    name = "gamedist-launcher",
    version,
    about = "GameDist launcher (headless mode)"
)]
pub struct Cli {
    /// Run without the graphical interface.
    #[arg(long, global = true)]
    pub headless: bool,
    /// Launcher data directory (library, key pins, chunk store).
    #[arg(long, global = true, env = "GAMEDIST_DATA_DIR")]
    pub data_dir: Option<PathBuf>,
    /// Print results as JSON on standard output.
    #[arg(long, global = true)]
    pub json: bool,
    /// No progress output.
    #[arg(long, short, global = true)]
    pub quiet: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Args)]
pub struct InstallArgs {
    /// Game URL (https://host/games/slug or gamedist://…), slug or game ID.
    pub game: String,
    /// Install directory (default: <install root>/<slug>).
    #[arg(long)]
    pub dir: Option<PathBuf>,
    #[arg(long)]
    pub platform: Option<String>,
    #[arg(long)]
    pub channel: Option<String>,
    /// Install exactly this version.
    #[arg(long)]
    pub version: Option<String>,
    #[arg(long)]
    pub allow_downgrade: bool,
    /// Trust a changed developer key. Dangerous: only after verifying the
    /// new key with the developer out of band.
    #[arg(long)]
    pub accept_key_change: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// Add a game to the library without installing it.
    Add {
        url: String,
        #[arg(long)]
        accept_key_change: bool,
    },
    /// Add (if needed) and install a game.
    Install(InstallArgs),
    /// Update an installed game to its channel's current version.
    Update {
        game: String,
        #[arg(long)]
        allow_downgrade: bool,
    },
    /// Show whether an update is available and how much it would download.
    Check { game: String },
    /// Re-hash installed files; re-download damaged ones unless --no-repair.
    Verify {
        game: String,
        #[arg(long)]
        no_repair: bool,
    },
    /// Start a game and wait for it to exit; its exit code becomes ours.
    Launch {
        game: String,
        /// Arguments passed to the game.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// List library entries.
    List,
    /// Remove a game's files; with --forget also its library entry and key pin.
    Uninstall {
        game: String,
        #[arg(long)]
        forget: bool,
    },
    /// Manage catalog servers.
    #[command(subcommand)]
    Catalog(CatalogCommand),
    /// Search the configured catalogs.
    Discover {
        #[arg(default_value = "")]
        query: String,
    },
    /// Show or change settings.
    Settings {
        #[arg(long)]
        install_root: Option<PathBuf>,
        /// Download limit in bytes per second (0 = unlimited).
        #[arg(long)]
        bandwidth_limit: Option<u64>,
        #[arg(long)]
        concurrency: Option<usize>,
        #[arg(long)]
        keep_chunks: Option<bool>,
        /// Wine-compatible runner for Windows builds.
        #[arg(long)]
        wine: Option<PathBuf>,
        /// direct, torrent-direct or torrent.
        #[arg(long)]
        transport: Option<String>,
        /// Seed installed games to other players.
        #[arg(long)]
        seed: Option<bool>,
        /// Upload limit in bytes per second (0 = unlimited).
        #[arg(long)]
        upload_limit: Option<u64>,
        /// BitTorrent listen port (0 = any free port).
        #[arg(long)]
        listen_port: Option<u16>,
    },
    /// Seed installed games until interrupted (Ctrl-C).
    Seed,
}

#[derive(Subcommand)]
pub enum CatalogCommand {
    Add { server: String },
    Remove { server: String },
    List,
}

/// Progress printer: one line per percent step on stderr.
fn progress_printer(quiet: bool) -> ProgressFn {
    if quiet {
        return gamedist_core::progress::no_progress();
    }
    let last = Arc::new(AtomicU64::new(u64::MAX));
    Arc::new(move |p: Progress| {
        if p.total == 0 || p.phase == Phase::Done {
            return;
        }
        let percent = p.done * 100 / p.total;
        let source = p.source.map_or(0, |s| s as u64 + 1);
        let key = ((p.phase as u64) << 16) | (source << 8) | (percent / 5);
        if last.swap(key, Ordering::Relaxed) != key {
            let phase = match p.phase {
                Phase::Harvest => "reusing installed data",
                Phase::Download => match p.source {
                    Some(Source::Peers) => "downloading from peers",
                    _ => "downloading",
                },
                Phase::Assemble => "installing",
                Phase::Verify => "verifying",
                Phase::Done => "done",
            };
            eprintln!("{phase}: {percent}% ({} / {} bytes)", p.done, p.total);
        }
    })
}

fn print<T: serde::Serialize>(json: bool, value: &T, human: impl FnOnce()) -> Result<()> {
    if json {
        let text = serde_json::to_string_pretty(value).map_err(gamedist_core::Error::from)?;
        println!("{text}");
    } else {
        human();
    }
    Ok(())
}

/// Run a parsed command line. Returns the process exit code.
pub async fn run(cli: Cli) -> Result<u8> {
    let data_dir = match cli.data_dir {
        Some(dir) => dir,
        None => Launcher::default_data_dir()?,
    };
    let launcher = Launcher::open(data_dir)?;
    let progress = progress_printer(cli.quiet || cli.json);
    let json = cli.json;
    match cli.command {
        Command::Add {
            url,
            accept_key_change,
        } => {
            let entry = launcher.add_game(&url, accept_key_change).await?;
            print(json, &entry, || {
                println!("added {} ({})", entry.title, entry.game_id);
                println!("pinned developer key {}", entry.pinned_key);
            })?;
        }
        Command::Install(args) => {
            // A URL adds the game (pinning its key on first sight); a slug or
            // game ID must already be in the library.
            let selector = if args.game.contains("://") {
                launcher
                    .add_game(&args.game, args.accept_key_change)
                    .await?
                    .game_id
            } else {
                args.game.clone()
            };
            let report = launcher
                .install(
                    &selector,
                    InstallOptions {
                        dir: args.dir,
                        platform: args.platform,
                        channel: args.channel,
                        version: args.version,
                        allow_downgrade: args.allow_downgrade,
                    },
                    progress,
                )
                .await?;
            print(json, &report, || {
                println!(
                    "installed {} ({}) to {}; downloaded {} bytes in {} chunks",
                    report.version,
                    report.platform,
                    report.dir.display(),
                    report.downloaded_bytes,
                    report.downloaded_chunks
                );
            })?;
        }
        Command::Update {
            game,
            allow_downgrade,
        } => {
            let report = launcher
                .install(
                    &game,
                    InstallOptions {
                        allow_downgrade,
                        ..Default::default()
                    },
                    progress,
                )
                .await?;
            print(json, &report, || {
                if report.up_to_date {
                    println!("already up to date ({})", report.version);
                } else {
                    println!(
                        "updated {} → {}; downloaded {} bytes, reused {} bytes, rewrote {} files",
                        report.previous_version.as_deref().unwrap_or("none"),
                        report.version,
                        report.downloaded_bytes,
                        report.harvested_bytes,
                        report.files_written
                    );
                }
            })?;
        }
        Command::Check { game } => {
            let info = launcher.check_update(&game).await?;
            print(json, &info, || {
                if info.update_available {
                    println!(
                        "update available: {} → {} ({} bytes to download)",
                        info.installed.as_deref().unwrap_or("not installed"),
                        info.latest,
                        info.download_bytes
                    );
                } else {
                    println!("up to date ({})", info.latest);
                }
            })?;
        }
        Command::Verify { game, no_repair } => {
            let report = launcher.verify(&game, !no_repair, progress).await?;
            print(json, &report, || {
                if report.bad_files.is_empty() {
                    println!("all {} files are intact", report.checked_files);
                } else {
                    println!(
                        "{} damaged file(s){}:",
                        report.bad_files.len(),
                        if report.repaired { ", repaired" } else { "" }
                    );
                    for path in &report.bad_files {
                        println!("  {path}");
                    }
                }
            })?;
            if !report.bad_files.is_empty() && !report.repaired {
                return Ok(2);
            }
        }
        Command::Launch { game, args } => {
            let mut child = launcher.launch(&game, &args)?;
            let status = child
                .wait()
                .map_err(|e| Error::msg(format!("waiting for the game: {e}")))?;
            let _ = std::io::stdout().flush();
            return Ok(status.code().map(|c| c.clamp(0, 255) as u8).unwrap_or(1));
        }
        Command::List => {
            let games = launcher.games();
            print(json, &games, || {
                for game in &games {
                    match &game.installed {
                        Some(i) => println!(
                            "{}  {}  {} ({})  {}",
                            game.game_id,
                            game.slug,
                            i.version,
                            i.platform,
                            i.dir.display()
                        ),
                        None => println!("{}  {}  not installed", game.game_id, game.slug),
                    }
                }
            })?;
        }
        Command::Uninstall { game, forget } => {
            launcher.uninstall(&game, forget).await?;
            if !json {
                println!("uninstalled {game}");
            }
        }
        Command::Catalog(CatalogCommand::Add { server }) => {
            let name = launcher.add_catalog(&server).await?;
            if !json {
                println!("added catalog {name}");
            }
        }
        Command::Catalog(CatalogCommand::Remove { server }) => launcher.remove_catalog(&server)?,
        Command::Catalog(CatalogCommand::List) => {
            let catalogs = launcher.catalogs();
            print(json, &catalogs, || {
                for catalog in &catalogs {
                    println!("{catalog}");
                }
            })?;
        }
        Command::Discover { query } => {
            let (found, errors) = launcher.discover(&query).await;
            for error in &errors {
                eprintln!("warning: {error}");
            }
            print(json, &found, || {
                for entry in &found {
                    println!(
                        "{}  —  {}  ({})",
                        entry.game.url, entry.game.title, entry.server_name
                    );
                }
            })?;
        }
        Command::Settings {
            install_root,
            bandwidth_limit,
            concurrency,
            keep_chunks,
            wine,
            transport,
            seed,
            upload_limit,
            listen_port,
        } => {
            let transport = transport
                .map(|t| match t.as_str() {
                    "direct" => Ok(crate::Transport::Direct),
                    "torrent-direct" => Ok(crate::Transport::TorrentDirect),
                    "torrent" => Ok(crate::Transport::Torrent),
                    other => Err(Error::msg(format!(
                        "unknown transport {other:?} (direct, torrent-direct, torrent)"
                    ))),
                })
                .transpose()?;
            let settings = launcher.update_settings(|s| {
                if let Some(transport) = transport {
                    s.transport = transport;
                }
                if let Some(seed) = seed {
                    s.seed = seed;
                }
                if let Some(limit) = upload_limit {
                    s.upload_limit = (limit > 0).then_some(limit);
                }
                if let Some(port) = listen_port {
                    s.listen_port = port;
                }
                if let Some(root) = install_root {
                    s.install_root = root;
                }
                if let Some(limit) = bandwidth_limit {
                    s.bandwidth_limit = (limit > 0).then_some(limit);
                }
                if let Some(n) = concurrency {
                    s.concurrency = n.clamp(1, 64);
                }
                if let Some(keep) = keep_chunks {
                    s.keep_chunks = keep;
                }
                if let Some(wine) = wine {
                    s.wine = Some(wine);
                }
            })?;
            let text =
                serde_json::to_string_pretty(&settings).map_err(gamedist_core::Error::from)?;
            println!("{text}");
        }
        Command::Seed => {
            launcher.update_settings(|s| s.seed = true)?;
            for (slug, reason) in launcher.start_seeding().await {
                eprintln!("warning: not seeding {slug}: {reason}");
            }
            let seeding = launcher.seeding();
            if seeding.is_empty() {
                return Err(Error::msg(
                    "nothing to seed: no installed game could be seeded",
                ));
            }
            if !json {
                println!(
                    "seeding {} game(s) on port {}; press Ctrl-C to stop",
                    seeding.len(),
                    launcher.torrent_port().unwrap_or(0)
                );
            }
            let _ = tokio::signal::ctrl_c().await;
            print(json, &launcher.seeding(), || {
                for status in launcher.seeding() {
                    println!(
                        "{}: uploaded {} bytes",
                        status.game_id, status.uploaded_bytes
                    );
                }
            })?;
        }
    }
    launcher.shutdown().await;
    Ok(0)
}

/// Parse `args` (including the program name) and run.
pub async fn run_from<I, T>(args: I) -> Result<u8>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = Cli::try_parse_from(args).map_err(|e| Error::msg(e.to_string()))?;
    run(cli).await
}

/// Entry point for binaries: parses the process arguments, runs on a fresh
/// Tokio runtime and maps errors to exit code 1.
pub fn main_from_args() -> ExitCode {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("error: starting the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(cli)) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
