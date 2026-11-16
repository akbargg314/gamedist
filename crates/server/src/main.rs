use clap::{Parser, Subcommand};
use gamedist_server::{api, auth, jobs, Config, Server};
use std::{io::Read, path::PathBuf};

#[derive(Parser)]
#[command(
    name = "gamedist-server",
    version,
    about = "GameDist distributor server"
)]
struct Cli {
    /// Path to server.toml (optional; GAMEDIST_* environment variables override it).
    #[arg(long, short, global = true, env = "GAMEDIST_CONFIG")]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server (default).
    Serve,
    /// Create an account directly in the database (works with closed registration).
    /// The password is read from GAMEDIST_PASSWORD or standard input.
    UserAdd {
        #[arg(long)]
        email: String,
        /// Make the account an administrator.
        #[arg(long)]
        admin: bool,
    },
    /// Run chunk garbage collection once and exit.
    Gc {
        /// Ignore the grace period and delete every unreferenced chunk.
        #[arg(long)]
        now: bool,
    },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    if let Err(message) = run(Cli::parse()).await {
        eprintln!("error: {message}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    let cfg = Config::load(cli.config.as_deref())?;
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => {
            if cfg.public_url.is_empty() {
                tracing::warn!(
                    "public_url is not set; URLs will use the listen address (local testing only)"
                );
            }
            let server = Server::bind(cfg).await?;
            tracing::info!("listening on {} as {}", server.addr, server.url());
            server
                .run(async {
                    shutdown_signal().await;
                    tracing::info!("shutting down");
                })
                .await
                .map_err(|e| e.to_string())
        }
        Command::UserAdd { email, admin } => {
            let password = match std::env::var("GAMEDIST_PASSWORD") {
                Ok(p) => p,
                Err(_) => {
                    eprintln!("reading password from standard input…");
                    let mut input = String::new();
                    std::io::stdin()
                        .read_to_string(&mut input)
                        .map_err(|e| e.to_string())?;
                    input.trim_end_matches(['\r', '\n']).to_string()
                }
            };
            let mut cfg = cfg;
            // Only the database is needed; do not fight a running server for the port.
            cfg.listen = "127.0.0.1:0".parse().expect("valid address");
            let server = Server::bind(cfg).await?;
            let email = auth::validate_email(&email).map_err(|e| e.detail)?;
            auth::validate_password(&password).map_err(|e| e.detail)?;
            let hash = auth::hash_password(password).await.map_err(|e| e.detail)?;
            api::insert_user(&server.state, &email, &hash, admin)
                .await
                .map_err(|e| e.detail)?;
            gamedist_server::db::audit(
                &server.state.db,
                None,
                "user.add",
                &email,
                if admin { "admin" } else { "" },
            )
            .await;
            println!(
                "created {}{email}",
                if admin { "administrator " } else { "" }
            );
            Ok(())
        }
        Command::Gc { now } => {
            let mut cfg = cfg;
            cfg.listen = "127.0.0.1:0".parse().expect("valid address");
            let grace = if now {
                0
            } else {
                cfg.gc_grace_days as i64 * 86_400
            };
            let server = Server::bind(cfg).await?;
            let (chunks, bytes) = jobs::collect_garbage(&server.state, grace)
                .await
                .map_err(|e| e.to_string())?;
            println!("removed {chunks} chunks ({bytes} bytes)");
            Ok(())
        }
    }
}

/// Resolves on Ctrl-C, or SIGTERM on Unix (systemd, Docker).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
