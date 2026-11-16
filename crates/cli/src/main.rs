use clap::{Args, Parser, Subcommand};
use gamedist_cli::{
    commands::{self, AssetOptions, Ctx, InitOptions, LoginOptions, PushOptions},
    config, Error, Result,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "gamedist",
    version,
    about = "Publish games to GameDist distributor servers"
)]
struct Cli {
    /// Project directory (where gamedist.toml lives).
    #[arg(long, short = 'C', global = true, default_value = ".")]
    dir: PathBuf,
    /// Less output.
    #[arg(long, short, global = true)]
    quiet: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create gamedist.toml and generate a signing key.
    Init {
        slug: String,
        #[arg(long)]
        title: Option<String>,
        /// Key name (defaults to the slug).
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Manage signing keys.
    #[command(subcommand)]
    Key(KeyCommand),
    /// Log in to a server and store an API token.
    Login(LoginArgs),
    /// Manage the game's registration on servers.
    #[command(subcommand)]
    Game(GameCommand),
    /// Chunk, upload and publish a build.
    Push(PushArgs),
    /// Point a channel at a published version.
    Promote {
        version: String,
        #[arg(long, default_value = "stable")]
        channel: String,
        #[arg(long)]
        server: Option<String>,
    },
    /// Manage store-page images.
    #[command(subcommand)]
    Assets(AssetsCommand),
    /// Re-publish signed releases to another server.
    Mirror {
        server: String,
        /// Only this version (default: all).
        #[arg(long)]
        version: Option<String>,
    },
    /// Show the project, its key and what each server has.
    Status,
    /// Inspect published releases.
    #[command(subcommand)]
    Releases(ReleasesCommand),
    /// Check a release manifest file offline (schema, structure, signature).
    Verify {
        file: PathBuf,
        /// Also require this developer key (ed25519:…).
        #[arg(long)]
        key: Option<String>,
    },
}

#[derive(Subcommand)]
enum KeyCommand {
    /// Print the public key.
    Show { name: Option<String> },
    /// Print the PRIVATE key for backup. Treat the output like a password.
    Export { name: String },
    /// Import a private key from a file (or `-` for standard input).
    Import { name: String, file: PathBuf },
    /// Rotate the project to a new signing key.
    Rotate {
        /// Name for the new key.
        new_name: String,
    },
}

#[derive(Args)]
struct LoginArgs {
    server: String,
    #[arg(long, env = "GAMEDIST_EMAIL")]
    email: Option<String>,
    /// Create the account first.
    #[arg(long)]
    register: bool,
    #[arg(long)]
    invite: Option<String>,
}

#[derive(Subcommand)]
enum GameCommand {
    /// Register the game (and your public key) on a server.
    Create {
        #[arg(long)]
        server: Option<String>,
        #[arg(long, default_value = "")]
        summary: String,
        /// Markdown file with the store description.
        #[arg(long)]
        description: Option<PathBuf>,
    },
}

#[derive(Args)]
struct PushArgs {
    /// Build directory (with --platform).
    build_dir: Option<PathBuf>,
    #[arg(long)]
    platform: Option<String>,
    /// Additional builds as PLATFORM=DIR; a release contains all its platforms.
    #[arg(long = "build", value_name = "PLATFORM=DIR")]
    builds: Vec<String>,
    #[arg(long)]
    version: String,
    /// Channel to promote to after publishing.
    #[arg(long)]
    channel: Option<String>,
    /// Markdown changelog file.
    #[arg(long)]
    changelog: Option<PathBuf>,
    #[arg(long)]
    server: Option<String>,
    /// Launch executable, relative to the build dir (saved to gamedist.toml).
    #[arg(long)]
    exe: Option<String>,
    #[arg(long, default_value_t = 8)]
    jobs: usize,
}

#[derive(Subcommand)]
enum AssetsCommand {
    /// Upload and set images (each at most 8 MiB).
    Set {
        #[arg(long)]
        icon: Option<PathBuf>,
        #[arg(long)]
        banner: Option<PathBuf>,
        #[arg(long, num_args = 1..)]
        screenshots: Option<Vec<PathBuf>>,
        #[arg(long)]
        server: Option<String>,
    },
}

#[derive(Subcommand)]
enum ReleasesCommand {
    /// List versions and channels on every configured server.
    List,
}

fn read_text(path: &PathBuf) -> Result<String> {
    std::fs::read_to_string(path)
        .map_err(|e| Error::msg(format!("reading {}: {e}", path.display())))
}

fn prompt(label: &str) -> Result<String> {
    use std::io::Write;
    eprint!("{label}: ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| Error::msg(e.to_string()))?;
    Ok(line.trim().to_string())
}

fn print_status(status: &commands::Status) {
    println!("{} ({})", status.project.title, status.project.slug);
    println!("  game id  {}", status.game_id);
    println!("  key      {} ({})", status.pubkey, status.project.key);
    for server in &status.servers {
        println!(
            "  {}{}",
            server.server,
            if server.logged_in {
                ""
            } else {
                "  (not logged in)"
            }
        );
        match &server.releases {
            Ok(list) => {
                for (channel, version) in &list.channels {
                    println!("      channel {channel} → {version}");
                }
                for release in &list.releases {
                    println!(
                        "      {}  {}  {}",
                        release.version,
                        release.created,
                        release.platforms.join(", ")
                    );
                }
                if list.releases.is_empty() {
                    println!("      no releases");
                }
            }
            Err(reason) => println!("      {reason}"),
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let mut ctx = Ctx::new(cli.dir.clone(), config::default_config_dir()?);
    ctx.verbose = !cli.quiet;
    match cli.command {
        Command::Init {
            slug,
            title,
            key,
            server,
        } => {
            let (project, pubkey) = commands::init(
                &ctx,
                InitOptions {
                    slug,
                    title,
                    key,
                    server,
                },
            )?;
            println!("created {} for {}", config::PROJECT_FILE, project.slug);
            println!("public key: {pubkey}");
        }
        Command::Key(KeyCommand::Show { name }) => {
            let name = match name {
                Some(name) => name,
                None => config::Project::load(&ctx.project_dir)?.key,
            };
            println!("{}", commands::key_public(&ctx, &name)?);
        }
        Command::Key(KeyCommand::Export { name }) => {
            println!("{}", commands::key_export(&ctx, &name)?);
        }
        Command::Key(KeyCommand::Import { name, file }) => {
            let encoded = if file.as_os_str() == "-" {
                std::io::read_to_string(std::io::stdin()).map_err(|e| Error::msg(e.to_string()))?
            } else {
                read_text(&file)?
            };
            println!("imported {}", commands::key_import(&ctx, &name, &encoded)?);
        }
        Command::Key(KeyCommand::Rotate { new_name }) => {
            let (pubkey, _) = commands::key_rotate(&ctx, &new_name).await?;
            println!("project now signs with {pubkey}");
        }
        Command::Login(args) => {
            let email = match args.email {
                Some(email) => email,
                None => prompt("Email")?,
            };
            let password = match std::env::var("GAMEDIST_PASSWORD") {
                Ok(password) => password,
                Err(_) => rpassword::prompt_password("Password: ")
                    .map_err(|e| Error::msg(format!("reading password: {e}")))?,
            };
            commands::login(
                &ctx,
                LoginOptions {
                    server: args.server,
                    email,
                    password,
                    register: args.register,
                    invite: args.invite,
                },
            )
            .await?;
        }
        Command::Game(GameCommand::Create {
            server,
            summary,
            description,
        }) => {
            let description = description.as_ref().map(read_text).transpose()?;
            for game in commands::game_create(
                &ctx,
                server.as_deref(),
                &summary,
                description.as_deref().unwrap_or(""),
            )
            .await?
            {
                println!("game id {}", game.game_id);
            }
        }
        Command::Push(args) => {
            let mut builds = Vec::new();
            match (args.build_dir, args.platform) {
                (Some(dir), Some(platform)) => builds.push((platform, dir)),
                (Some(dir), None) => builds.push((gamedist_core::ids::host_platform(), dir)),
                (None, Some(_)) => {
                    return Err(Error::msg("--platform needs a build directory"));
                }
                (None, None) => {}
            }
            for spec in &args.builds {
                let (platform, dir) = spec.split_once('=').ok_or_else(|| {
                    Error::msg(format!("--build expects PLATFORM=DIR, got {spec:?}"))
                })?;
                builds.push((platform.to_string(), PathBuf::from(dir)));
            }
            let changelog = args.changelog.as_ref().map(read_text).transpose()?;
            let reports = commands::push(
                &ctx,
                PushOptions {
                    builds,
                    version: args.version,
                    channel: args.channel,
                    changelog,
                    server: args.server,
                    exe: args.exe,
                    concurrency: args.jobs,
                },
            )
            .await?;
            for report in reports {
                println!(
                    "{}: uploaded {} of {} chunks ({} bytes){}",
                    report.server,
                    report.chunks_uploaded,
                    report.chunks_total,
                    report.bytes_uploaded,
                    if report.created {
                        ""
                    } else {
                        " — release already existed"
                    }
                );
            }
        }
        Command::Promote {
            version,
            channel,
            server,
        } => commands::promote(&ctx, &version, &channel, server.as_deref()).await?,
        Command::Assets(AssetsCommand::Set {
            icon,
            banner,
            screenshots,
            server,
        }) => {
            commands::assets_set(
                &ctx,
                AssetOptions {
                    icon,
                    banner,
                    screenshots,
                    server,
                },
            )
            .await?;
        }
        Command::Mirror { server, version } => {
            let report = commands::mirror(&ctx, &server, version.as_deref()).await?;
            println!(
                "mirrored {} release(s), copied {} chunks ({} bytes)",
                report.versions.len(),
                report.chunks_copied,
                report.bytes_copied
            );
        }
        Command::Status | Command::Releases(ReleasesCommand::List) => {
            print_status(&commands::status(&ctx).await?);
        }
        Command::Verify { file, key } => {
            let pinned = key.map(|k| k.parse()).transpose()?;
            let manifest = commands::verify(&file, pinned.as_ref())?;
            println!(
                "OK: {} {} signed by {}",
                manifest.slug, manifest.version, manifest.developer_pubkey
            );
            for (name, platform) in &manifest.platforms {
                println!(
                    "  {name}: {} files, {} bytes, launches {}",
                    platform.files.len(),
                    platform.total_size,
                    platform.launch.exe
                );
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(e) = run(Cli::parse()).await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
