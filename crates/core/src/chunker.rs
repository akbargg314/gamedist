//! Content-defined chunking (FastCDC 2020) with BLAKE3 chunk IDs.

use crate::{error::IoContext, manifest::ChunkRef, ChunkId, Error, Result};
use std::{io::Read, path::Path};

pub const MIN_CHUNK: u32 = 512 * 1024;
pub const AVG_CHUNK: u32 = 2 * 1024 * 1024;
pub const MAX_CHUNK: u32 = 8 * 1024 * 1024;

/// Chunk sizes; the protocol values unless overridden (tests use small ones).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkParams {
    pub min: u32,
    pub avg: u32,
    pub max: u32,
}

impl Default for ChunkParams {
    fn default() -> Self {
        ChunkParams {
            min: MIN_CHUNK,
            avg: AVG_CHUNK,
            max: MAX_CHUNK,
        }
    }
}

impl ChunkParams {
    /// Small chunks for tests and tiny games.
    pub const fn small() -> Self {
        ChunkParams {
            min: 1024,
            avg: 4096,
            max: 16384,
        }
    }
}

/// Split a stream into chunks, calling `sink(offset, bytes)` for each.
/// Returns the chunk list in order.
pub fn chunk_reader<R: Read>(
    reader: R,
    params: ChunkParams,
    mut sink: impl FnMut(u64, ChunkId, &[u8]) -> Result<()>,
) -> Result<Vec<ChunkRef>> {
    let mut out = Vec::new();
    for item in fastcdc::v2020::StreamCDC::new(reader, params.min, params.avg, params.max) {
        let chunk = item.map_err(|e| match e {
            fastcdc::v2020::Error::IoError(io) => Error::io("reading input for chunking", io),
            other => Error::other(format!("chunker: {other}")),
        })?;
        let id = ChunkId::of(&chunk.data);
        sink(chunk.offset, id, &chunk.data)?;
        out.push(ChunkRef {
            id,
            size: chunk.data.len() as u64,
        });
    }
    Ok(out)
}

/// Chunk a file on disk without keeping its contents.
pub fn chunk_file(path: &Path, params: ChunkParams) -> Result<Vec<ChunkRef>> {
    let file = std::fs::File::open(path).at(path)?;
    chunk_reader(file, params, |_, _, _| Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
        // xorshift64*: deterministic, incompressible enough for chunking tests.
        let mut x = seed.wrapping_mul(0x9E3779B97F4A7C15) | 1;
        let mut out = Vec::with_capacity(len + 8);
        while out.len() < len {
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            out.extend_from_slice(&x.wrapping_mul(0x2545F4914F6CDD1D).to_le_bytes());
        }
        out.truncate(len);
        out
    }

    fn chunk(data: &[u8], params: ChunkParams) -> Vec<ChunkRef> {
        chunk_reader(data, params, |_, _, _| Ok(())).unwrap()
    }

    #[test]
    fn deterministic_and_covers_input() {
        let data = pseudo_random(300_000, 1);
        let p = ChunkParams::small();
        let a = chunk(&data, p);
        let b = chunk(&data, p);
        assert_eq!(a, b);
        assert!(a.len() > 10);
        assert_eq!(a.iter().map(|c| c.size).sum::<u64>(), data.len() as u64);
        let mut off = 0usize;
        for c in &a {
            let slice = &data[off..off + c.size as usize];
            assert_eq!(ChunkId::of(slice), c.id);
            assert!(c.size <= p.max as u64);
            off += c.size as usize;
        }
        // All but the last chunk respect the minimum.
        assert!(a[..a.len() - 1].iter().all(|c| c.size >= p.min as u64));
    }

    #[test]
    fn empty_input_has_no_chunks() {
        assert!(chunk(&[], ChunkParams::small()).is_empty());
    }

    #[test]
    fn small_input_is_one_chunk() {
        let c = chunk(b"tiny", ChunkParams::default());
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].id, ChunkId::of(b"tiny"));
    }

    #[test]
    fn insertion_only_disturbs_nearby_chunks() {
        let data = pseudo_random(400_000, 7);
        let mut edited = data.clone();
        edited.splice(200_000..200_000, b"INSERTED BYTES".iter().copied());
        let p = ChunkParams::small();
        let a: std::collections::HashSet<_> = chunk(&data, p).into_iter().map(|c| c.id).collect();
        let b = chunk(&edited, p);
        let new: Vec<_> = b.iter().filter(|c| !a.contains(&c.id)).collect();
        assert!(
            new.len() <= 4,
            "an insertion should invalidate only a few chunks, got {} of {}",
            new.len(),
            b.len()
        );
    }

    #[test]
    fn protocol_parameters_on_real_sized_input() {
        let data = pseudo_random(20 * 1024 * 1024, 3);
        let c = chunk(&data, ChunkParams::default());
        assert!(c.len() >= 3 && c.len() <= 40, "{}", c.len());
        assert!(c.iter().all(|c| c.size <= crate::MAX_CHUNK_SIZE));
    }

    #[test]
    fn sink_errors_propagate() {
        let data = pseudo_random(50_000, 2);
        let r = chunk_reader(&data[..], ChunkParams::small(), |_, _, _| {
            Err(Error::other("stop"))
        });
        assert!(r.is_err());
    }
}
