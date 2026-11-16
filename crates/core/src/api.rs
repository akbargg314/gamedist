//! Request/response bodies of the REST API (SPEC §5), shared by server and clients.

use crate::{manifest::Assets, ChunkId, PublicKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// RFC 7807 problem document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    #[serde(rename = "type", default = "about_blank")]
    pub kind: String,
    pub title: String,
    pub status: u16,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

fn about_blank() -> String {
    "about:blank".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRequest {
    pub email: String,
    pub password: String,
    /// Label shown in the dashboard, e.g. `cli@laptop`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddKeyRequest {
    pub pubkey: PublicKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateGameRequest {
    pub slug: String,
    pub developer_pubkey: PublicKey,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub description_md: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PatchGameRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description_md: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assets: Option<Assets>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingChunksRequest {
    pub ids: Vec<ChunkId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingChunksResponse {
    pub missing: Vec<ChunkId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetChannelRequest {
    pub channel: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentRef {
    pub info_hash: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseSummary {
    pub version: String,
    pub created: String,
    pub platforms: Vec<String>,
    #[serde(default)]
    pub torrents: BTreeMap<String, TorrentRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseList {
    pub channels: BTreeMap<String, String>,
    pub releases: Vec<ReleaseSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameSummary {
    pub game_id: String,
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    pub developer_pubkey: PublicKey,
    /// Canonical game page URL on the serving host.
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<ChunkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banner: Option<ChunkId>,
    #[serde(default)]
    pub channels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogPage {
    pub games: Vec<GameSummary>,
    pub page: u32,
    pub pages: u32,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevKey {
    pub pubkey: PublicKey,
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Me {
    pub email: String,
    pub is_admin: bool,
    pub keys: Vec<DevKey>,
    pub games: Vec<String>,
    pub storage_used: u64,
    /// `None` means unlimited.
    pub storage_quota: Option<u64>,
}
