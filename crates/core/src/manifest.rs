//! Protocol objects: server info, game manifest, signed release manifest and
//! key-rotation statements.

use crate::{
    ids::{self, game_id},
    keys::{sign_object, verify_object, PublicKey, SecretKey},
    paths,
    schema::{self, Schema},
    ChunkId, Error, Result, MAX_CHUNK_SIZE, PROTOCOL,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Registration {
    Open,
    Invite,
    #[default]
    Closed,
}

impl Registration {
    pub fn as_str(self) -> &'static str {
        match self {
            Registration::Open => "open",
            Registration::Invite => "invite",
            Registration::Closed => "closed",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "open" => Ok(Registration::Open),
            "invite" => Ok(Registration::Invite),
            "closed" => Ok(Registration::Closed),
            other => Err(Error::invalid("registration mode", other.to_string())),
        }
    }
}

/// `GET /.well-known/gamedist.json`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub protocol: String,
    pub name: String,
    pub api: String,
    pub chunks: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    pub registration: Registration,
}

impl ServerInfo {
    pub fn parse(text: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(text)?;
        schema::validate(Schema::ServerInfo, &value)?;
        Ok(serde_json::from_value(value)?)
    }

    pub fn chunk_url(&self, id: &ChunkId) -> String {
        format!("{}{}", self.chunks, id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Assets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<ChunkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banner: Option<ChunkId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub screenshots: Vec<ChunkId>,
}

impl Assets {
    pub fn chunk_ids(&self) -> impl Iterator<Item = &ChunkId> {
        self.icon
            .iter()
            .chain(self.banner.iter())
            .chain(self.screenshots.iter())
    }
}

/// `GET /games/<slug>/manifest.json` — server-generated, unsigned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameManifest {
    pub protocol: String,
    pub game_id: String,
    pub slug: String,
    pub developer_pubkey: PublicKey,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description_md: String,
    #[serde(default)]
    pub assets: Assets,
    #[serde(default)]
    pub channels: BTreeMap<String, String>,
    pub releases: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rotations: Vec<Value>,
}

impl GameManifest {
    /// Parse, schema-check and check the game ID derivation.
    pub fn parse(text: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(text)?;
        schema::validate(Schema::GameManifest, &value)?;
        let m: GameManifest = serde_json::from_value(value)?;
        m.validate()?;
        Ok(m)
    }

    pub fn validate(&self) -> Result<()> {
        ids::validate_slug(&self.slug)?;
        if self.game_id != game_id(&self.developer_pubkey, &self.slug) {
            return Err(Error::invalid(
                "game manifest",
                "game_id does not match developer_pubkey + slug",
            ));
        }
        for (channel, version) in &self.channels {
            ids::validate_channel(channel)?;
            ids::parse_version(version)?;
        }
        Ok(())
    }

