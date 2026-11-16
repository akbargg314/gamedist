//! Minimal HTTP BitTorrent tracker (BEP 3, compact peers per BEP 23) that only
//! serves this server's own torrents.

use crate::{auth::ClientIp, metrics::Metrics, state::St};
use axum::{
    extract::{RawQuery, State},
    http::header,
    response::{IntoResponse, Response},
};
use gamedist_core::torrent::Bencode;
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
struct Peer {
    ip: IpAddr,
    port: u16,
    seeder: bool,
    seen: Instant,
}

/// Peers of one torrent, keyed by peer id.
type Swarm = HashMap<Vec<u8>, Peer>;

pub struct Tracker {
    interval: u32,
    /// Swarms keyed by info hash.
    swarms: Mutex<HashMap<[u8; 20], Swarm>>,
}

impl Tracker {
    pub fn new(interval: u32) -> Self {
        Tracker {
            interval: interval.max(1),
            swarms: Mutex::new(HashMap::new()),
        }
    }

    pub fn peer_count(&self) -> usize {
        self.swarms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(HashMap::len)
            .sum()
    }

    /// Record an announce and return `(seeders, leechers, other peers)`.
    fn announce(&self, req: &Announce, ip: IpAddr, now: Instant) -> (i64, i64, Vec<(IpAddr, u16)>) {
        let ttl = Duration::from_secs(self.interval as u64 * 2 + 30);
        let mut swarms = self.swarms.lock().unwrap_or_else(|e| e.into_inner());
        let swarm = swarms.entry(req.info_hash).or_default();
        swarm.retain(|_, p| now.duration_since(p.seen) < ttl);
        if req.event.as_deref() == Some("stopped") {
            swarm.remove(&req.peer_id);
        } else {
            swarm.insert(
                req.peer_id.clone(),
                Peer {
                    ip,
                    port: req.port,
                    seeder: req.left == 0,
                    seen: now,
                },
            );
        }
        let seeders = swarm.values().filter(|p| p.seeder).count() as i64;
        let leechers = swarm.len() as i64 - seeders;
        let peers = swarm
            .iter()
            .filter(|(id, _)| **id != req.peer_id)
            // A seeder has no use for other seeders.
            .filter(|(_, p)| !(req.left == 0 && p.seeder))
            .take(req.numwant)
            .map(|(_, p)| (p.ip, p.port))
            .collect();
        if swarm.is_empty() {
            swarms.remove(&req.info_hash);
        }
        (seeders, leechers, peers)
    }
}

#[derive(Debug, PartialEq)]
struct Announce {
    info_hash: [u8; 20],
    peer_id: Vec<u8>,
    port: u16,
    left: u64,
    event: Option<String>,
    numwant: usize,
}

/// Percent-decode to raw bytes (info hashes are not UTF-8).
fn percent_decode(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

fn parse_announce(query: &str) -> Result<Announce, &'static str> {
    let mut params: HashMap<&str, Vec<u8>> = HashMap::new();
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            params.insert(k, percent_decode(v));
        }
    }
    let text = |key: &str| {
        params
            .get(key)
            .and_then(|v| std::str::from_utf8(v).ok())
            .map(str::to_string)
    };
    let info_hash: [u8; 20] = params
        .get("info_hash")
        .and_then(|v| v.as_slice().try_into().ok())
        .ok_or("info_hash must be 20 bytes")?;
    let peer_id = params.get("peer_id").cloned().ok_or("missing peer_id")?;
    if peer_id.len() != 20 {
        return Err("peer_id must be 20 bytes");
    }
    let port: u16 = text("port")
        .and_then(|p| p.parse().ok())
        .filter(|p| *p != 0)
        .ok_or("missing or invalid port")?;
    Ok(Announce {
        info_hash,
        peer_id,
        port,
        left: text("left").and_then(|v| v.parse().ok()).unwrap_or(1),
        event: text("event"),
        numwant: text("numwant")
            .and_then(|v| v.parse().ok())
            .unwrap_or(50usize)
            .min(50),
    })
}

fn bencoded(value: Bencode) -> Response {
    ([(header::CONTENT_TYPE, "text/plain")], value.encode()).into_response()
}

fn failure(reason: &str) -> Response {
    bencoded(Bencode::dict([("failure reason", Bencode::str(reason))]))
}

