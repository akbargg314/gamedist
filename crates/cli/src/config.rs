//! Project file (`gamedist.toml`), key files and credential storage.

use crate::{Error, Result};
use gamedist_core::{manifest::Launch, SecretKey};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub const PROJECT_FILE: &str = "gamedist.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchConfig {
    pub exe: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

impl LaunchConfig {
    pub fn to_launch(&self) -> Launch {
        Launch {
            exe: self.exe.clone(),
            args: self.args.clone(),
            cwd: self.cwd.clone().unwrap_or_else(|| ".".into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformConfig {
    pub launch: LaunchConfig,
}

/// `gamedist.toml`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub slug: String,
    pub title: String,
    /// Name of the signing key in the key directory.
    pub key: String,
    #[serde(default)]
    pub servers: Vec<String>,
    #[serde(default)]
    pub platforms: BTreeMap<String, PlatformConfig>,
}

impl Project {
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(PROJECT_FILE);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::msg(format!(
                    "no {PROJECT_FILE} in {}; run `gamedist init` first",
                    dir.display()
                ))
            } else {
                Error::msg(format!("reading {}: {e}", path.display()))
            }
        })?;
        let project: Project =
            toml::from_str(&text).map_err(|e| Error::msg(format!("{}: {e}", path.display())))?;
        gamedist_core::ids::validate_slug(&project.slug)?;
        for name in project.platforms.keys() {
            gamedist_core::ids::validate_platform(name)?;
        }
        Ok(project)
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        let path = dir.join(PROJECT_FILE);
        let text = toml::to_string_pretty(self).map_err(|e| Error::msg(e.to_string()))?;
        std::fs::write(&path, text)
            .map_err(|e| Error::msg(format!("writing {}: {e}", path.display())))
    }

    pub fn servers(&self) -> Vec<String> {
        self.servers.iter().map(|s| normalize_server(s)).collect()
    }
}

pub fn normalize_server(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

/// `~/.config/gamedist` (or `$GAMEDIST_CONFIG_DIR`).
pub fn default_config_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("GAMEDIST_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    dirs::config_dir()
        .map(|d| d.join("gamedist"))
        .ok_or_else(|| Error::msg("cannot determine the config directory; set GAMEDIST_CONFIG_DIR"))
}

fn validate_key_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(Error::msg(format!(
            "key name {name:?} may only contain letters, digits, '-' and '_'"
        )))
    }
}

pub fn key_path(config_dir: &Path, name: &str) -> Result<PathBuf> {
    validate_key_name(name)?;
    Ok(config_dir.join("keys").join(format!("{name}.key")))
}

/// Write a file that only the current user can read.
pub fn write_private(path: &Path, contents: &str) -> Result<()> {
    let err = |e: std::io::Error| Error::msg(format!("writing {}: {e}", path.display()));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    #[cfg(unix)]
    {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(err)?;
        file.write_all(contents.as_bytes()).map_err(err)?;
    }
    #[cfg(not(unix))]
    std::fs::write(path, contents).map_err(err)?;
    Ok(())
}

pub fn load_key(config_dir: &Path, name: &str) -> Result<SecretKey> {
    let path = key_path(config_dir, name)?;
    let text = std::fs::read_to_string(&path).map_err(|e| {
        Error::msg(format!(
            "cannot read signing key {name:?} at {}: {e}",
            path.display()
        ))
    })?;
    Ok(SecretKey::from_encoded(&text)?)
}

/// Save a key; refuses to overwrite an existing one.
pub fn save_key(config_dir: &Path, name: &str, key: &SecretKey) -> Result<PathBuf> {
    let path = key_path(config_dir, name)?;
    if path.exists() {
        return Err(Error::msg(format!(
            "a key named {name:?} already exists at {}",
            path.display()
        )));
    }
    write_private(&path, &format!("{}\n", key.to_encoded()))?;
    Ok(path)
}

/// API token storage: OS keyring when available, otherwise a 0600 file.
/// `GAMEDIST_TOKEN` overrides everything (for CI).
pub struct Credentials {
    file: PathBuf,
    use_keyring: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CredentialFile {
    #[serde(default)]
    tokens: BTreeMap<String, String>,
}

impl Credentials {
    pub fn new(config_dir: &Path) -> Self {
        let disabled = std::env::var("GAMEDIST_TOKEN_STORE").is_ok_and(|v| v == "file");
        Credentials {
            file: config_dir.join("credentials.toml"),
            use_keyring: cfg!(feature = "keyring") && !disabled,
        }
    }

