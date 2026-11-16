//! Assembling an install tree from the content store: planning, harvesting
//! chunks from the installed version, atomic apply with rollback, and
//! verification.

use crate::{
    error::IoContext,
    manifest::{ChunkRef, Platform},
    paths,
    store::ContentStore,
    ChunkId, Error, Result,
};
use rayon::prelude::*;
use std::{
    collections::{HashMap, HashSet},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

pub const STAGING_DIR: &str = ".gamedist-staging";
pub const BACKUP_DIR: &str = ".gamedist-backup";

/// What has to change on disk to get from `old` to `new`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallPlan {
    /// Indices into `new.files` that must be (re)written.
    pub write: Vec<usize>,
    /// Paths from the old version that no longer exist.
    pub delete: Vec<String>,
    /// Distinct chunks needed to write those files.
    pub chunks: Vec<ChunkRef>,
}

impl InstallPlan {
    pub fn is_noop(&self) -> bool {
        self.write.is_empty() && self.delete.is_empty()
    }

    pub fn write_bytes(&self, new: &Platform) -> u64 {
        self.write.iter().map(|&i| new.files[i].size).sum()
    }

    fn from_indices(new: &Platform, write: Vec<usize>, delete: Vec<String>) -> Self {
        let mut seen = HashSet::new();
        let chunks = write
            .iter()
            .flat_map(|&i| new.files[i].chunks.iter())
            .filter(|c| seen.insert(c.id))
            .copied()
            .collect();
        InstallPlan {
            write,
            delete,
            chunks,
        }
    }
}

/// Plan a fresh install (`old = None`) or an update. A file is rewritten when
/// its content differs from the old manifest or it is missing / the wrong
/// size on disk.
pub fn plan_install(new: &Platform, old: Option<&Platform>, dir: &Path) -> InstallPlan {
    let old_files: HashMap<&str, _> = old
        .map(|p| p.files.iter().map(|f| (f.path.as_str(), f)).collect())
        .unwrap_or_default();
    let write = new
        .files
        .iter()
        .enumerate()
        .filter(|(_, file)| {
            let unchanged = old_files
                .get(file.path.as_str())
                .is_some_and(|o| o.same_content(file));
            let on_disk = paths::join(dir, &file.path)
                .ok()
                .and_then(|p| std::fs::symlink_metadata(p).ok())
                .is_some_and(|m| m.is_file() && m.len() == file.size);
            !(unchanged && on_disk)
        })
        .map(|(i, _)| i)
        .collect();
    let new_paths: HashSet<&str> = new.files.iter().map(|f| f.path.as_str()).collect();
    let delete = old
        .map(|p| {
            p.files
                .iter()
                .filter(|f| !new_paths.contains(f.path.as_str()))
                .map(|f| f.path.clone())
                .collect()
        })
        .unwrap_or_default();
    InstallPlan::from_indices(new, write, delete)
}

/// Plan a repair: rewrite exactly the given files.
pub fn plan_repair(platform: &Platform, bad_paths: &[String]) -> InstallPlan {
    let bad: HashSet<&str> = bad_paths.iter().map(String::as_str).collect();
    let write = platform
        .files
        .iter()
        .enumerate()
        .filter(|(_, f)| bad.contains(f.path.as_str()))
        .map(|(i, _)| i)
        .collect();
    InstallPlan::from_indices(platform, write, Vec::new())
}

/// Chunks of the plan that are not in the store yet.
pub fn missing_chunks(plan: &InstallPlan, store: &ContentStore) -> Vec<ChunkRef> {
    plan.chunks
        .iter()
        .filter(|c| !store.has(&c.id))
        .copied()
        .collect()
}

/// Copy wanted chunks out of an installed tree into the store. This is what
/// makes delta updates work when the store was pruned after install. Every
/// chunk is verified; damaged regions are simply skipped.
/// Returns the number of bytes recovered.
pub fn harvest(
    installed: &Platform,
    dir: &Path,
    wanted: &[ChunkRef],
    store: &ContentStore,
) -> Result<u64> {
    harvest_with(installed, dir, wanted, store, |_, _| {})
}

