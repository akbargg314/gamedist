//! Content-addressed chunk store: `<root>/<id[0..2]>/<id>`.
//!
//! Writes are verified, go to a temp file first and are renamed into place, so
//! a chunk that exists under its ID is always complete.

use crate::{error::IoContext, ChunkId, Error, Result};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct ContentStore {
    root: PathBuf,
}

impl ContentStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root).at(&root)?;
        Ok(ContentStore { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path(&self, id: &ChunkId) -> PathBuf {
        let hex = id.to_hex();
        self.root.join(&hex[..2]).join(hex)
    }

    pub fn has(&self, id: &ChunkId) -> bool {
        self.path(id).is_file()
    }

    pub fn size(&self, id: &ChunkId) -> Option<u64> {
        std::fs::metadata(self.path(id)).ok().map(|m| m.len())
    }

    /// Store `data` under `id` after verifying its hash.
    /// Returns `true` if the chunk was newly written.
    pub fn put(&self, id: &ChunkId, data: &[u8]) -> Result<bool> {
        let actual = ChunkId::of(data);
        if actual != *id {
            return Err(Error::ChunkMismatch {
                id: id.to_hex(),
                actual: actual.to_hex(),
            });
        }
        self.put_trusted(id, data)
    }

    /// Store data whose hash the caller has already verified.
    pub fn put_trusted(&self, id: &ChunkId, data: &[u8]) -> Result<bool> {
        let dest = self.path(id);
        if dest.is_file() {
            return Ok(false);
        }
        let dir = dest.parent().expect("chunk path has a parent");
        std::fs::create_dir_all(dir).at(dir)?;
        let tmp = dir.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let write = (|| {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(data)?;
            f.sync_data()?;
            std::fs::rename(&tmp, &dest)
        })();
        if let Err(e) = write {
            let _ = std::fs::remove_file(&tmp);
            return Err(Error::io(dest.display().to_string(), e));
        }
        Ok(true)
    }

    /// Read a chunk and verify it. A corrupt chunk is removed and reported missing.
    pub fn get(&self, id: &ChunkId) -> Result<Vec<u8>> {
        let path = self.path(id);
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::MissingChunk(id.to_hex()))
            }
            Err(e) => return Err(Error::io(path.display().to_string(), e)),
        };
        if ChunkId::of(&data) != *id {
            let _ = std::fs::remove_file(&path);
            return Err(Error::MissingChunk(id.to_hex()));
        }
        Ok(data)
    }

    /// Read without re-hashing (for servers streaming immutable chunks).
    pub fn get_unverified(&self, id: &ChunkId) -> Result<Vec<u8>> {
        let path = self.path(id);
        std::fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::MissingChunk(id.to_hex())
            } else {
                Error::io(path.display().to_string(), e)
            }
        })
    }

    pub fn remove(&self, id: &ChunkId) -> Result<bool> {
        match std::fs::remove_file(self.path(id)) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(Error::io(self.path(id).display().to_string(), e)),
        }
    }

    /// Every chunk currently in the store.
    pub fn list(&self) -> Result<Vec<ChunkId>> {
        let mut out = Vec::new();
        for shard in std::fs::read_dir(&self.root).at(&self.root)? {
            let shard = shard.at(&self.root)?.path();
            if !shard.is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(&shard).at(&shard)? {
                let entry = entry.at(&shard)?;
                if let Some(id) = entry.file_name().to_str().and_then(|n| n.parse().ok()) {
                    out.push(id);
                }
            }
        }
        Ok(out)
    }

    /// Total bytes stored.
    pub fn disk_usage(&self) -> Result<u64> {
        Ok(self.list()?.iter().filter_map(|id| self.size(id)).sum())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_roundtrip_and_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ContentStore::open(tmp.path().join("store")).unwrap();
        let id = ChunkId::of(b"data");
        assert!(!store.has(&id));
        assert!(store.put(&id, b"data").unwrap());
        assert!(!store.put(&id, b"data").unwrap(), "second put is a no-op");
        assert!(store.has(&id));
        assert_eq!(store.get(&id).unwrap(), b"data");
        let hex = id.to_hex();
        assert!(tmp
            .path()
            .join("store")
            .join(&hex[..2])
            .join(&hex)
            .is_file());
        assert_eq!(store.list().unwrap(), vec![id]);
        assert_eq!(store.disk_usage().unwrap(), 4);
        assert!(store.remove(&id).unwrap());
        assert!(!store.remove(&id).unwrap());
    }

    #[test]
    fn rejects_wrong_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ContentStore::open(tmp.path()).unwrap();
        let id = ChunkId::of(b"right");
        assert!(matches!(
            store.put(&id, b"wrong"),
            Err(Error::ChunkMismatch { .. })
        ));
        assert!(!store.has(&id));
        assert!(
            store.list().unwrap().is_empty(),
            "no temp files left behind"
        );
    }

    #[test]
    fn corrupt_chunk_is_evicted_on_read() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ContentStore::open(tmp.path()).unwrap();
        let id = ChunkId::of(b"good");
        store.put(&id, b"good").unwrap();
        std::fs::write(store.path(&id), b"evil").unwrap();
        assert!(matches!(store.get(&id), Err(Error::MissingChunk(_))));
        assert!(!store.has(&id));
        assert!(matches!(store.get(&id), Err(Error::MissingChunk(_))));
    }
}
