//! Identifiers: slugs, game IDs, chunk IDs, versions, platforms, game URLs.

use crate::{keys::PublicKey, Error, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::{fmt, str::FromStr};

/// `[a-z0-9][a-z0-9-]{1,63}`
pub fn validate_slug(slug: &str) -> Result<()> {
    let b = slug.as_bytes();
    let ok = (2..=64).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-');
    if ok {
        Ok(())
    } else {
        Err(Error::invalid(
            "slug",
            format!("{slug:?} must match [a-z0-9][a-z0-9-]{{1,63}}"),
        ))
    }
}

/// `[a-z0-9][a-z0-9-]{0,31}`
pub fn validate_channel(channel: &str) -> Result<()> {
    let b = channel.as_bytes();
    let ok = (1..=32).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-');
    if ok {
        Ok(())
    } else {
        Err(Error::invalid("channel", format!("{channel:?}")))
    }
}

/// `<os>-<arch>`: `[a-z0-9]+-[a-z0-9_]+`
pub fn validate_platform(platform: &str) -> Result<()> {
    let ok = match platform.split_once('-') {
        Some((os, arch)) => {
            !os.is_empty()
                && !arch.is_empty()
                && os
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                && arch
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        }
        None => false,
    };
    if ok {
        Ok(())
    } else {
        Err(Error::invalid(
            "platform",
            format!("{platform:?} must look like linux-x86_64"),
        ))
    }
}

/// SemVer 2.0.0 without build metadata.
pub fn parse_version(version: &str) -> Result<semver::Version> {
    let v = semver::Version::parse(version)
        .map_err(|e| Error::invalid("version", format!("{version:?}: {e}")))?;
    if !v.build.is_empty() {
        return Err(Error::invalid(
            "version",
            format!("{version:?}: build metadata is not allowed"),
        ));
    }
    Ok(v)
}

/// The platform string of the machine this code runs on.
pub fn host_platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Stable game identifier: `base32(sha256(pubkey || slug))[0..26]`.
pub fn game_id(pubkey: &PublicKey, slug: &str) -> String {
    let mut h = Sha256::new();
    h.update(pubkey.as_bytes());
    h.update(slug.as_bytes());
    let digest = h.finalize();
    let mut s = data_encoding::BASE32_NOPAD
        .encode(&digest)
        .to_ascii_lowercase();
    s.truncate(26);
    s
}

/// BLAKE3-256 of a chunk's raw bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChunkId(pub [u8; 32]);

impl ChunkId {
    pub fn of(data: &[u8]) -> Self {
        ChunkId(*blake3::hash(data).as_bytes())
    }

    pub fn to_hex(&self) -> String {
        data_encoding::HEXLOWER.encode(&self.0)
    }
}

impl fmt::Display for ChunkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for ChunkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChunkId({})", self.to_hex())
    }
}

impl FromStr for ChunkId {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let bad = || Error::invalid("chunk id", format!("{s:?} is not 64 lowercase hex chars"));
        if s.len() != 64
            || !s
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(bad());
        }
        let bytes = data_encoding::HEXLOWER
            .decode(s.as_bytes())
            .map_err(|_| bad())?;
        Ok(ChunkId(bytes.try_into().map_err(|_| bad())?))
    }
}

impl Serialize for ChunkId {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ChunkId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// A parsed game URL (`https://host/games/slug` or `gamedist://host/games/slug`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameUrl {
    /// Scheme + authority (+ optional path prefix), no trailing slash.
    pub base: String,
    pub slug: String,
}