/// [`harvest`] with progress: `on_progress(done, total)` is called once up
/// front and after every chunk that was read, whether or not it was usable.
/// `total` is the size of the wanted chunks that `installed` contains.
pub fn harvest_with(
    installed: &Platform,
    dir: &Path,
    wanted: &[ChunkRef],
    store: &ContentStore,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<u64> {
    let mut wanted: HashSet<ChunkId> = wanted
        .iter()
        .filter(|c| !store.has(&c.id))
        .map(|c| c.id)
        .collect();
    let total: u64 = {
        let mut counted = HashSet::new();
        installed
            .files
            .iter()
            .flat_map(|f| &f.chunks)
            .filter(|c| wanted.contains(&c.id) && counted.insert(c.id))
            .map(|c| c.size)
            .sum()
    };
    let mut attempted: HashSet<ChunkId> = HashSet::new();
    let mut done = 0u64;
    on_progress(0, total);
    let mut recovered = 0;
    for file in &installed.files {
        if wanted.is_empty() {
            break;
        }
        if !file.chunks.iter().any(|c| wanted.contains(&c.id)) {
            continue;
        }
        let Ok(path) = paths::join_no_symlinks(dir, &file.path) else {
            continue;
        };
        let Ok(mut handle) = std::fs::File::open(&path) else {
            continue;
        };
        let mut offset = 0u64;
        for chunk in &file.chunks {
            if wanted.contains(&chunk.id) {
                let mut buf = vec![0u8; chunk.size as usize];
                let ok = handle.seek(SeekFrom::Start(offset)).is_ok()
                    && handle.read_exact(&mut buf).is_ok()
                    && ChunkId::of(&buf) == chunk.id;
                if ok {
                    store.put_trusted(&chunk.id, &buf)?;
                    wanted.remove(&chunk.id);
                    recovered += chunk.size;
                }
                // A damaged copy is tried again if another file has the same
                // chunk; count each chunk once so `done` never passes `total`.
                if attempted.insert(chunk.id) {
                    done += chunk.size;
                    on_progress(done, total);
                }
            }
            offset += chunk.size;
        }
    }
    Ok(recovered)
}

/// If a previous apply was interrupted mid-swap, put the old files back.
pub fn recover_interrupted(dir: &Path) -> Result<bool> {
    let backup = dir.join(BACKUP_DIR);
    let staging = dir.join(STAGING_DIR);
    let mut recovered = false;
    if backup.is_dir() {
        let journal = backup.join("journal.json");
        if let Ok(text) = std::fs::read_to_string(&journal) {
            let entries: Vec<JournalEntry> = serde_json::from_str(&text).unwrap_or_default();
            rollback(dir, &backup, &entries);
            recovered = true;
        }
        let _ = std::fs::remove_dir_all(&backup);
    }
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    Ok(recovered)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct JournalEntry {
    /// Manifest path of the file that was touched.
    path: String,
    /// Name in the backup dir holding the previous file, if there was one.
    backup: Option<String>,
    /// Whether a new file was moved into place.
    placed: bool,
}

fn rollback(dir: &Path, backup: &Path, entries: &[JournalEntry]) {
    for entry in entries.iter().rev() {
        let Ok(dest) = paths::join(dir, &entry.path) else {
            continue;
        };
        let backup_file = entry.backup.as_ref().map(|name| backup.join(name));
        // Only remove what we placed. If the old file never made it to the
        // backup dir (crash between journal write and rename), `dest` is
        // still the old file and must be left alone.
        let old_was_moved = backup_file.as_ref().is_some_and(|p| p.exists());
        if entry.placed && (entry.backup.is_none() || old_was_moved) {
            let _ = std::fs::remove_file(&dest);
        }
        if let Some(src) = backup_file.filter(|_| old_was_moved) {
            if let Some(parent) = dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::rename(src, &dest);
        }
    }
}

/// Write the planned files from the store into `dir`.
///
/// Files are first fully assembled (and their chunks verified) in a staging
/// directory inside `dir`, then swapped into place one rename at a time while
/// the previous files are kept in a backup directory. Any failure restores
/// the previous state and returns [`Error::RolledBack`].
pub fn apply_install(
    new: &Platform,
    dir: &Path,
    store: &ContentStore,
    plan: &InstallPlan,
    mut on_progress: impl FnMut(u64),
) -> Result<()> {
    new.validate()?;
    std::fs::create_dir_all(dir).at(dir)?;
    recover_interrupted(dir)?;

    let staging = dir.join(STAGING_DIR);
    let backup = dir.join(BACKUP_DIR);
    std::fs::create_dir_all(&staging).at(&staging)?;

    // 1. Stage. Nothing outside the staging dir is touched yet.
    let staged = (|| -> Result<()> {
        for &index in &plan.write {
            let file = &new.files[index];
            let tmp = staging.join(index.to_string());
            let mut out = std::fs::File::create(&tmp).at(&tmp)?;
            for chunk in &file.chunks {
                let data = store.get(&chunk.id)?;
                if data.len() as u64 != chunk.size {
                    return Err(Error::ChunkMismatch {
                        id: chunk.id.to_hex(),
                        actual: format!("{} bytes", data.len()),
                    });
                }
                out.write_all(&data).at(&tmp)?;
                on_progress(chunk.size);
            }
            out.sync_data().at(&tmp)?;
            drop(out);
            set_mode(&tmp, file.effective_mode())?;
        }
        Ok(())
    })();
    if let Err(e) = staged {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }

    // 2. Swap, journaling every step so it can be undone (even after a crash).
    std::fs::create_dir_all(&backup).at(&backup)?;
    let mut journal: Vec<JournalEntry> = Vec::new();
    let mut backup_seq = 0usize;
    let mut swap = || -> Result<()> {
        let steps = plan
            .write
            .iter()
            .map(|&i| {
                (
                    new.files[i].path.as_str(),
                    Some(staging.join(i.to_string())),
                )
            })
            .chain(
                plan.delete
                    .iter()
                    .filter(|rel| paths::validate_rel_path(rel).is_ok())
                    .map(|rel| (rel.as_str(), None)),
            );
        for (rel, staged) in steps {
            swap_one(dir, &backup, &mut journal, &mut backup_seq, rel, staged)?;
        }
        Ok(())
    };

    if let Err(e) = swap() {
        rollback(dir, &backup, &journal);
        let _ = std::fs::remove_dir_all(&backup);
        let _ = std::fs::remove_dir_all(&staging);
        return Err(Error::RolledBack(e.to_string()));
    }

    // 3. Commit.
    let _ = std::fs::remove_dir_all(&backup);
    let _ = std::fs::remove_dir_all(&staging);
    for rel in &plan.delete {
        prune_empty_parents(dir, rel);
    }
    // Modes can change without content changing.
    for file in &new.files {
        if let Ok(path) = paths::join(dir, &file.path) {
            if path.is_file() {
                set_mode(&path, file.effective_mode())?;
            }
        }
    }
    Ok(())
}

fn swap_one(
    dir: &Path,
    backup: &Path,
    journal: &mut Vec<JournalEntry>,
    backup_seq: &mut usize,
    rel: &str,
    staged: Option<PathBuf>,
) -> Result<()> {
    let dest = paths::join_no_symlinks(dir, rel)?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).at(parent)?;
    }
    let mut entry = JournalEntry {
        path: rel.to_string(),
        backup: None,
        placed: false,
    };
    match std::fs::symlink_metadata(&dest) {
        Ok(meta) if meta.is_dir() => {
            return Err(Error::other(format!(
                "{} is a directory but the release has a file there",
                dest.display()
            )))
        }
        Ok(_) => {
            let name = backup_seq.to_string();
            *backup_seq += 1;
            // Journal first: rollback tolerates a backup file that never
            // got moved.
            entry.backup = Some(name.clone());
            journal.push(entry);
            write_journal(backup, journal)?;
            std::fs::rename(&dest, backup.join(&name)).at(&dest)?;
        }
        Err(_) => journal.push(entry),
    }
    if let Some(staged) = staged {
        journal.last_mut().expect("just pushed").placed = true;
        write_journal(backup, journal)?;
        std::fs::rename(&staged, &dest).at(&dest)?;
    }
    Ok(())
}

