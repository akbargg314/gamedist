//! Implementations of the CLI commands.

use crate::{
    config::{self, normalize_server, Credentials, LaunchConfig, PlatformConfig, Project},
    Error, Result,
};
use futures_util::StreamExt;
use gamedist_core::{
    api::{CreateGameRequest, PatchGameRequest, ReleaseList},
    build::{scan_build, BuildScan},
    chunker::ChunkParams,
    client::ApiClient,
    ids,
    manifest::{Assets, GameManifest, KeyRotation, Platform, ReleaseManifest, SignedRelease},
    ChunkId, PublicKey, SecretKey,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

/// Where a command runs: the project directory, the per-user config
/// directory, and how to talk to the user.
pub struct Ctx {
    pub project_dir: PathBuf,
    pub config_dir: PathBuf,
    pub credentials: Credentials,
    /// Chunking parameters; the protocol defaults outside tests.
    pub chunk_params: ChunkParams,
    /// Print progress to stderr.
    pub verbose: bool,
}

impl Ctx {
    pub fn new(project_dir: PathBuf, config_dir: PathBuf) -> Self {
        Ctx {
            credentials: Credentials::new(&config_dir),
            project_dir,
            config_dir,
            chunk_params: ChunkParams::default(),
            verbose: true,
        }
    }

    fn say(&self, message: impl AsRef<str>) {
        if self.verbose {
            eprintln!("{}", message.as_ref());
        }
    }

    fn project(&self) -> Result<Project> {
        Project::load(&self.project_dir)
    }

    fn key(&self, project: &Project) -> Result<SecretKey> {
        config::load_key(&self.config_dir, &project.key)
    }

    /// A client for `server` carrying the stored token.
    async fn authed(&self, server: &str) -> Result<ApiClient> {
        let server = normalize_server(server);
        let token = self.credentials.get(&server).ok_or_else(|| {
            Error::msg(format!(
                "not logged in to {server}; run `gamedist login {server}`"
            ))
        })?;
        Ok(ApiClient::connect(&server).await?.with_token(token))
    }

    fn targets(&self, project: &Project, server: Option<&str>) -> Result<Vec<String>> {
        let servers = match server {
            Some(s) => vec![normalize_server(s)],
            None => project.servers(),
        };
        if servers.is_empty() {
            return Err(Error::msg(
                "no server configured; add one to `servers` in gamedist.toml or pass --server",
            ));
        }
        Ok(servers)
    }
}

// ---- init / keys -----------------------------------------------------------

pub struct InitOptions {
    pub slug: String,
    pub title: Option<String>,
    pub key: Option<String>,
    pub server: Option<String>,
}

/// Create `gamedist.toml` and (if it does not exist yet) the signing key.
pub fn init(ctx: &Ctx, opts: InitOptions) -> Result<(Project, PublicKey)> {
    ids::validate_slug(&opts.slug)?;
    if ctx.project_dir.join(config::PROJECT_FILE).exists() {
        return Err(Error::msg(format!(
            "{} already exists here",
            config::PROJECT_FILE
        )));
    }
    let key_name = opts.key.unwrap_or_else(|| opts.slug.clone());
    let key_path = config::key_path(&ctx.config_dir, &key_name)?;
    let key = if key_path.exists() {
        ctx.say(format!("using existing key {}", key_path.display()));
        config::load_key(&ctx.config_dir, &key_name)?
    } else {
        let key = SecretKey::generate();
        let path = config::save_key(&ctx.config_dir, &key_name, &key)?;
        ctx.say(format!(
            "generated signing key {}\nBACK THIS FILE UP. Whoever holds it can publish updates to your game; if you lose it, players must re-trust a new key.",
            path.display()
        ));
        key
    };
    let project = Project {
        title: opts.title.unwrap_or_else(|| opts.slug.clone()),
        slug: opts.slug,
        key: key_name,
        servers: opts.server.iter().map(|s| normalize_server(s)).collect(),
        platforms: BTreeMap::new(),
    };
    std::fs::create_dir_all(&ctx.project_dir)
        .map_err(|e| Error::msg(format!("creating {}: {e}", ctx.project_dir.display())))?;
    project.save(&ctx.project_dir)?;
    Ok((project, key.public()))
}

pub fn key_export(ctx: &Ctx, name: &str) -> Result<String> {
    Ok(config::load_key(&ctx.config_dir, name)?.to_encoded())
}

pub fn key_import(ctx: &Ctx, name: &str, encoded: &str) -> Result<PublicKey> {
    let key = SecretKey::from_encoded(encoded)?;
    config::save_key(&ctx.config_dir, name, &key)?;
    Ok(key.public())
}

pub fn key_public(ctx: &Ctx, name: &str) -> Result<PublicKey> {
    Ok(config::load_key(&ctx.config_dir, name)?.public())
}

// ---- login -----------------------------------------------------------------

pub struct LoginOptions {
    pub server: String,
    pub email: String,
    pub password: String,
    /// Create the account first.
    pub register: bool,
    pub invite: Option<String>,
}

pub async fn login(ctx: &Ctx, opts: LoginOptions) -> Result<&'static str> {
    let server = normalize_server(&opts.server);
    let client = ApiClient::connect(&server).await?;
    if opts.register {
        client
            .register(&opts.email, &opts.password, opts.invite.as_deref())
            .await?;
        ctx.say(format!(
            "registered {} on {}",
            opts.email,
            client.info().name
        ));
    }
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".into());
    let token = client
        .token(
            &opts.email,
            &opts.password,
            Some(&format!("gamedist cli @ {host}")),
        )
        .await?;
    let stored_in = ctx.credentials.set(&server, &token)?;
    ctx.say(format!(
        "logged in to {}; token stored in {stored_in}",
        client.info().name
    ));
    Ok(stored_in)
}

