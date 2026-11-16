//! HTTP chunk download engine: parallel, resumable at chunk granularity,
//! verified before anything reaches the store, optionally rate limited.

use crate::{
    client::{check, net},
    manifest::ChunkRef,
    progress::{Progress, ProgressFn, Source},
    store::ContentStore,
    ChunkId, Error, Result,
};
use futures_util::StreamExt;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{sync::Mutex, time::Instant};

/// Paces transfers to a byte rate shared by all concurrent downloads.
#[derive(Debug)]
pub struct RateLimiter {
    bytes_per_sec: u64,
    next_free: Mutex<Instant>,
}

impl RateLimiter {
    pub fn new(bytes_per_sec: u64) -> Self {
        RateLimiter {
            bytes_per_sec: bytes_per_sec.max(1),
            next_free: Mutex::new(Instant::now()),
        }
    }

    /// Wait until `bytes` may be consumed.
    pub async fn acquire(&self, bytes: u64) {
        let wake = {
            let mut next = self.next_free.lock().await;
            let start = (*next).max(Instant::now());
            *next = start + Duration::from_secs_f64(bytes as f64 / self.bytes_per_sec as f64);
            *next
        };
        tokio::time::sleep_until(wake).await;
    }
}

#[derive(Debug, Clone)]
pub struct DownloadOptions {
    /// Parallel chunk requests.
    pub concurrency: usize,
    /// Bytes per second across all requests; `None` is unlimited.
    pub rate_limit: Option<u64>,
    /// Attempts per chunk per mirror.
    pub attempts: u32,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        DownloadOptions {
            concurrency: 8,
            rate_limit: None,
            attempts: 4,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DownloadStats {
    /// Chunks fetched over the network.
    pub chunks: u64,
    pub bytes: u64,
    /// Chunks that were already in the store.
    pub skipped: u64,
}

/// Fetch every chunk in `chunks` that the store lacks.
///
/// `chunk_bases` are URL prefixes ending in `/` (the `chunks` member of each
/// mirror's server info); they are tried in order for every chunk. Because
/// completed chunks land in the store, calling this again after an
/// interruption resumes where it stopped.
pub async fn fetch_chunks(
    http: &reqwest::Client,
    chunk_bases: &[String],
    store: &ContentStore,
    chunks: &[ChunkRef],
    options: &DownloadOptions,
    progress: ProgressFn,
) -> Result<DownloadStats> {
    if chunk_bases.is_empty() {
        return Err(Error::other("no download source available"));
    }
    let mut seen = std::collections::HashSet::new();
    let unique: Vec<ChunkRef> = chunks
        .iter()
        .filter(|c| seen.insert(c.id))
        .copied()
        .collect();
    let todo: Vec<ChunkRef> = unique
        .iter()
        .filter(|c| !store.has(&c.id))
        .copied()
        .collect();
    let total: u64 = todo.iter().map(|c| c.size).sum();
    let done = Arc::new(AtomicU64::new(0));
    let limiter = options.rate_limit.map(|r| Arc::new(RateLimiter::new(r)));
    progress(Progress::download(Source::Http, 0, total));

    // The closure takes chunks by value: a closure over `&ChunkRef` that
    // returns an async block makes the enclosing future fail rustc's `Send`
    // check ("FnOnce is not general enough"), which Tauri commands require.
    let mut stream = futures_util::stream::iter(todo.iter().copied().map(|chunk| {
        let limiter = limiter.clone();
        let done = done.clone();
        let progress = progress.clone();
        async move {
            let report = |n: u64| {
                let so_far = done.fetch_add(n, Ordering::Relaxed) + n;
                progress(Progress::download(Source::Http, so_far.min(total), total));
            };
            let mut last_err = Error::other("no attempt made");
            for attempt in 0..options.attempts.max(1) {
                for base in chunk_bases {
                    let url = format!("{base}{}", chunk.id);
                    let mut counted = 0u64;
                    match fetch_one(http, &url, &chunk, limiter.as_deref(), |n| {
                        counted += n;
                        report(n)
                    })
                    .await
                    {
                        Ok(data) => {
                            store.put_trusted(&chunk.id, &data)?;
                            return Ok(chunk.size);
                        }
                        Err(e) => {
                            // Un-count the bytes of the failed attempt.
                            done.fetch_sub(counted, Ordering::Relaxed);
                            last_err = e;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(200 * (1 << attempt.min(5)))).await;
            }
            Err(last_err)
        }
    }))
    .buffer_unordered(options.concurrency.max(1));

    let mut stats = DownloadStats {
        skipped: (unique.len() - todo.len()) as u64,
        ..Default::default()
    };
    while let Some(result) = stream.next().await {
        stats.bytes += result?;
        stats.chunks += 1;
    }
    Ok(stats)
}

/// Download one chunk into memory; the hash is verified before returning.
async fn fetch_one(
    http: &reqwest::Client,
    url: &str,
    chunk: &ChunkRef,
    limiter: Option<&RateLimiter>,
    mut on_bytes: impl FnMut(u64),
) -> Result<Vec<u8>> {
    let resp = http.get(url).send().await.map_err(net)?;
    let mut resp = check(resp).await?;
    let mut data = Vec::with_capacity(chunk.size as usize);
    while let Some(part) = resp.chunk().await.map_err(net)? {
        if data.len() + part.len() > chunk.size as usize {
            return Err(Error::ChunkMismatch {
                id: chunk.id.to_hex(),
                actual: "body longer than declared size".into(),
            });
        }
        if let Some(limiter) = limiter {
            limiter.acquire(part.len() as u64).await;
        }
        data.extend_from_slice(&part);
        on_bytes(part.len() as u64);
    }
    let actual = ChunkId::of(&data);
    if actual != chunk.id || data.len() as u64 != chunk.size {
        return Err(Error::ChunkMismatch {
            id: chunk.id.to_hex(),
            actual: actual.to_hex(),
        });
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn rate_limiter_paces() {
        let limiter = RateLimiter::new(1000);
        let start = Instant::now();
        limiter.acquire(500).await;
        limiter.acquire(500).await;
        limiter.acquire(1000).await;
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(1990) && elapsed <= Duration::from_millis(2100),
            "{elapsed:?}"
        );
    }

    #[tokio::test]
    async fn no_sources_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ContentStore::open(tmp.path()).unwrap();
        let http = crate::client::http_client().unwrap();
        let r = fetch_chunks(
            &http,
            &[],
            &store,
            &[],
            &DownloadOptions::default(),
            crate::progress::no_progress(),
        )
        .await;
        assert!(r.is_err());
    }
}