    /// Never use the OS keyring (tests, headless machines).
    pub fn file_only(config_dir: &Path) -> Self {
        Credentials {
            file: config_dir.join("credentials.toml"),
            use_keyring: false,
        }
    }

    fn read_file(&self) -> CredentialFile {
        std::fs::read_to_string(&self.file)
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn get(&self, server: &str) -> Option<String> {
        if let Ok(token) = std::env::var("GAMEDIST_TOKEN") {
            if !token.is_empty() {
                return Some(token);
            }
        }
        if self.use_keyring {
            if let Some(token) = keyring_get(server) {
                return Some(token);
            }
        }
        self.read_file().tokens.get(server).cloned()
    }

    /// Returns where the token ended up, for the user's information.
    pub fn set(&self, server: &str, token: &str) -> Result<&'static str> {
        if self.use_keyring && keyring_set(server, token) {
            // Do not leave an older copy lying around in the file.
            let mut file = self.read_file();
            if file.tokens.remove(server).is_some() {
                self.write_file(&file)?;
            }
            return Ok("the OS keyring");
        }
        let mut file = self.read_file();
        file.tokens.insert(server.to_string(), token.to_string());
        self.write_file(&file)?;
        Ok("the credentials file (no OS keyring available)")
    }

    fn write_file(&self, file: &CredentialFile) -> Result<()> {
        let text = toml::to_string(file).map_err(|e| Error::msg(e.to_string()))?;
        write_private(&self.file, &text)
    }
}

#[cfg(feature = "keyring")]
fn keyring_get(server: &str) -> Option<String> {
    let server = server.to_string();
    // The secret-service backend drives its own runtime; keep it off ours.
    std::thread::spawn(move || {
        keyring::Entry::new("gamedist", &server)
            .ok()?
            .get_password()
            .ok()
    })
    .join()
    .ok()
    .flatten()
}

#[cfg(feature = "keyring")]
fn keyring_set(server: &str, token: &str) -> bool {
    let (server, token) = (server.to_string(), token.to_string());
    std::thread::spawn(move || {
        keyring::Entry::new("gamedist", &server)
            .and_then(|entry| entry.set_password(&token))
            .is_ok()
    })
    .join()
    .unwrap_or(false)
}

#[cfg(not(feature = "keyring"))]
fn keyring_get(_server: &str) -> Option<String> {
    None
}

#[cfg(not(feature = "keyring"))]
fn keyring_set(_server: &str, _token: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_roundtrip_matches_documented_format() {
        let text = r#"
slug = "cool-game"
title = "Cool Game"
key = "cool-game"
servers = ["https://host.example/"]

[platforms.windows-x86_64]
launch = { exe = "CoolGame.exe" }
[platforms.linux-x86_64]
launch = { exe = "CoolGame.x86_64", args = ["--fullscreen"] }
"#;
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(PROJECT_FILE), text).unwrap();
        let project = Project::load(tmp.path()).unwrap();
        assert_eq!(project.servers(), ["https://host.example"]);
        assert_eq!(
            project.platforms["linux-x86_64"].launch.to_launch().cwd,
            "."
        );
        project.save(tmp.path()).unwrap();
        assert_eq!(Project::load(tmp.path()).unwrap(), project);

        std::fs::write(
            tmp.path().join(PROJECT_FILE),
            text.replace("cool-game\"\ntitle", "Bad Slug\"\ntitle"),
        )
        .unwrap();
        assert!(Project::load(tmp.path()).is_err());
        assert!(Project::load(&tmp.path().join("missing"))
            .unwrap_err()
            .to_string()
            .contains("gamedist init"));
    }

    #[test]
    fn keys_are_private_and_not_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let key = SecretKey::generate();
        let path = save_key(tmp.path(), "main", &key).unwrap();
        assert_eq!(load_key(tmp.path(), "main").unwrap().public(), key.public());
        assert!(save_key(tmp.path(), "main", &SecretKey::generate()).is_err());
        assert!(key_path(tmp.path(), "../escape").is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let _ = path;
    }

    #[test]
    fn file_credentials() {
        let tmp = tempfile::tempdir().unwrap();
        let creds = Credentials::file_only(tmp.path());
        assert_eq!(creds.get("https://a.example"), None);
        creds.set("https://a.example", "gd_token").unwrap();
        creds.set("https://b.example", "gd_other").unwrap();
        assert_eq!(creds.get("https://a.example").as_deref(), Some("gd_token"));
        assert_eq!(creds.get("https://b.example").as_deref(), Some("gd_other"));
    }
}