pub async fn announce(
    State(st): State<St>,
    ClientIp(ip): ClientIp,
    RawQuery(query): RawQuery,
) -> Response {
    Metrics::add(&st.metrics.announces, 1);
    let req = match parse_announce(query.as_deref().unwrap_or("")) {
        Ok(req) => req,
        Err(reason) => return failure(reason),
    };
    let known: Result<Option<i64>, _> =
        sqlx::query_scalar("SELECT 1 FROM torrents WHERE info_hash = ? LIMIT 1")
            .bind(data_encoding::HEXLOWER.encode(&req.info_hash))
            .fetch_optional(&st.db)
            .await;
    if !matches!(known, Ok(Some(_))) {
        return failure("unregistered torrent");
    }
    let (seeders, leechers, peers) = st.tracker.announce(&req, ip, Instant::now());
    let mut v4 = Vec::new();
    let mut v6 = Vec::new();
    for (ip, port) in peers {
        match ip {
            IpAddr::V4(ip) => {
                v4.extend_from_slice(&ip.octets());
                v4.extend_from_slice(&port.to_be_bytes());
            }
            IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
                Some(ip) => {
                    v4.extend_from_slice(&ip.octets());
                    v4.extend_from_slice(&port.to_be_bytes());
                }
                None => {
                    v6.extend_from_slice(&ip.octets());
                    v6.extend_from_slice(&port.to_be_bytes());
                }
            },
        }
    }
    bencoded(Bencode::dict([
        ("interval", Bencode::Int(st.tracker.interval as i64)),
        ("complete", Bencode::Int(seeders)),
        ("incomplete", Bencode::Int(leechers)),
        ("peers", Bencode::Bytes(v4)),
        ("peers6", Bencode::Bytes(v6)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(peer: u8, left: u64, event: Option<&str>) -> Announce {
        Announce {
            info_hash: [7; 20],
            peer_id: vec![peer; 20],
            port: 6881 + peer as u16,
            left,
            event: event.map(str::to_string),
            numwant: 50,
        }
    }

    #[test]
    fn decode_binary_info_hash() {
        assert_eq!(
            percent_decode("%00%ffab%2"),
            vec![0, 255, b'a', b'b', b'%', b'2']
        );
        assert_eq!(percent_decode("a+b%41"), b"a bA");
        assert_eq!(percent_decode("%"), b"%");
        let q = format!(
            "info_hash={}&peer_id={}&port=6881&left=0&event=started&numwant=500",
            "%01".repeat(20),
            "-GD0001-abcdefghijkl"
        );
        let a = parse_announce(&q).unwrap();
        assert_eq!(a.info_hash, [1; 20]);
        assert_eq!(a.left, 0);
        assert_eq!(a.numwant, 50);
        assert_eq!(a.event.as_deref(), Some("started"));
        assert!(parse_announce("info_hash=short&peer_id=x&port=1").is_err());
        assert!(parse_announce(&q.replace("port=6881", "port=0")).is_err());
    }

    #[test]
    fn swarm_bookkeeping() {
        let tracker = Tracker::new(60);
        let now = Instant::now();
        let ip = |n: u8| IpAddr::from([10, 0, 0, n]);

        let (s, l, peers) = tracker.announce(&req(1, 0, Some("started")), ip(1), now);
        assert_eq!((s, l), (1, 0));
        assert!(peers.is_empty());

        let (s, l, peers) = tracker.announce(&req(2, 100, None), ip(2), now);
        assert_eq!((s, l), (1, 1));
        assert_eq!(peers, vec![(ip(1), 6882)]);

        // Seeders are not handed other seeders.
        let (_, _, peers) = tracker.announce(&req(3, 0, None), ip(3), now);
        assert_eq!(peers, vec![(ip(2), 6883)]);

        tracker.announce(&req(2, 100, Some("stopped")), ip(2), now);
        assert_eq!(tracker.peer_count(), 2);

        // Peers expire.
        let later = now + Duration::from_secs(60 * 2 + 31);
        let (s, l, peers) = tracker.announce(&req(4, 5, None), ip(4), later);
        assert_eq!((s, l), (0, 1));
        assert!(peers.is_empty());
        assert_eq!(tracker.peer_count(), 1);
    }
}
