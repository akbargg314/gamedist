//! BitTorrent v1 metainfo generation (BEP 3) with web seeds (BEP 19), plus a
//! small bencode codec used by the tracker.

use crate::{Error, Result};
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bencode {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Bencode>),
    Dict(BTreeMap<Vec<u8>, Bencode>),
}

impl Bencode {
    pub fn str(s: &str) -> Self {
        Bencode::Bytes(s.as_bytes().to_vec())
    }

    pub fn dict<const N: usize>(entries: [(&str, Bencode); N]) -> Self {
        Bencode::Dict(
            entries
                .into_iter()
                .map(|(k, v)| (k.as_bytes().to_vec(), v))
                .collect(),
        )
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        match self {
            Bencode::Int(i) => out.extend_from_slice(format!("i{i}e").as_bytes()),
            Bencode::Bytes(b) => encode_bytes(b, out),
            Bencode::List(items) => {
                out.push(b'l');
                for item in items {
                    item.encode_into(out);
                }
                out.push(b'e');
            }
            // BTreeMap<Vec<u8>, _> iterates in raw byte order, as BEP 3 requires.
            Bencode::Dict(map) => {
                out.push(b'd');
                for (k, v) in map {
                    encode_bytes(k, out);
                    v.encode_into(out);
                }
                out.push(b'e');
            }
        }
    }

    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut pos = 0;
        let value = decode_at(input, &mut pos, 0)?;
        if pos != input.len() {
            return Err(Error::invalid("bencode", "trailing data"));
        }
        Ok(value)
    }

    pub fn get(&self, key: &str) -> Option<&Bencode> {
        match self {
            Bencode::Dict(map) => map.get(key.as_bytes()),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Bencode::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Bencode::Int(i) => Some(*i),
            _ => None,
        }
    }
}

fn encode_bytes(b: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(b.len().to_string().as_bytes());
    out.push(b':');
    out.extend_from_slice(b);
}

fn decode_at(input: &[u8], pos: &mut usize, depth: usize) -> Result<Bencode> {
    let bad = |why: &str| Error::invalid("bencode", why.to_string());
    if depth > 64 {
        return Err(bad("nesting too deep"));
    }
    match input.get(*pos).copied() {
        Some(b'i') => {
            let end = input[*pos..]
                .iter()
                .position(|&c| c == b'e')
                .ok_or_else(|| bad("unterminated integer"))?
                + *pos;
            let text =
                std::str::from_utf8(&input[*pos + 1..end]).map_err(|_| bad("bad integer"))?;
            let value = text.parse().map_err(|_| bad("bad integer"))?;
            *pos = end + 1;
            Ok(Bencode::Int(value))
        }
        Some(b'l') => {
            *pos += 1;
            let mut items = Vec::new();
            while input.get(*pos) != Some(&b'e') {
                items.push(decode_at(input, pos, depth + 1)?);
            }
            *pos += 1;
            Ok(Bencode::List(items))
        }
        Some(b'd') => {
            *pos += 1;
            let mut map = BTreeMap::new();
            while input.get(*pos) != Some(&b'e') {
                let key = match decode_at(input, pos, depth + 1)? {
                    Bencode::Bytes(b) => b,
                    _ => return Err(bad("dictionary key is not a string")),
                };
                map.insert(key, decode_at(input, pos, depth + 1)?);
            }
            *pos += 1;
            Ok(Bencode::Dict(map))
        }
        Some(c) if c.is_ascii_digit() => {
            let colon = input[*pos..]
                .iter()
                .position(|&c| c == b':')
                .ok_or_else(|| bad("bad string length"))?
                + *pos;
            let len: usize = std::str::from_utf8(&input[*pos..colon])
                .ok()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| bad("bad string length"))?;
            let start = colon + 1;
            let end = start
                .checked_add(len)
                .filter(|&e| e <= input.len())
                .ok_or_else(|| bad("string runs past end of input"))?;
            *pos = end;
            Ok(Bencode::Bytes(input[start..end].to_vec()))
        }
        _ => Err(bad("unexpected byte or end of input")),
    }
}