// ---- game create -----------------------------------------------------------

pub async fn game_create(
    ctx: &Ctx,
    server: Option<&str>,
    summary: &str,
    description_md: &str,
) -> Result<Vec<GameManifest>> {
    let mut project = ctx.project()?;
    let key = ctx.key(&project)?;
    let targets = ctx.targets(&project, server)?;
    let mut created = Vec::new();
    for server in &targets {
        let client = ctx.authed(server).await?;
        client.add_key(key.public(), Some(&project.key)).await?;
        let game = client
            .create_game(&CreateGameRequest {
                slug: project.slug.clone(),
                developer_pubkey: key.public(),
                title: project.title.clone(),
                summary: summary.to_string(),
                description_md: description_md.to_string(),
            })
            .await?;
        ctx.say(format!("created {server}/games/{}", project.slug));
        created.push(game);
        if !project.servers().contains(server) {
            project.servers.push(server.clone());
            project.save(&ctx.project_dir)?;
        }
    }
    Ok(created)
}

// ---- push ------------------------------------------------------------------

pub struct PushOptions {
    /// `(platform, build directory)`; a release carries all its platforms.
    pub builds: Vec<(String, PathBuf)>,
    pub version: String,
    /// Channel to point at this version after publishing.
    pub channel: Option<String>,
    pub changelog: Option<String>,
    pub server: Option<String>,
    /// Launch executable override for platforms missing from gamedist.toml.
    pub exe: Option<String>,
    pub concurrency: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushReport {
    pub server: String,
    /// Distinct chunks in the release.
    pub chunks_total: usize,
    pub chunks_uploaded: usize,
    pub bytes_uploaded: u64,
    /// `false` when this exact release was already published.
    pub created: bool,
}

fn platforms_equal(a: &BTreeMap<String, Platform>, b: &BTreeMap<String, Platform>) -> bool {
    a == b
}

pub async fn push(ctx: &Ctx, opts: PushOptions) -> Result<Vec<PushReport>> {
    let mut project = ctx.project()?;
    let key = ctx.key(&project)?;
    ids::parse_version(&opts.version)?;
    if let Some(channel) = &opts.channel {
        ids::validate_channel(channel)?;
    }
    if opts.builds.is_empty() {
        return Err(Error::msg("nothing to push: give a build directory"));
    }
    let targets = ctx.targets(&project, opts.server.as_deref())?;

    // 1. Scan every build directory (chunk + hash, parallel inside).
    let mut platforms = BTreeMap::new();
    let mut scans: Vec<BuildScan> = Vec::new();
    let mut project_changed = false;
    for (platform, dir) in &opts.builds {
        ids::validate_platform(platform)?;
        let launch = match (project.platforms.get(platform), &opts.exe) {
            (Some(cfg), _) => cfg.launch.clone(),
            (None, Some(exe)) => {
                let launch = LaunchConfig {
                    exe: exe.clone(),
                    args: vec![],
                    cwd: None,
                };
                project.platforms.insert(
                    platform.clone(),
                    PlatformConfig {
                        launch: launch.clone(),
                    },
                );
                project_changed = true;
                launch
            }
            (None, None) => {
                return Err(Error::msg(format!(
                    "no launch executable known for {platform}: add [platforms.{platform}] launch = {{ exe = \"…\" }} to gamedist.toml or pass --exe"
                )))
            }
        };
        ctx.say(format!("scanning {} ({platform})", dir.display()));
        let (dir, params) = (dir.clone(), ctx.chunk_params);
        let scan = tokio::task::spawn_blocking(move || scan_build(&dir, params))
            .await
            .map_err(|e| Error::msg(format!("scan task failed: {e}")))??;
        platforms.insert(
            platform.clone(),
            scan.clone().into_platform(launch.to_launch())?,
        );
        scans.push(scan);
    }
    if project_changed {
        project.save(&ctx.project_dir)?;
    }

    let mut manifest = ReleaseManifest::new(key.public(), &project.slug, &opts.version);
    manifest.changelog_md = opts.changelog.clone().unwrap_or_default();
    manifest.platforms = platforms;
    let chunks = manifest.unique_chunks();
    let mut signed: Option<SignedRelease> = None;

    let mut reports = Vec::new();
    for server in &targets {
        let client = ctx.authed(server).await?;

        // Releases are immutable. If this version is already there with the
        // same content, pushing again is a no-op (the timestamp inside the
        // signed manifest would otherwise differ on every run).
        let existing = match client.release_text(&project.slug, &opts.version).await {
            Ok(text) => Some(SignedRelease::parse_pinned(&text, &key.public())?),
            Err(gamedist_core::Error::Api { status: 404, .. }) => None,
            Err(e) => return Err(e.into()),
        };
        if let Some(existing) = &existing {
            if !platforms_equal(&existing.manifest().platforms, &manifest.platforms) {
                return Err(Error::msg(format!(
                    "{} {} is already published on {server} with different content; releases are immutable, bump the version",
                    project.slug, opts.version
                )));
            }
        }

        // 2. Ask which chunks the server lacks; 3. upload only those.
        let ids: Vec<ChunkId> = chunks.iter().map(|c| c.id).collect();
        let missing = client.missing_chunks(&ids).await?;
        let total_bytes: u64 = chunks
            .iter()
            .filter(|c| missing.contains(&c.id))
            .map(|c| c.size)
            .sum();
        ctx.say(format!(
            "{server}: {} of {} chunks to upload ({total_bytes} bytes)",
            missing.len(),
            chunks.len()
        ));
        let bar = progress_bar(ctx.verbose, total_bytes);
        let uploaded_bytes = Arc::new(AtomicU64::new(0));
        let mut uploads = futures_util::stream::iter(missing.iter().map(|id| {
            let client = client.clone();
            let source = scans
                .iter()
                .find_map(|scan| scan.sources.get(id))
                .cloned()
                .expect("every chunk came from a scan");
            let (bar, uploaded_bytes, id) = (bar.clone(), uploaded_bytes.clone(), *id);
            async move {
                let mut last = None;
                for attempt in 0..4u32 {
                    let src = source.clone();
                    let data = tokio::task::spawn_blocking(move || src.read(&id))
                        .await
                        .map_err(|e| Error::msg(format!("read task failed: {e}")))??;
                    let len = data.len() as u64;
                    match client.put_chunk(&id, data).await {
                        Ok(_) => {
                            uploaded_bytes.fetch_add(len, Ordering::Relaxed);
                            bar.inc(len);
                            return Ok(());
                        }
                        // Client errors will not get better by retrying.
                        Err(
                            e @ gamedist_core::Error::Api {
                                status: 400..=499 | 507,
                                ..
                            },
                        ) => return Err(Error::from(e)),
                        Err(e) => last = Some(e),
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(300 << attempt)).await;
                }
                Err(Error::from(last.expect("at least one attempt")))
            }
        }))
        .buffer_unordered(opts.concurrency.max(1));
        while let Some(result) = uploads.next().await {
            result?;
        }
        drop(uploads);
        bar.finish_and_clear();

        // 4. Sign once, publish.
        let created = match existing {
            Some(_) => false,
            None => {
                if signed.is_none() {
                    signed = Some(manifest.sign(&key)?);
                }
                client
                    .publish_release(&project.slug, signed.as_ref().expect("just set"))
                    .await?
            }
        };
        if let Some(channel) = &opts.channel {
            client
                .set_channel(&project.slug, channel, &opts.version)
                .await?;
        }
        ctx.say(format!(
            "{server}: {} {} {}{}",
            project.slug,
            opts.version,
            if created {
                "published"
            } else {
                "already published, nothing to do"
            },
            opts.channel
                .as_ref()
                .map(|c| format!("; channel {c} → {}", opts.version))
                .unwrap_or_default()
        ));
        reports.push(PushReport {
            server: server.clone(),
            chunks_total: chunks.len(),
            chunks_uploaded: missing.len(),
            bytes_uploaded: uploaded_bytes.load(Ordering::Relaxed),
            created,
        });
    }
    Ok(reports)
}

fn progress_bar(visible: bool, total: u64) -> indicatif::ProgressBar {
    if !visible || total == 0 {
        return indicatif::ProgressBar::hidden();
    }
    let bar = indicatif::ProgressBar::new(total);
    if let Ok(style) = indicatif::ProgressStyle::with_template(
        "{bar:32} {bytes}/{total_bytes} {bytes_per_sec} eta {eta}",
    ) {
        bar.set_style(style);
    }
    bar
}

// ---- promote / assets / status ---------------------------------------------

pub async fn promote(ctx: &Ctx, version: &str, channel: &str, server: Option<&str>) -> Result<()> {
    let project = ctx.project()?;
    ids::validate_channel(channel)?;
    for server in ctx.targets(&project, server)? {
        ctx.authed(&server)
            .await?
            .set_channel(&project.slug, channel, version)
            .await?;
        ctx.say(format!("{server}: channel {channel} → {version}"));
    }
    Ok(())
}

#[derive(Default)]
pub struct AssetOptions {
    pub icon: Option<PathBuf>,
    pub banner: Option<PathBuf>,
    /// `Some` replaces all screenshots.
    pub screenshots: Option<Vec<PathBuf>>,
    pub server: Option<String>,
}

pub async fn assets_set(ctx: &Ctx, opts: AssetOptions) -> Result<Assets> {
    let project = ctx.project()?;
    let read = |path: &PathBuf| -> Result<(ChunkId, Vec<u8>)> {
        let data = std::fs::read(path)
            .map_err(|e| Error::msg(format!("reading {}: {e}", path.display())))?;
        if data.is_empty() || data.len() as u64 > gamedist_core::MAX_CHUNK_SIZE {
            return Err(Error::msg(format!(
                "{}: assets must be between 1 byte and 8 MiB",
                path.display()
            )));
        }
        Ok((ChunkId::of(&data), data))
    };
    let icon = opts.icon.as_ref().map(read).transpose()?;
    let banner = opts.banner.as_ref().map(read).transpose()?;
    let screenshots = opts
        .screenshots
        .as_ref()
        .map(|list| list.iter().map(read).collect::<Result<Vec<_>>>())
        .transpose()?;

    let mut last = Assets::default();
    for server in ctx.targets(&project, opts.server.as_deref())? {
        let client = ctx.authed(&server).await?;
        let mut assets = client.game(&project.slug).await?.assets;
        for (id, data) in icon
            .iter()
            .chain(banner.iter())
            .chain(screenshots.iter().flatten())
        {
            client.put_chunk(id, data.clone()).await?;
        }
        if let Some((id, _)) = &icon {
            assets.icon = Some(*id);
        }
        if let Some((id, _)) = &banner {
            assets.banner = Some(*id);
        }
        if let Some(list) = &screenshots {
            assets.screenshots = list.iter().map(|(id, _)| *id).collect();
        }
        last = client
            .patch_game(
                &project.slug,
                &PatchGameRequest {
                    assets: Some(assets),
                    ..Default::default()
                },
            )
            .await?
            .assets;
        ctx.say(format!("{server}: assets updated"));
    }
    Ok(last)
}

#[derive(Debug)]
pub struct ServerStatus {
    pub server: String,
    /// `Err` holds a human-readable reason (unreachable, game missing…).
    pub releases: std::result::Result<ReleaseList, String>,
    pub logged_in: bool,
}

#[derive(Debug)]
pub struct Status {
    pub project: Project,
    pub pubkey: PublicKey,
    pub game_id: String,
    pub servers: Vec<ServerStatus>,
}

pub async fn status(ctx: &Ctx) -> Result<Status> {
    let project = ctx.project()?;
    let pubkey = ctx.key(&project)?.public();
    let mut servers = Vec::new();
    for server in project.servers() {
        let releases = match ApiClient::connect(&server).await {
            Ok(client) => client
                .releases(&project.slug)
                .await
                .map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        servers.push(ServerStatus {
            logged_in: ctx.credentials.get(&server).is_some(),
            server,
            releases,
        });
    }
    Ok(Status {
        game_id: gamedist_core::game_id(&pubkey, &project.slug),
        pubkey,
        project,
        servers,
    })
}

// ---- verify ----------------------------------------------------------------

/// Offline schema + structure + signature check of a release manifest file.
pub fn verify(path: &Path, pinned: Option<&PublicKey>) -> Result<ReleaseManifest> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| Error::msg(format!("reading {}: {e}", path.display())))?;
    let release = match pinned {
        Some(key) => SignedRelease::parse_pinned(&text, key)?,
        None => SignedRelease::parse(&text)?,
    };
    Ok(release.into_manifest())
}