impl GameUrl {
    pub fn parse(input: &str) -> Result<Self> {
        let input = input.trim();
        let bad = |why: &str| Error::invalid("game url", format!("{input:?}: {why}"));
        let (scheme, rest) = input
            .split_once("://")
            .ok_or_else(|| bad("expected https://host/games/<slug>"))?;
        let scheme = match scheme.to_ascii_lowercase().as_str() {
            "https" | "gamedist" => "https",
            "http" | "gamedist+http" => "http",
            _ => return Err(bad("unsupported scheme")),
        };
        let rest = rest.split(['?', '#']).next().unwrap_or("");
        let rest = rest.trim_end_matches('/');
        let rest = rest.strip_suffix("/manifest.json").unwrap_or(rest);
        let (prefix, slug) = rest
            .rsplit_once("/games/")
            .ok_or_else(|| bad("missing /games/<slug>"))?;
        if prefix.is_empty() || prefix.contains(['@', ' ', '\\']) {
            return Err(bad("missing or invalid host"));
        }
        validate_slug(slug)?;
        Ok(GameUrl {
            base: format!("{scheme}://{prefix}"),
            slug: slug.to_string(),
        })
    }

    pub fn page_url(&self) -> String {
        format!("{}/games/{}", self.base, self.slug)
    }

    pub fn manifest_url(&self) -> String {
        format!("{}/games/{}/manifest.json", self.base, self.slug)
    }
}

impl fmt::Display for GameUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.page_url())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        for ok in ["ab", "cool-game", "0ad", &"a".repeat(64)] {
            validate_slug(ok).unwrap();
        }
        for bad in ["", "a", "-ab", "Cool", "a_b", "a b", &"a".repeat(65), "é1"] {
            assert!(validate_slug(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn platforms_and_versions() {
        validate_platform("linux-x86_64").unwrap();
        validate_platform("macos-aarch64").unwrap();
        for bad in ["linux", "Linux-x86_64", "linux-", "-x86", "a/b-c"] {
            assert!(validate_platform(bad).is_err(), "{bad}");
        }
        parse_version("1.2.0").unwrap();
        parse_version("1.3.0-beta.1").unwrap();
        assert!(parse_version("1.2").is_err());
        assert!(parse_version("1.2.0+build5").is_err());
        assert!(parse_version("1.3.0-beta.1").unwrap() < parse_version("1.3.0").unwrap());
        validate_platform(&host_platform()).unwrap();
    }

    #[test]
    fn game_id_is_stable() {
        let key: PublicKey = "ed25519:11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo"
            .parse()
            .unwrap();
        let id = game_id(&key, "cool-game");
        assert_eq!(id.len(), 26);
        assert_eq!(id, game_id(&key, "cool-game"));
        assert_ne!(id, game_id(&key, "cool-game2"));
        assert!(id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || (b'2'..=b'7').contains(&c)));
        // Fixed vector so other implementations can check themselves.
        assert_eq!(id, "b3m5htl3wjehc6dcbmofo6h6cq");
    }

    #[test]
    fn chunk_id_roundtrip() {
        let id = ChunkId::of(b"hello");
        assert_eq!(
            id.to_hex(),
            "ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f"
        );
        assert_eq!(id.to_hex().parse::<ChunkId>().unwrap(), id);
        assert!("EA8F".parse::<ChunkId>().is_err());
        assert!(id.to_hex().to_uppercase().parse::<ChunkId>().is_err());
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<ChunkId>(&json).unwrap(), id);
    }

    #[test]
    fn game_urls() {
        let u = GameUrl::parse("https://host.example/games/cool-game").unwrap();
        assert_eq!(u.base, "https://host.example");
        assert_eq!(u.slug, "cool-game");
        assert_eq!(
            GameUrl::parse("gamedist://host.example/games/cool-game/").unwrap(),
            u
        );
        assert_eq!(
            GameUrl::parse("https://host.example/games/cool-game/manifest.json").unwrap(),
            u
        );
        let dev = GameUrl::parse("gamedist+http://127.0.0.1:8080/games/cool-game").unwrap();
        assert_eq!(dev.base, "http://127.0.0.1:8080");
        assert_eq!(
            dev.manifest_url(),
            "http://127.0.0.1:8080/games/cool-game/manifest.json"
        );
        for bad in [
            "cool-game",
            "ftp://h/games/x1",
            "https://host/cool-game",
            "https:///games/cool-game",
            "https://host/games/Bad_Slug",
        ] {
            assert!(GameUrl::parse(bad).is_err(), "{bad}");
        }
    }
}