/// Piece length for a payload: aims for at most ~2000 pieces, clamped to
/// 256 KiB – 8 MiB (powers of two).
pub fn piece_length_for(total_size: u64) -> u32 {
    let mut len: u64 = 256 * 1024;
    while len < 8 * 1024 * 1024 && total_size / len > 2000 {
        len *= 2;
    }
    len as u32
}

/// Streaming SHA-1 piece hasher: feed the concatenated file data in order.
pub struct PieceHasher {
    piece_length: usize,
    filled: usize,
    current: Sha1,
    pieces: Vec<u8>,
}

impl PieceHasher {
    pub fn new(piece_length: u32) -> Self {
        PieceHasher {
            piece_length: piece_length as usize,
            filled: 0,
            current: Sha1::new(),
            pieces: Vec::new(),
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            let take = (self.piece_length - self.filled).min(data.len());
            self.current.update(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == self.piece_length {
                self.flush();
            }
        }
    }

    fn flush(&mut self) {
        let digest = std::mem::take(&mut self.current).finalize();
        self.pieces.extend_from_slice(&digest);
        self.filled = 0;
    }

    /// Concatenated 20-byte piece hashes.
    pub fn finish(mut self) -> Vec<u8> {
        if self.filled > 0 {
            self.flush();
        }
        self.pieces
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentFile {
    /// Manifest-style relative path (`a/b.bin`).
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metainfo {
    pub bytes: Vec<u8>,
    pub info_hash: [u8; 20],
}

impl Metainfo {
    pub fn info_hash_hex(&self) -> String {
        data_encoding::HEXLOWER.encode(&self.info_hash)
    }
}

/// Build a multi-file v1 torrent.
pub fn build_metainfo(
    name: &str,
    files: &[TorrentFile],
    piece_length: u32,
    pieces: Vec<u8>,
    trackers: &[String],
    web_seeds: &[String],
    comment: &str,
) -> Result<Metainfo> {
    let total: u64 = files.iter().map(|f| f.size).sum();
    let expected_pieces = total.div_ceil(piece_length as u64) as usize;
    if pieces.len() != expected_pieces * 20 {
        return Err(Error::invalid(
            "torrent",
            format!(
                "expected {expected_pieces} piece hashes, got {} bytes",
                pieces.len()
            ),
        ));
    }
    let file_list = files
        .iter()
        .map(|f| {
            Bencode::dict([
                ("length", Bencode::Int(f.size as i64)),
                (
                    "path",
                    Bencode::List(f.path.split('/').map(Bencode::str).collect()),
                ),
            ])
        })
        .collect();
    let info = Bencode::dict([
        ("files", Bencode::List(file_list)),
        ("name", Bencode::str(name)),
        ("piece length", Bencode::Int(piece_length as i64)),
        ("pieces", Bencode::Bytes(pieces)),
    ]);
    let info_hash: [u8; 20] = Sha1::digest(info.encode()).into();

    let mut root = BTreeMap::new();
    root.insert(b"info".to_vec(), info);
    if let Some(first) = trackers.first() {
        root.insert(b"announce".to_vec(), Bencode::str(first));
        if trackers.len() > 1 {
            root.insert(
                b"announce-list".to_vec(),
                Bencode::List(
                    trackers
                        .iter()
                        .map(|t| Bencode::List(vec![Bencode::str(t)]))
                        .collect(),
                ),
            );
        }
    }
    if !web_seeds.is_empty() {
        root.insert(
            b"url-list".to_vec(),
            Bencode::List(web_seeds.iter().map(|u| Bencode::str(u)).collect()),
        );
    }
    if !comment.is_empty() {
        root.insert(b"comment".to_vec(), Bencode::str(comment));
    }
    root.insert(b"created by".to_vec(), Bencode::str("gamedist"));
    Ok(Metainfo {
        bytes: Bencode::Dict(root).encode(),
        info_hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bencode_roundtrip() {
        let v = Bencode::dict([
            ("b", Bencode::Int(-42)),
            (
                "a",
                Bencode::List(vec![Bencode::str("spam"), Bencode::Int(0)]),
            ),
            ("c", Bencode::Bytes(vec![0, 255, b':'])),
        ]);
        let enc = v.encode();
        assert_eq!(&enc[..], b"d1:al4:spami0ee1:bi-42e1:c3:\x00\xff:e");
        assert_eq!(Bencode::decode(&enc).unwrap(), v);
        assert_eq!(v.get("b").and_then(Bencode::as_int), Some(-42));
    }

    #[test]
    fn bencode_rejects_garbage() {
        for bad in [
            &b"i12"[..],
            b"5:abc",
            b"l",
            b"d1:a",
            b"x",
            b"i1ei2e",
            b"di1ei2ee",
            b"",
        ] {
            assert!(Bencode::decode(bad).is_err(), "{:?}", bad);
        }
        let deep = [vec![b'l'; 100], vec![b'e'; 100]].concat();
        assert!(Bencode::decode(&deep).is_err());
    }

    #[test]
    fn piece_hasher_matches_manual_hashing() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let mut h = PieceHasher::new(256);
        // Feed in awkward slices.
        for part in data.chunks(77) {
            h.update(part);
        }
        let pieces = h.finish();
        let expected: Vec<u8> = data
            .chunks(256)
            .flat_map(|p| Sha1::digest(p).to_vec())
            .collect();
        assert_eq!(pieces, expected);
        assert_eq!(pieces.len(), 4 * 20);
        assert!(PieceHasher::new(256).finish().is_empty());
    }

    #[test]
    fn piece_length_scales() {
        assert_eq!(piece_length_for(0), 256 * 1024);
        assert_eq!(piece_length_for(100 * 1024 * 1024), 256 * 1024);
        assert_eq!(piece_length_for(100 * 1024 * 1024 * 1024), 8 * 1024 * 1024);
        assert!(piece_length_for(4 * 1024 * 1024 * 1024).is_power_of_two());
    }

    #[test]
    fn metainfo_structure() {
        let files = vec![
            TorrentFile {
                path: "game.bin".into(),
                size: 300,
            },
            TorrentFile {
                path: "data/a.txt".into(),
                size: 100,
            },
        ];
        let mut h = PieceHasher::new(256);
        h.update(&[1u8; 400]);
        let m = build_metainfo(
            "files",
            &files,
            256,
            h.finish(),
            &["https://host/announce".into()],
            &["https://host/games/x/releases/1.0.0/linux-x86_64/".into()],
            "x 1.0.0",
        )
        .unwrap();
        let root = Bencode::decode(&m.bytes).unwrap();
        let info = root.get("info").unwrap();
        assert_eq!(info.get("name").unwrap().as_bytes().unwrap(), b"files");
        assert_eq!(info.get("pieces").unwrap().as_bytes().unwrap().len(), 40);
        assert_eq!(<[u8; 20]>::from(Sha1::digest(info.encode())), m.info_hash);
        assert_eq!(m.info_hash_hex().len(), 40);
        assert!(root.get("url-list").is_some());
        assert_eq!(
            root.get("announce").unwrap().as_bytes().unwrap(),
            b"https://host/announce"
        );
        match info.get("files").unwrap() {
            Bencode::List(l) => {
                assert_eq!(l.len(), 2);
                assert_eq!(
                    l[1].get("path").unwrap(),
                    &Bencode::List(vec![Bencode::str("data"), Bencode::str("a.txt")])
                );
            }
            _ => panic!(),
        }
        // Wrong number of pieces is refused.
        assert!(build_metainfo("files", &files, 256, vec![0; 20], &[], &[], "").is_err());
    }
}
