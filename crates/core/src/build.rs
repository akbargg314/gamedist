//! Scanning a build directory into manifest file entries.

use crate::{
    chunker::{chunk_reader, ChunkParams},
    error::IoContext,
    manifest::{FileEntry, Launch, Platform},
    paths, ChunkId, Error, Result,
};
use rayon::prelude::*;
use std::{
    collections::HashMap,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

/// Where a chunk's bytes can be re-read from in the build directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkSource {
    pub path: PathBuf,
    pub offset: u64,
    pub size: u64,
}

impl ChunkSource {
    /// Re-read and re-verify the chunk (the build dir may have changed).
    pub fn read(&self, id: &ChunkId) -> Result<Vec<u8>> {
        let mut file = std::fs::File::open(&self.path).at(&self.path)?;
        file.seek(SeekFrom::Start(self.offset)).at(&self.path)?;
        let mut buf = vec![0u8; self.size as usize];
        file.read_exact(&mut buf).at(&self.path)?;
        let actual = ChunkId::of(&buf);
        if actual != *id {
            return Err(Error::other(format!(
                "{} changed while it was being pushed",
                self.path.display()
            )));
        }
        Ok(buf)
    }
}

/// The result of scanning one build directory.
#[derive(Debug, Clone)]
pub struct BuildScan {
    pub files: Vec<FileEntry>,
    pub sources: HashMap<ChunkId, ChunkSource>,
    pub total_size: u64,
}

impl BuildScan {
    pub fn into_platform(self, launch: Launch) -> Result<Platform> {
        let platform = Platform {
            launch,
            files: self.files,
            total_size: self.total_size,
        };
        platform.validate()?;
        Ok(platform)
    }
}

/// Walk `dir`, chunk and hash every file (in parallel) and return entries
/// sorted by path. Symlinks are refused: they do not survive distribution.
pub fn scan_build(dir: &Path, params: ChunkParams) -> Result<BuildScan> {
    if !dir.is_dir() {
        return Err(Error::other(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    let mut inputs: Vec<(String, PathBuf)> = Vec::new();
    for entry in walkdir::WalkDir::new(dir).follow_links(false) {
        let entry = entry.map_err(|e| Error::other(format!("walking {}: {e}", dir.display())))?;
        let ty = entry.file_type();
        if ty.is_dir() {
            continue;
        }
        if ty.is_symlink() {
            return Err(Error::UnsafePath {
                path: entry.path().display().to_string(),
                reason: "symlinks are not supported in builds; replace it with a real file",
            });
        }
        if !ty.is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(dir)
            .expect("walkdir yields paths under its root");
        inputs.push((paths::to_manifest_path(rel)?, entry.path().to_path_buf()));
    }
    inputs.sort();

    type Scanned = (FileEntry, Vec<(ChunkId, ChunkSource)>);
    let scanned: Vec<Scanned> = inputs
        .par_iter()
        .map(|(rel, abs)| -> Result<Scanned> {
            let file = std::fs::File::open(abs).at(abs)?;
            let meta = file.metadata().at(abs)?;
            let mut sources = Vec::new();
            let mut head = [0u8; 4];
            let mut head_len = 0usize;
            let chunks = chunk_reader(&file, params, |offset, id, data| {
                if offset == 0 {
                    head_len = data.len().min(4);
                    head[..head_len].copy_from_slice(&data[..head_len]);
                }
                sources.push((
                    id,
                    ChunkSource {
                        path: abs.clone(),
                        offset,
                        size: data.len() as u64,
                    },
                ));
                Ok(())
            })?;
            let size: u64 = chunks.iter().map(|c| c.size).sum();
            Ok((
                FileEntry {
                    path: rel.clone(),
                    size,
                    mode: file_mode(&meta, &head[..head_len]),
                    chunks,
                },
                sources,
            ))
        })
        .collect::<Result<_>>()?;

    let mut files = Vec::with_capacity(scanned.len());
    let mut sources = HashMap::new();
    let mut total_size = 0;
    for (entry, chunk_sources) in scanned {
        total_size += entry.size;
        files.push(entry);
        for (id, source) in chunk_sources {
            sources.entry(id).or_insert(source);
        }
    }
    Ok(BuildScan {
        files,
        sources,
        total_size,
    })
}

#[cfg(unix)]
fn file_mode(meta: &std::fs::Metadata, _head: &[u8]) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    if meta.permissions().mode() & 0o111 != 0 {
        0o755
    } else {
        0o644
    }
}

/// Windows has no executable bit; infer it for binaries and scripts so a
/// Linux/macOS build pushed from Windows still launches.
#[cfg(not(unix))]
fn file_mode(_meta: &std::fs::Metadata, head: &[u8]) -> u32 {
    const MAGICS: &[&[u8]] = &[
        b"\x7fELF",
        b"#!",
        b"MZ",
        &[0xCF, 0xFA, 0xED, 0xFE],
        &[0xCA, 0xFE, 0xBA, 0xBE],
    ];
    if MAGICS.iter().any(|m| head.starts_with(m)) {
        0o755
    } else {
        0o644
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_sorted_and_deterministic() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("data/sub")).unwrap();
        std::fs::write(tmp.path().join("game"), b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::write(tmp.path().join("data/b.bin"), vec![7u8; 50_000]).unwrap();
        std::fs::write(tmp.path().join("data/sub/a.txt"), b"hello").unwrap();
        std::fs::write(tmp.path().join("data/empty"), b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                tmp.path().join("game"),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        }

        let scan = scan_build(tmp.path(), ChunkParams::small()).unwrap();
        let paths: Vec<_> = scan.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            ["data/b.bin", "data/empty", "data/sub/a.txt", "game"]
        );
        assert_eq!(scan.total_size, 50_000 + 5 + 17);
        assert_eq!(scan.files[1].chunks.len(), 0);
        assert_eq!(scan.files[3].mode, 0o755);
        assert_eq!(scan.files[2].mode, 0o644);

        let again = scan_build(tmp.path(), ChunkParams::small()).unwrap();
        assert_eq!(scan.files, again.files);

        // Every chunk can be re-read from its recorded source.
        for file in &scan.files {
            for chunk in &file.chunks {
                let data = scan.sources[&chunk.id].read(&chunk.id).unwrap();
                assert_eq!(data.len() as u64, chunk.size);
            }
        }

        let platform = scan
            .into_platform(Launch {
                exe: "game".into(),
                args: vec![],
                cwd: ".".into(),
            })
            .unwrap();
        assert_eq!(platform.files.len(), 4);
    }

    #[test]
    fn detects_files_changing_under_us() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f"), b"original").unwrap();
        let scan = scan_build(tmp.path(), ChunkParams::small()).unwrap();
        std::fs::write(tmp.path().join("f"), b"modified").unwrap();
        let chunk = scan.files[0].chunks[0];
        assert!(scan.sources[&chunk.id].read(&chunk.id).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("real"), b"x").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", tmp.path().join("link")).unwrap();
        assert!(matches!(
            scan_build(tmp.path(), ChunkParams::small()),
            Err(Error::UnsafePath { .. })
        ));
    }

    #[test]
    fn missing_launch_exe_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f"), b"x").unwrap();
        let scan = scan_build(tmp.path(), ChunkParams::small()).unwrap();
        assert!(scan
            .into_platform(Launch {
                exe: "nope".into(),
                args: vec![],
                cwd: ".".into()
            })
            .is_err());
    }
}