    pub fn release_url(&self, version: &str) -> String {
        format!("{}{}.json", self.releases, version)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChunkRef {
    pub id: ChunkId,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub mode: u32,
    pub chunks: Vec<ChunkRef>,
}

impl FileEntry {
    pub fn is_executable(&self) -> bool {
        self.mode & 0o111 != 0
    }

    /// The only two modes ever applied to disk.
    pub fn effective_mode(&self) -> u32 {
        if self.is_executable() {
            0o755
        } else {
            0o644
        }
    }

    /// Two entries with the same chunk list have identical content.
    pub fn same_content(&self, other: &FileEntry) -> bool {
        self.size == other.size && self.chunks == other.chunks
    }
}

fn default_cwd() -> String {
    ".".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Launch {
    pub exe: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_cwd")]
    pub cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Platform {
    pub launch: Launch,
    pub files: Vec<FileEntry>,
    pub total_size: u64,
}

impl Platform {
    /// Every distinct chunk, in first-use order.
    pub fn unique_chunks(&self) -> Vec<ChunkRef> {
        let mut seen = HashSet::new();
        self.files
            .iter()
            .flat_map(|f| f.chunks.iter())
            .filter(|c| seen.insert(c.id))
            .copied()
            .collect()
    }

    pub fn file(&self, path: &str) -> Option<&FileEntry> {
        self.files.iter().find(|f| f.path == path)
    }

    pub fn validate(&self) -> Result<()> {
        let mut seen = HashSet::new();
        let mut total: u64 = 0;
        for file in &self.files {
            paths::validate_rel_path(&file.path)?;
            if !seen.insert(file.path.to_lowercase()) {
                return Err(Error::invalid(
                    "release manifest",
                    format!("duplicate path (case-insensitive): {}", file.path),
                ));
            }
            let mut sum: u64 = 0;
            for chunk in &file.chunks {
                if chunk.size == 0 || chunk.size > MAX_CHUNK_SIZE {
                    return Err(Error::invalid(
                        "release manifest",
                        format!("chunk {} has illegal size {}", chunk.id, chunk.size),
                    ));
                }
                sum = sum
                    .checked_add(chunk.size)
                    .ok_or_else(|| Error::invalid("release manifest", "size overflow"))?;
            }
            if sum != file.size {
                return Err(Error::invalid(
                    "release manifest",
                    format!(
                        "{}: size {} does not equal the sum of its chunks ({sum})",
                        file.path, file.size
                    ),
                ));
            }
            total = total
                .checked_add(file.size)
                .ok_or_else(|| Error::invalid("release manifest", "size overflow"))?;
        }
        // A file may not also be used as a directory by another entry.
        for file in &self.files {
            let lower = file.path.to_lowercase();
            let mut prefix = String::new();
            for comp in lower.split('/') {
                if !prefix.is_empty() && seen.contains(&prefix) {
                    return Err(Error::invalid(
                        "release manifest",
                        format!("{} is used both as a file and a directory", prefix),
                    ));
                }
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(comp);
            }
        }
        if total != self.total_size {
            return Err(Error::invalid(
                "release manifest",
                format!(
                    "total_size {} does not equal the sum of file sizes ({total})",
                    self.total_size
                ),
            ));
        }
        paths::validate_rel_path(&self.launch.exe)?;
        if self.file(&self.launch.exe).is_none() {
            return Err(Error::invalid(
                "release manifest",
                format!("launch.exe {:?} is not in the file list", self.launch.exe),
            ));
        }
        if self.launch.cwd != "." {
            paths::validate_rel_path(&self.launch.cwd)?;
        }
        Ok(())
    }
}

/// A release manifest body. Use [`SignedRelease`] for anything that came off
/// the network.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseManifest {
    pub protocol: String,
    pub game_id: String,
    pub slug: String,
    pub version: String,
    pub created: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub changelog_md: String,
    pub platforms: BTreeMap<String, Platform>,
    pub developer_pubkey: PublicKey,
}

impl ReleaseManifest {
    pub fn new(pubkey: PublicKey, slug: &str, version: &str) -> Self {
        ReleaseManifest {
            protocol: PROTOCOL.to_string(),
            game_id: game_id(&pubkey, slug),
            slug: slug.to_string(),
            version: version.to_string(),
            created: crate::now_rfc3339(),
            changelog_md: String::new(),
            platforms: BTreeMap::new(),
            developer_pubkey: pubkey,
        }
    }

    /// Structural rules of SPEC §4.3 (everything except the signature).
    pub fn validate(&self) -> Result<()> {
        if self.protocol != PROTOCOL {
            return Err(Error::invalid(
                "release manifest",
                format!("unsupported protocol {:?}", self.protocol),
            ));
        }
        ids::validate_slug(&self.slug)?;
        ids::parse_version(&self.version)?;
        if self.game_id != game_id(&self.developer_pubkey, &self.slug) {
            return Err(Error::invalid(
                "release manifest",
                "game_id does not match developer_pubkey + slug",
            ));
        }
        if self.platforms.is_empty() {
            return Err(Error::invalid("release manifest", "no platforms"));
        }
        for (name, platform) in &self.platforms {
            ids::validate_platform(name)?;
            platform.validate()?;
        }
        Ok(())
    }

    /// Every distinct chunk across all platforms.
    pub fn unique_chunks(&self) -> Vec<ChunkRef> {
        let mut seen = HashSet::new();
        self.platforms
            .values()
            .flat_map(|p| p.files.iter())
            .flat_map(|f| f.chunks.iter())
            .filter(|c| seen.insert(c.id))
            .copied()
            .collect()
    }

    /// Validate and sign.
    pub fn sign(&self, key: &SecretKey) -> Result<SignedRelease> {
        if key.public() != self.developer_pubkey {
            return Err(Error::invalid(
                "release manifest",
                "signing key does not match developer_pubkey",
            ));
        }
        self.validate()?;
        let mut value = serde_json::to_value(self)?;
        sign_object(key, &mut value)?;
        let text = serde_json::to_string_pretty(&value)?;
        Ok(SignedRelease {
            text,
            manifest: self.clone(),
        })
    }
}

/// A release manifest whose schema, structure and signature have been checked.
#[derive(Debug, Clone, PartialEq)]
pub struct SignedRelease {
    text: String,
    manifest: ReleaseManifest,
}

impl SignedRelease {
    /// Parse and fully verify a manifest. The signature is checked against the
    /// `developer_pubkey` **inside** the manifest; callers that have a pinned
    /// key must use [`SignedRelease::parse_pinned`] (or compare
    /// `manifest().developer_pubkey` themselves).
    pub fn parse(text: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(text)?;
        schema::validate(Schema::ReleaseManifest, &value)?;
        let manifest: ReleaseManifest = serde_json::from_value(value.clone())?;
        manifest.validate()?;
        verify_object(&manifest.developer_pubkey, &value)?;
        Ok(SignedRelease {
            text: text.to_string(),
            manifest,
        })
    }

    /// Parse and verify against a key the caller already trusts.
    pub fn parse_pinned(text: &str, pinned: &PublicKey) -> Result<Self> {
        let release = Self::parse(text)?;
        if release.manifest.developer_pubkey != *pinned {
            return Err(Error::BadSignature);
        }
        Ok(release)
    }

    /// The exact bytes that were signed over / received.
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn manifest(&self) -> &ReleaseManifest {
        &self.manifest
    }

    pub fn into_manifest(self) -> ReleaseManifest {
        self.manifest
    }
}

/// A statement, signed by `old_key`, that `new_key` succeeds it for `slug`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyRotation {
    pub protocol: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub slug: String,
    pub old_key: PublicKey,
    pub new_key: PublicKey,
    pub created: String,
}

impl KeyRotation {
    pub fn create(old: &SecretKey, new_key: PublicKey, slug: &str) -> Result<Value> {
        ids::validate_slug(slug)?;
        let statement = KeyRotation {
            protocol: PROTOCOL.to_string(),
            kind: "key-rotation".to_string(),
            slug: slug.to_string(),
            old_key: old.public(),
            new_key,
            created: crate::now_rfc3339(),
        };
        let mut value = serde_json::to_value(&statement)?;
        sign_object(old, &mut value)?;
        Ok(value)
    }

    /// Schema-check and verify the signature (made with `old_key`).
    pub fn verify(value: &Value) -> Result<Self> {
        schema::validate(Schema::KeyRotation, value)?;
        let statement: KeyRotation = serde_json::from_value(value.clone())?;
        if statement.old_key == statement.new_key {
            return Err(Error::invalid("key rotation", "old and new key are equal"));
        }
        verify_object(&statement.old_key, value)?;
        Ok(statement)
    }
}

/// Is there a chain of valid rotation statements for `slug` leading from
/// `pinned` to `target`? Invalid statements are ignored.
pub fn rotation_chain_links(
    slug: &str,
    pinned: &PublicKey,
    target: &PublicKey,
    statements: &[Value],
) -> bool {
    let valid: Vec<KeyRotation> = statements
        .iter()
        .filter_map(|v| KeyRotation::verify(v).ok())
        .filter(|s| s.slug == slug)
        .collect();
    let mut current = *pinned;
    let mut visited = vec![current];
    loop {
        if current == *target {
            return true;
        }
        match valid
            .iter()
            .find(|s| s.old_key == current && !visited.contains(&s.new_key))
        {
            Some(next) => {
                current = next.new_key;
                visited.push(current);
            }
            None => return false,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample_platform(files: &[(&str, &[u8])]) -> Platform {
        let entries: Vec<FileEntry> = files
            .iter()
            .map(|(path, data)| FileEntry {
                path: path.to_string(),
                size: data.len() as u64,
                mode: 0o755,
                chunks: if data.is_empty() {
                    vec![]
                } else {
                    vec![ChunkRef {
                        id: ChunkId::of(data),
                        size: data.len() as u64,
                    }]
                },
            })
            .collect();
        Platform {
            launch: Launch {
                exe: files[0].0.to_string(),
                args: vec![],
                cwd: ".".into(),
            },
            total_size: entries.iter().map(|f| f.size).sum(),
            files: entries,
        }
    }

    fn sample(key: &SecretKey) -> ReleaseManifest {
        let mut m = ReleaseManifest::new(key.public(), "cool-game", "1.2.0");
        m.platforms.insert(
            "linux-x86_64".into(),
            sample_platform(&[("game.bin", b"binary"), ("data/a.txt", b"hello")]),
        );
        m
    }

    #[test]
    fn sign_parse_roundtrip() {
        let key = SecretKey::generate();
        let signed = sample(&key).sign(&key).unwrap();
        let parsed = SignedRelease::parse(signed.text()).unwrap();
        assert_eq!(parsed.manifest(), signed.manifest());
        SignedRelease::parse_pinned(signed.text(), &key.public()).unwrap();
        assert!(matches!(
            SignedRelease::parse_pinned(signed.text(), &SecretKey::generate().public()),
            Err(Error::BadSignature)
        ));
    }

    #[test]
    fn tampering_is_detected() {
        let key = SecretKey::generate();
        let signed = sample(&key).sign(&key).unwrap();
        let tampered = signed.text().replace("game.bin", "game.bix");
        assert!(SignedRelease::parse(&tampered).is_err());

        // Swapping in an attacker's key (and re-deriving game_id) still fails
        // for a client that has the original key pinned.
        let attacker = SecretKey::generate();
        let forged = sample(&attacker).sign(&attacker).unwrap();
        SignedRelease::parse(forged.text()).unwrap();
        assert!(SignedRelease::parse_pinned(forged.text(), &key.public()).is_err());
    }

    #[test]
    fn wrong_signing_key_is_refused() {
        let key = SecretKey::generate();
        assert!(sample(&key).sign(&SecretKey::generate()).is_err());
    }

    #[test]
    fn structural_rules() {
        let key = SecretKey::generate();
        let good = sample(&key);
        good.validate().unwrap();

        let mut m = good.clone();
        m.game_id = "a".repeat(26);
        assert!(m.validate().is_err(), "game id mismatch");

        let mut m = good.clone();
        m.version = "1.2".into();
        assert!(m.validate().is_err(), "bad version");

        let mut m = good.clone();
        m.platforms.get_mut("linux-x86_64").unwrap().files[1].path = "../evil".into();
        assert!(m.validate().is_err(), "path traversal");

        let mut m = good.clone();
        m.platforms.get_mut("linux-x86_64").unwrap().files[1].path = "GAME.BIN".into();
        assert!(m.validate().is_err(), "case-insensitive duplicate");

        let mut m = good.clone();
        m.platforms.get_mut("linux-x86_64").unwrap().files[1].path = "game.bin/inner".into();
        assert!(m.validate().is_err(), "file used as directory");

        let mut m = good.clone();
        m.platforms.get_mut("linux-x86_64").unwrap().files[0].size += 1;
        assert!(m.validate().is_err(), "file size mismatch");

        let mut m = good.clone();
        m.platforms.get_mut("linux-x86_64").unwrap().total_size += 1;
        assert!(m.validate().is_err(), "total size mismatch");

        let mut m = good.clone();
        m.platforms.get_mut("linux-x86_64").unwrap().launch.exe = "missing".into();
        assert!(m.validate().is_err(), "launch exe missing");

        let mut m = good.clone();
        m.platforms.get_mut("linux-x86_64").unwrap().launch.cwd = "/tmp".into();
        assert!(m.validate().is_err(), "absolute cwd");

        let mut m = good.clone();
        let p = m.platforms.remove("linux-x86_64").unwrap();
        m.platforms.insert("Linux".into(), p);
        assert!(m.validate().is_err(), "bad platform name");

        let mut m = good.clone();
        m.platforms.clear();
        assert!(m.validate().is_err(), "no platforms");
    }

    #[test]
    fn oversized_chunk_rejected() {
        let key = SecretKey::generate();
        let mut m = sample(&key);
        let p = m.platforms.get_mut("linux-x86_64").unwrap();
        p.files[0].chunks[0].size = MAX_CHUNK_SIZE + 1;
        p.files[0].size = MAX_CHUNK_SIZE + 1;
        p.total_size = p.files.iter().map(|f| f.size).sum();
        assert!(m.validate().is_err());
    }

    #[test]
    fn fixture_release_verifies() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../protocol/fixtures/release-manifest.json");
        let release = SignedRelease::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(release.manifest().slug, "cool-game");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../protocol/fixtures/game-manifest.json");
        let game = GameManifest::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(game.game_id, release.manifest().game_id);
        assert_eq!(
            game.release_url("1.2.0"),
            "https://host.example/games/cool-game/releases/1.2.0.json"
        );
    }

    #[test]
    fn unique_chunks_dedupes() {
        let p = sample_platform(&[("a", b"same"), ("b", b"same"), ("c", b"other")]);
        assert_eq!(p.unique_chunks().len(), 2);
    }

    #[test]
    fn rotation_statements() {
        let k1 = SecretKey::generate();
        let k2 = SecretKey::generate();
        let k3 = SecretKey::generate();
        let s12 = KeyRotation::create(&k1, k2.public(), "cool-game").unwrap();
        let s23 = KeyRotation::create(&k2, k3.public(), "cool-game").unwrap();
        KeyRotation::verify(&s12).unwrap();

        let all = vec![s23.clone(), s12.clone()];
        assert!(rotation_chain_links(
            "cool-game",
            &k1.public(),
            &k3.public(),
            &all
        ));
        assert!(rotation_chain_links(
            "cool-game",
            &k1.public(),
            &k2.public(),
            &all
        ));
        assert!(!rotation_chain_links(
            "cool-game",
            &k3.public(),
            &k1.public(),
            &all
        ));
        assert!(!rotation_chain_links(
            "other-game",
            &k1.public(),
            &k2.public(),
            &all
        ));
        assert!(!rotation_chain_links(
            "cool-game",
            &k1.public(),
            &k3.public(),
            std::slice::from_ref(&s12)
        ));

        // A statement signed by the *new* key (attacker) is worthless.
        let attacker = SecretKey::generate();
        let mut forged = s12.clone();
        forged["new_key"] = serde_json::json!(attacker.public().to_string());
        assert!(KeyRotation::verify(&forged).is_err());
        assert!(!rotation_chain_links(
            "cool-game",
            &k1.public(),
            &attacker.public(),
            &[forged]
        ));
    }
}
