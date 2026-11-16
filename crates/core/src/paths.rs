//! Manifest path validation. Manifest paths are untrusted input.

use crate::{Error, Result};
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Names the launcher uses inside an install directory.
pub const RESERVED_TOP_LEVEL: &[&str] =
    &[".gamedist-staging", ".gamedist-backup", "installed.json"];

/// Validate a relative, forward-slash manifest path (SPEC §4.3 rule 2).
pub fn validate_rel_path(path: &str) -> Result<()> {
    let fail = |reason: &'static str| {
        Err(Error::UnsafePath {
            path: path.to_string(),
            reason,
        })
    };
    if path.is_empty() {
        return fail("empty path");
    }
    if path.len() > 1024 {
        return fail("path too long");
    }
    if path.starts_with('/') {
        return fail("absolute path");
    }
    if path.contains('\\') {
        return fail("backslash in path");
    }
    if path.contains(':') {
        return fail("colon in path (drive letter or alternate data stream)");
    }
    if path.chars().any(|c| c.is_control()) {
        return fail("control character in path");
    }
    if path.nfc().ne(path.chars()) {
        return fail("path is not NFC-normalized");
    }
    for (i, comp) in path.split('/').enumerate() {
        if comp.is_empty() {
            return fail("empty path component");
        }
        if comp == "." || comp == ".." {
            return fail("'.' or '..' path component");
        }
        if comp.ends_with('.') || comp.ends_with(' ') {
            return fail("component ends with a dot or space");
        }
        let stem = comp.split('.').next().unwrap_or(comp);
        if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
            return fail("Windows reserved device name");
        }
        if i == 0
            && RESERVED_TOP_LEVEL
                .iter()
                .any(|r| r.eq_ignore_ascii_case(comp))
        {
            return fail("name reserved by the launcher");
        }
    }
    Ok(())
}

/// Join a validated manifest path onto `root`.
pub fn join(root: &Path, rel: &str) -> Result<PathBuf> {
    validate_rel_path(rel)?;
    let mut out = root.to_path_buf();
    for comp in rel.split('/') {
        out.push(comp);
    }
    Ok(out)
}

/// Join `rel` onto `root` and make sure no existing component between `root`
/// and the target is a symlink (which could redirect writes outside `root`).
pub fn join_no_symlinks(root: &Path, rel: &str) -> Result<PathBuf> {
    validate_rel_path(rel)?;
    let mut out = root.to_path_buf();
    for comp in rel.split('/') {
        out.push(comp);
        match std::fs::symlink_metadata(&out) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(Error::UnsafePath {
                    path: rel.to_string(),
                    reason: "a component of the path is a symlink",
                })
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Convert an OS-relative path into manifest form (NFC, forward slashes).
pub fn to_manifest_path(rel: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for comp in rel.components() {
        match comp {
            std::path::Component::Normal(os) => {
                let s = os.to_str().ok_or_else(|| Error::UnsafePath {
                    path: rel.display().to_string(),
                    reason: "file name is not valid Unicode",
                })?;
                parts.push(s.nfc().collect::<String>());
            }
            _ => {
                return Err(Error::UnsafePath {
                    path: rel.display().to_string(),
                    reason: "unexpected path component",
                })
            }
        }
    }
    let joined = parts.join("/");
    validate_rel_path(&joined)?;
    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_paths() {
        for p in [
            "CoolGame.exe",
            "data/level 1/map.bin",
            "assets/音楽/テーマ.ogg",
            ".hidden/file",
            "a.b.c/d",
        ] {
            validate_rel_path(p).unwrap_or_else(|e| panic!("{p}: {e}"));
        }
    }

    #[test]
    fn rejects_dangerous_paths() {
        for p in [
            "",
            "/etc/passwd",
            "../escape",
            "a/../../b",
            "a/./b",
            "a//b",
            "a/",
            "a\\b",
            "C:/Windows/system32",
            "C:evil",
            "file.txt:stream",
            "a/NUL",
            "con.txt",
            "dir/COM1.dat",
            "trailing./x",
            "trailing /x",
            "nul\0byte",
            "tab\tname",
            "installed.json",
            ".gamedist-staging/x",
            "e\u{301}.txt", // decomposed é is not NFC
        ] {
            assert!(validate_rel_path(p).is_err(), "should reject {p:?}");
        }
        // Reserved launcher names are only reserved at the top level.
        validate_rel_path("sub/installed.json").unwrap();
    }

    #[test]
    fn join_stays_inside_root() {
        let root = Path::new("/games/x");
        assert_eq!(
            join(root, "a/b.txt").unwrap(),
            PathBuf::from("/games/x/a/b.txt")
        );
        assert!(join(root, "../y").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_components() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("link")).unwrap();
        assert!(join_no_symlinks(tmp.path(), "link/file").is_err());
        assert!(join_no_symlinks(tmp.path(), "link").is_err());
        join_no_symlinks(tmp.path(), "real/file").unwrap();
    }

    #[test]
    fn manifest_path_conversion() {
        assert_eq!(
            to_manifest_path(Path::new("a").join("b.txt").as_path()).unwrap(),
            "a/b.txt"
        );
        assert_eq!(
            to_manifest_path(Path::new("e\u{301}.txt")).unwrap(),
            "\u{e9}.txt"
        );
        assert!(to_manifest_path(Path::new("../x")).is_err());
    }
}