fn write_journal(backup: &Path, journal: &[JournalEntry]) -> Result<()> {
    let path = backup.join("journal.json");
    let tmp = backup.join("journal.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(journal)?).at(&tmp)?;
    std::fs::rename(&tmp, &path).at(&path)
}

fn prune_empty_parents(dir: &Path, rel: &str) {
    let Ok(mut path) = paths::join(dir, rel) else {
        return;
    };
    while path.pop() && path != dir && path.starts_with(dir) {
        if std::fs::remove_dir(&path).is_err() {
            break;
        }
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).at(path)
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

/// Re-hash the installed tree against the manifest. Returns the paths of
/// files that are missing, the wrong size, or contain a bad chunk.
pub fn verify_tree(platform: &Platform, dir: &Path) -> Result<Vec<String>> {
    verify_tree_with(platform, dir, &|_| {})
}

/// [`verify_tree`] with progress: `on_bytes(n)` is called, from worker
/// threads, as files are checked. Over the whole run the calls add up to
/// `platform.total_size`: a file found bad reports its unread remainder at
/// once.
pub fn verify_tree_with(
    platform: &Platform,
    dir: &Path,
    on_bytes: &(dyn Fn(u64) + Sync),
) -> Result<Vec<String>> {
    platform.validate()?;
    let bad: Vec<String> = platform
        .files
        .par_iter()
        .filter(|file| {
            let mut checked = 0u64;
            let intact = file_is_intact(file, dir, &mut |n| {
                checked += n;
                on_bytes(n);
            });
            if !intact {
                on_bytes(file.size.saturating_sub(checked));
            }
            !intact
        })
        .map(|file| file.path.clone())
        .collect();
    Ok(bad)
}

fn file_is_intact(
    file: &crate::manifest::FileEntry,
    dir: &Path,
    on_bytes: &mut dyn FnMut(u64),
) -> bool {
    let Ok(path) = paths::join_no_symlinks(dir, &file.path) else {
        return false;
    };
    let Ok(meta) = std::fs::symlink_metadata(&path) else {
        return false;
    };
    if !meta.is_file() || meta.len() != file.size {
        return false;
    }
    let Ok(mut handle) = std::fs::File::open(&path) else {
        return false;
    };
    let mut buf = Vec::new();
    for chunk in &file.chunks {
        buf.resize(chunk.size as usize, 0);
        if handle.read_exact(&mut buf).is_err() || ChunkId::of(&buf) != chunk.id {
            return false;
        }
        on_bytes(chunk.size);
    }
    true
}

/// Remove every file of `platform` from `dir` (uninstall), leaving unknown
/// files (saves, configs written by the game) alone.
pub fn remove_tree(platform: &Platform, dir: &Path) -> Result<()> {
    for file in &platform.files {
        if let Ok(path) = paths::join_no_symlinks(dir, &file.path) {
            let _ = std::fs::remove_file(&path);
            prune_empty_parents(dir, &file.path);
        }
    }
    let _ = std::fs::remove_dir_all(dir.join(STAGING_DIR));
    let _ = std::fs::remove_dir_all(dir.join(BACKUP_DIR));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{FileEntry, Launch};

    /// Build a platform from in-memory files, chunking each into 4-byte pieces.
    fn platform(store: &ContentStore, files: &[(&str, &[u8])]) -> Platform {
        let entries: Vec<FileEntry> = files
            .iter()
            .map(|(path, data)| FileEntry {
                path: path.to_string(),
                size: data.len() as u64,
                mode: if path.ends_with(".sh") { 0o755 } else { 0o644 },
                chunks: data
                    .chunks(4)
                    .map(|c| {
                        let id = ChunkId::of(c);
                        store.put(&id, c).unwrap();
                        ChunkRef {
                            id,
                            size: c.len() as u64,
                        }
                    })
                    .collect(),
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

    fn setup() -> (tempfile::TempDir, ContentStore, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let store = ContentStore::open(tmp.path().join("store")).unwrap();
        let dir = tmp.path().join("install");
        (tmp, store, dir)
    }

    fn read(dir: &Path, rel: &str) -> Vec<u8> {
        std::fs::read(dir.join(rel)).unwrap()
    }

    fn install(p: &Platform, old: Option<&Platform>, dir: &Path, store: &ContentStore) {
        apply_install(p, dir, store, &plan_install(p, old, dir), |_| {}).unwrap();
    }

    #[test]
    fn fresh_install_then_verify() {
        let (_tmp, store, dir) = setup();
        let p = platform(
            &store,
            &[
                ("run.sh", b"#!/bin/sh\n"),
                ("data/a.bin", b"0123456789"),
                ("empty", b""),
            ],
        );
        let plan = plan_install(&p, None, &dir);
        assert_eq!(plan.write, vec![0, 1, 2]);
        assert!(missing_chunks(&plan, &store).is_empty());
        let mut progressed = 0;
        apply_install(&p, &dir, &store, &plan, |n| progressed += n).unwrap();
        assert_eq!(progressed, p.total_size);
        assert_eq!(read(&dir, "data/a.bin"), b"0123456789");
        assert_eq!(read(&dir, "empty"), b"");
        assert!(verify_tree(&p, &dir).unwrap().is_empty());
        assert!(!dir.join(STAGING_DIR).exists() && !dir.join(BACKUP_DIR).exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |rel| {
                std::fs::metadata(dir.join(rel))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777
            };
            assert_eq!(mode("run.sh"), 0o755);
            assert_eq!(mode("data/a.bin"), 0o644);
        }
        // Re-planning against the same version is a no-op.
        assert!(plan_install(&p, Some(&p), &dir).is_noop());
    }

    #[test]
    fn update_rewrites_only_changed_files_and_deletes_removed() {
        let (_tmp, store, dir) = setup();
        let v1 = platform(
            &store,
            &[
                ("run.sh", b"v1"),
                ("keep.bin", b"unchanged!"),
                ("old/gone.txt", b"bye"),
            ],
        );
        install(&v1, None, &dir, &store);
        std::fs::write(dir.join("save.dat"), b"player save").unwrap();

        let v2 = platform(
            &store,
            &[
                ("run.sh", b"v2!!"),
                ("keep.bin", b"unchanged!"),
                ("new.txt", b"hi"),
            ],
        );
        let plan = plan_install(&v2, Some(&v1), &dir);
        assert_eq!(plan.write, vec![0, 2]);
        assert_eq!(plan.delete, vec!["old/gone.txt".to_string()]);
        apply_install(&v2, &dir, &store, &plan, |_| {}).unwrap();

        assert_eq!(read(&dir, "run.sh"), b"v2!!");
        assert_eq!(read(&dir, "new.txt"), b"hi");
        assert!(!dir.join("old").exists(), "emptied directory is pruned");
        assert_eq!(
            read(&dir, "save.dat"),
            b"player save",
            "unknown files survive"
        );
        assert!(verify_tree(&v2, &dir).unwrap().is_empty());
    }

    #[test]
    fn missing_chunk_fails_before_touching_the_install() {
        let (_tmp, store, dir) = setup();
        let v1 = platform(&store, &[("a", b"aaaa"), ("b", b"bbbb")]);
        install(&v1, None, &dir, &store);
        let v2 = platform(&store, &[("a", b"AAAA"), ("b", b"BBBB")]);
        store.remove(&v2.files[1].chunks[0].id).unwrap();
        let plan = plan_install(&v2, Some(&v1), &dir);
        assert_eq!(missing_chunks(&plan, &store).len(), 1);
        let err = apply_install(&v2, &dir, &store, &plan, |_| {}).unwrap_err();
        assert!(matches!(err, Error::MissingChunk(_)), "{err}");
        assert_eq!(read(&dir, "a"), b"aaaa");
        assert_eq!(read(&dir, "b"), b"bbbb");
        assert!(!dir.join(STAGING_DIR).exists());
    }

    #[test]
    fn failure_during_swap_rolls_back() {
        let (_tmp, store, dir) = setup();
        let v1 = platform(&store, &[("a", b"aaaa"), ("gone", b"gone"), ("m", b"mmmm")]);
        install(&v1, None, &dir, &store);
        // v2 wants a file where the user has created a directory: the swaps of
        // "a" and "m" succeed, then "z" fails, and everything must be restored.
        std::fs::create_dir_all(dir.join("z/sub")).unwrap();
        let v2 = platform(&store, &[("a", b"AAAA"), ("m", b"MMMM"), ("z", b"zzzz")]);
        let plan = plan_install(&v2, Some(&v1), &dir);
        let err = apply_install(&v2, &dir, &store, &plan, |_| {}).unwrap_err();
        assert!(matches!(err, Error::RolledBack(_)), "{err}");
        assert_eq!(read(&dir, "a"), b"aaaa");
        assert_eq!(read(&dir, "m"), b"mmmm");
        assert_eq!(read(&dir, "gone"), b"gone");
        assert!(verify_tree(&v1, &dir).unwrap().is_empty());
        assert!(!dir.join(STAGING_DIR).exists() && !dir.join(BACKUP_DIR).exists());
    }

    #[test]
    fn interrupted_swap_is_recovered_from_the_journal() {
        let (_tmp, store, dir) = setup();
        let v1 = platform(&store, &[("a", b"aaaa"), ("b", b"bbbb")]);
        install(&v1, None, &dir, &store);
        // Simulate a crash after "a" was swapped and while "b" was journaled
        // but not yet moved.
        let backup = dir.join(BACKUP_DIR);
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::rename(dir.join("a"), backup.join("0")).unwrap();
        std::fs::write(dir.join("a"), b"HALF").unwrap();
        write_journal(
            &backup,
            &[
                JournalEntry {
                    path: "a".into(),
                    backup: Some("0".into()),
                    placed: true,
                },
                JournalEntry {
                    path: "b".into(),
                    backup: Some("1".into()),
                    placed: true,
                },
            ],
        )
        .unwrap();
        assert!(recover_interrupted(&dir).unwrap());
        assert_eq!(read(&dir, "a"), b"aaaa");
        assert_eq!(read(&dir, "b"), b"bbbb");
        assert!(verify_tree(&v1, &dir).unwrap().is_empty());
        assert!(!recover_interrupted(&dir).unwrap());
    }

    #[test]
    fn verify_finds_damage_and_repair_fixes_it() {
        let (_tmp, store, dir) = setup();
        let p = platform(
            &store,
            &[("a", b"aaaabbbbcccc"), ("b", b"bbbb"), ("c", b"cccc")],
        );
        install(&p, None, &dir, &store);
        std::fs::write(dir.join("a"), b"aaaaXXXXcccc").unwrap(); // same size, bad middle chunk
        std::fs::remove_file(dir.join("b")).unwrap();
        std::fs::write(dir.join("c"), b"too long now").unwrap();
        let mut bad = verify_tree(&p, &dir).unwrap();
        bad.sort();
        assert_eq!(bad, ["a", "b", "c"]);

        let plan = plan_repair(&p, &bad);
        apply_install(&p, &dir, &store, &plan, |_| {}).unwrap();
        assert!(verify_tree(&p, &dir).unwrap().is_empty());
    }

    #[test]
    fn harvest_recovers_chunks_from_installed_files() {
        let (tmp, store, dir) = setup();
        let v1 = platform(&store, &[("a", b"aaaabbbbcccc"), ("b", b"dddd")]);
        install(&v1, None, &dir, &store);

        // Player pruned the store; v2 reuses most of v1's chunks.
        let scratch = ContentStore::open(tmp.path().join("scratch")).unwrap();
        let v2 = platform(&scratch, &[("a", b"aaaaNEW!cccc"), ("b2", b"dddd")]);
        let empty = ContentStore::open(tmp.path().join("pruned")).unwrap();
        let plan = plan_install(&v2, Some(&v1), &dir);
        assert_eq!(missing_chunks(&plan, &empty).len(), 4);

        // Damage one chunk on disk: it must not be harvested.
        std::fs::write(dir.join("b"), b"dddX").unwrap();
        let recovered = harvest(&v1, &dir, &plan.chunks, &empty).unwrap();
        assert_eq!(recovered, 8, "aaaa + cccc");
        let ids: Vec<_> = missing_chunks(&plan, &empty).iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![ChunkId::of(b"NEW!"), ChunkId::of(b"dddd")]);
    }

    #[cfg(unix)]
    #[test]
    fn refuses_to_write_through_symlinks() {
        let (tmp, store, dir) = setup();
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("data")).unwrap();
        let p = platform(&store, &[("data/evil.txt", b"pwn!")]);
        let err =
            apply_install(&p, &dir, &store, &plan_install(&p, None, &dir), |_| {}).unwrap_err();
        assert!(matches!(err, Error::RolledBack(_)), "{err}");
        assert!(!outside.join("evil.txt").exists());
        // And verification does not follow them either.
        std::fs::write(outside.join("evil.txt"), b"pwn!").unwrap();
        assert_eq!(verify_tree(&p, &dir).unwrap(), ["data/evil.txt"]);
    }

    #[test]
    fn uninstall_leaves_foreign_files() {
        let (_tmp, store, dir) = setup();
        let p = platform(&store, &[("a", b"aaaa"), ("d/b", b"bbbb")]);
        install(&p, None, &dir, &store);
        std::fs::write(dir.join("save.dat"), b"keep").unwrap();
        remove_tree(&p, &dir).unwrap();
        assert!(!dir.join("a").exists() && !dir.join("d").exists());
        assert!(dir.join("save.dat").exists());
    }

    #[test]
    fn verify_progress_adds_up_to_total_size() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let (_tmp, store, dir) = setup();
        let p = platform(
            &store,
            &[
                ("a", b"aaaabbbbcc"),
                ("d/b", b"ddddeeee"),
                ("c", b"ffffgggg"),
            ],
        );
        install(&p, None, &dir, &store);
        let sum = AtomicU64::new(0);
        let count = |n| {
            sum.fetch_add(n, Ordering::Relaxed);
        };
        assert!(verify_tree_with(&p, &dir, &count).unwrap().is_empty());
        assert_eq!(sum.load(Ordering::Relaxed), p.total_size);

        // Damaged in its second chunk, missing, and intact: still the total.
        std::fs::write(dir.join("a"), b"aaaaXbbbcc").unwrap();
        std::fs::remove_file(dir.join("d/b")).unwrap();
        sum.store(0, Ordering::Relaxed);
        let mut bad = verify_tree_with(&p, &dir, &count).unwrap();
        bad.sort();
        assert_eq!(bad, ["a", "d/b"]);
        assert_eq!(sum.load(Ordering::Relaxed), p.total_size);
    }

    #[test]
    fn harvest_progress_counts_each_wanted_chunk_once() {
        let (_tmp, store, dir) = setup();
        // "aaaa" occurs in both files; "zzzz" is wanted but not installed.
        let p = platform(&store, &[("a", b"aaaabbbb"), ("b", b"aaaacccc")]);
        install(&p, None, &dir, &store);
        let mut wanted: Vec<ChunkRef> = p.files.iter().flat_map(|f| f.chunks.clone()).collect();
        let absent = ChunkId::of(b"zzzz");
        wanted.push(ChunkRef {
            id: absent,
            size: 4,
        });
        for c in &wanted {
            let _ = store.remove(&c.id);
        }
        // The first copy of "aaaa" is damaged; the second one is good.
        std::fs::write(dir.join("a"), b"aaaXbbbb").unwrap();

        let mut reports = Vec::new();
        let recovered = harvest_with(&p, &dir, &wanted, &store, |done, total| {
            reports.push((done, total))
        })
        .unwrap();
        assert_eq!(recovered, 12);
        assert_eq!(reports.first(), Some(&(0, 12)));
        assert_eq!(reports.last(), Some(&(12, 12)));
        assert!(reports.windows(2).all(|w| w[0].0 <= w[1].0));
        assert!(!store.has(&absent));
    }
}