// ---- mirror ----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorReport {
    pub versions: Vec<String>,
    pub chunks_copied: usize,
    pub bytes_copied: u64,
}

/// Re-publish signed releases from the project's first server to `target`.
/// No signing key is needed: the releases are copied exactly as signed.
pub async fn mirror(ctx: &Ctx, target: &str, version: Option<&str>) -> Result<MirrorReport> {
    let mut project = ctx.project()?;
    let target = normalize_server(target);
    let source_url = project
        .servers()
        .into_iter()
        .find(|s| *s != target)
        .ok_or_else(|| Error::msg("no source server in gamedist.toml to mirror from"))?;
    let source = ApiClient::connect(&source_url).await?;
    let dest = ctx.authed(&target).await?;

    let game = source.game(&project.slug).await?;
    let pubkey = game.developer_pubkey;
    dest.add_key(pubkey, Some("mirrored")).await?;
    match dest.game(&project.slug).await {
        Ok(existing) if existing.developer_pubkey != pubkey => {
            return Err(Error::msg(format!(
                "{target} already has a different game called {} (other developer key)",
                project.slug
            )))
        }
        Ok(_) => {}
        Err(gamedist_core::Error::Api { status: 404, .. }) => {
            dest.create_game(&CreateGameRequest {
                slug: game.slug.clone(),
                developer_pubkey: pubkey,
                title: game.title.clone(),
                summary: game.summary.clone(),
                description_md: game.description_md.clone(),
            })
            .await?;
            ctx.say(format!("created {target}/games/{}", game.slug));
        }
        Err(e) => return Err(e.into()),
    }

    let mut report = MirrorReport {
        versions: Vec::new(),
        chunks_copied: 0,
        bytes_copied: 0,
    };
    let copy = |ids: Vec<ChunkId>| {
        let (source, dest) = (source.clone(), dest.clone());
        async move {
            let missing = dest.missing_chunks(&ids).await?;
            let mut bytes = 0u64;
            let mut copies = futures_util::stream::iter(missing.iter().map(|id| {
                let (source, dest) = (source.clone(), dest.clone());
                async move {
                    let data = source.chunk(id).await?;
                    let len = data.len() as u64;
                    dest.put_chunk(id, data).await?;
                    Ok::<u64, Error>(len)
                }
            }))
            .buffer_unordered(8);
            while let Some(result) = copies.next().await {
                bytes += result?;
            }
            Ok::<(usize, u64), Error>((missing.len(), bytes))
        }
    };

    // Store-page assets.
    let asset_ids: Vec<ChunkId> = game.assets.chunk_ids().copied().collect();
    if !asset_ids.is_empty() {
        let (n, bytes) = copy(asset_ids).await?;
        report.chunks_copied += n;
        report.bytes_copied += bytes;
        dest.patch_game(
            &game.slug,
            &PatchGameRequest {
                assets: Some(game.assets.clone()),
                ..Default::default()
            },
        )
        .await?;
    }

    let list = source.releases(&project.slug).await?;
    let versions: Vec<String> = match version {
        Some(v) => vec![v.to_string()],
        // Oldest first, so the mirror's history reads the same.
        None => list
            .releases
            .iter()
            .rev()
            .map(|r| r.version.clone())
            .collect(),
    };
    for version in &versions {
        let release = source.release(&project.slug, version, &pubkey).await?;
        let ids = release
            .manifest()
            .unique_chunks()
            .iter()
            .map(|c| c.id)
            .collect();
        let (n, bytes) = copy(ids).await?;
        report.chunks_copied += n;
        report.bytes_copied += bytes;
        dest.publish_release(&project.slug, &release).await?;
        ctx.say(format!(
            "{target}: mirrored {} {version} ({n} chunks copied)",
            project.slug
        ));
        report.versions.push(version.clone());
    }
    for (channel, version) in &list.channels {
        if versions.contains(version) {
            dest.set_channel(&project.slug, channel, version).await?;
        }
    }
    if !project.servers().contains(&target) {
        project.servers.push(target);
        project.save(&ctx.project_dir)?;
    }
    Ok(report)
}

// ---- key rotation ----------------------------------------------------------

/// Rotate the project's signing key: generate (or reuse) `new_key_name`, sign
/// a rotation statement with the old key, submit it to every server and
/// switch `gamedist.toml` to the new key.
pub async fn key_rotate(ctx: &Ctx, new_key_name: &str) -> Result<(PublicKey, serde_json::Value)> {
    let mut project = ctx.project()?;
    let old = ctx.key(&project)?;
    if new_key_name == project.key {
        return Err(Error::msg("the new key must have a different name"));
    }
    let new = if config::key_path(&ctx.config_dir, new_key_name)?.exists() {
        config::load_key(&ctx.config_dir, new_key_name)?
    } else {
        let key = SecretKey::generate();
        config::save_key(&ctx.config_dir, new_key_name, &key)?;
        key
    };
    let statement = KeyRotation::create(&old, new.public(), &project.slug)?;
    for server in project.servers() {
        let client = ctx.authed(&server).await?;
        client.add_key(new.public(), Some(new_key_name)).await?;
        client.post_rotation(&project.slug, &statement).await?;
        ctx.say(format!(
            "{server}: key rotated; publish a new release signed with the new key"
        ));
    }
    project.key = new_key_name.to_string();
    project.save(&ctx.project_dir)?;
    Ok((new.public(), statement))
}
