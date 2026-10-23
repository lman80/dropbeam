//! The media bridge: the per-call TURN server's socket on one side, the friend's
//! iroh connection on the other. Each packet the WebRTC engine relays (already
//! SRTP/DTLS-encrypted end to end by WebRTC) rides one unreliable QUIC DATAGRAM —
//! no retransmission, no head-of-line blocking — so WebRTC's own jitter buffer,
//! FEC and congestion control see a plain lossy network path. The connection is
//! iroh's: local network, direct internet or relay, whichever iroh picked.
//!
//! Datagram format (version 1):
//! ```text
//! 0      0xC7
//! 1..9   call tag (u64, random per call, from the signaling)
//! 9      0 = whole packet | 1 = fragment
//! whole:    from(6) to(6) payload
//! fragment: id(2) index(1) count(1) from(6) to(6) chunk
//! ```
//! from/to are the two sides' virtual TURN relayed addresses (IPv4 + port).
//! A packet larger than the path's datagram limit is split; losing one piece
//! loses the packet, exactly like an IP fragment.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use iroh::endpoint::Connection;
use tokio::net::UdpSocket;
use tokio::sync::{watch, Notify};

use super::turn::{Out, Turn};

pub const MARK: u8 = 0xC7;
const WHOLE: usize = 22;
const FRAG: usize = 26;
const MAX_PENDING: usize = 64;
const MAX_FRAGS: u8 = 8;

fn put_addr(v: &mut Vec<u8>, a: SocketAddrV4) {
    v.extend_from_slice(&a.ip().octets());
    v.extend_from_slice(&a.port().to_be_bytes());
}
fn get_addr(b: &[u8]) -> SocketAddrV4 {
    SocketAddrV4::new(Ipv4Addr::new(b[0], b[1], b[2], b[3]), u16::from_be_bytes([b[4], b[5]]))
}

/// Encode one relayed packet into one or more datagrams of at most `max` bytes.
pub fn encode(tag: u64, from: SocketAddrV4, to: SocketAddrV4, payload: &[u8], max: usize, next_id: &mut u16) -> Vec<Bytes> {
    if WHOLE + payload.len() <= max {
        let mut v = Vec::with_capacity(WHOLE + payload.len());
        v.push(MARK);
        v.extend_from_slice(&tag.to_be_bytes());
        v.push(0);
        put_addr(&mut v, from);
        put_addr(&mut v, to);
        v.extend_from_slice(payload);
        return vec![v.into()];
    }
    let room = max.saturating_sub(FRAG);
    if room < 64 { return vec![]; }
    let count = payload.len().div_ceil(room);
    if count > MAX_FRAGS as usize { return vec![]; }
    let id = *next_id;
    *next_id = next_id.wrapping_add(1);
    payload.chunks(room).enumerate().map(|(i, chunk)| {
        let mut v = Vec::with_capacity(FRAG + chunk.len());
        v.push(MARK);
        v.extend_from_slice(&tag.to_be_bytes());
        v.push(1);
        v.extend_from_slice(&id.to_be_bytes());
        v.push(i as u8);
        v.push(count as u8);
        put_addr(&mut v, from);
        put_addr(&mut v, to);
        v.extend_from_slice(chunk);
        Bytes::from(v)
    }).collect()
}

#[derive(Default)]
pub struct Reassembler {
    pending: HashMap<u16, (Instant, Vec<Option<Vec<u8>>>)>,
    pub dropped: u64,
}

impl Reassembler {
    /// One datagram for call `tag` → a whole packet (from, to, payload) once complete.
    pub fn push(&mut self, tag: u64, d: &[u8], now: Instant) -> Option<(SocketAddrV4, SocketAddrV4, Vec<u8>)> {
        if d.len() < WHOLE || d[0] != MARK || d[1..9] != tag.to_be_bytes() { return None; }
        match d[9] {
            0 => Some((get_addr(&d[10..16]), get_addr(&d[16..22]), d[22..].to_vec())),
            1 if d.len() >= FRAG => {
                let id = u16::from_be_bytes([d[10], d[11]]);
                let (idx, count) = (d[12], d[13]);
                if count == 0 || count > MAX_FRAGS || idx >= count { return None; }
                if !self.pending.contains_key(&id) && self.pending.len() >= MAX_PENDING {
                    // Evict the oldest unfinished packet.
                    if let Some(old) = self.pending.iter().min_by_key(|(_, (t, _))| *t).map(|(k, _)| *k) {
                        self.pending.remove(&old);
                        self.dropped += 1;
                    }
                }
                let entry = self.pending.entry(id).or_insert_with(|| (now, vec![None; count as usize]));
                if entry.1.len() != count as usize { self.pending.remove(&id); return None; }
                entry.1[idx as usize] = Some(d[FRAG..].to_vec());
                if entry.1.iter().all(|p| p.is_some()) {
                    let (_, parts) = self.pending.remove(&id)?;
                    let payload = parts.into_iter().flatten().flatten().collect();
                    return Some((get_addr(&d[14..20]), get_addr(&d[20..26]), payload));
                }
                None
            }
            _ => None,
        }
    }
    pub fn expire(&mut self, now: Instant) {
        let before = self.pending.len();
        self.pending.retain(|_, (t, _)| now.duration_since(*t) < Duration::from_secs(2));
        self.dropped += (before - self.pending.len()) as u64;
    }
}

#[derive(Default, Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaStats {
    pub packets_out: u64,
    pub packets_in: u64,
    pub bytes_out: u64,
    pub bytes_in: u64,
    pub fragmented_out: u64,
    pub send_dropped: u64,
    pub reassembly_dropped: u64,
    pub max_datagram: usize,
    /// ms since the last packet from the other side (None = nothing yet).
    pub last_rx_ms: Option<u64>,
    pub turn: super::turn::TurnCounters,
    /// Gaps between consecutive packets (ms), to tell where burstiness comes
    /// from: from the WebRTC engine into the bridge (`gaps_from_engine`) vs
    /// arriving from the other side (`gaps_from_peer`). Buckets: >30, >60, >120 ms.
    pub gaps_from_engine: [u64; 3],
    pub gaps_from_peer: [u64; 3],
    pub max_gap_from_engine_ms: u64,
    pub max_gap_from_peer_ms: u64,
    #[serde(skip)]
    pub last_rx: Option<Instant>,
    #[serde(skip)]
    pub last_tx: Option<Instant>,
}

/// A running bridge. Dropping it stops the task.
pub struct Bridge {
    pub port: u16,
    conn: watch::Sender<Option<Connection>>,
    stop: Arc<Notify>,
    pub stats: Arc<Mutex<MediaStats>>,
}

impl Drop for Bridge {
    fn drop(&mut self) { self.stop.notify_one(); }
}

impl Bridge {
    /// Bind the TURN server on 127.0.0.1 and start relaying. The connection to
    /// the other side is attached later (`attach`) — once a device answers.
    pub async fn start(username: &str, password: &str, caller: bool, tag: u64) -> std::io::Result<Bridge> {
        let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = sock.local_addr()?.port();
        let (conn_tx, conn_rx) = watch::channel(None);
        let stop = Arc::new(Notify::new());
        let stats = Arc::new(Mutex::new(MediaStats::default()));
        let turn = Turn::new(username, password, caller);
        tokio::spawn(run(sock, turn, conn_rx, stop.clone(), stats.clone(), tag));
        Ok(Bridge { port, conn: conn_tx, stop, stats })
    }

    /// Carry media over `conn` from now on (also used to move a call onto a
    /// replacement connection).
    pub fn attach(&self, conn: Connection) {
        let _ = self.conn.send(Some(conn));
    }

    pub fn snapshot(&self) -> MediaStats {
        let mut s = self.stats.lock().map(|s| s.clone()).unwrap_or_default();
        s.last_rx_ms = s.last_rx.map(|t| t.elapsed().as_millis() as u64);
        s
    }
}

async fn run(sock: UdpSocket, mut turn: Turn, mut conn_rx: watch::Receiver<Option<Connection>>, stop: Arc<Notify>, stats: Arc<Mutex<MediaStats>>, tag: u64) {
    let mut buf = vec![0u8; 65536];
    let mut reasm = Reassembler::default();
    let mut next_id = 0u16;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut conn: Option<Connection> = conn_rx.borrow().clone();
    loop {
        let reading = conn.clone();
        tokio::select! {
            _ = stop.notified() => break,
            changed = conn_rx.changed() => {
                if changed.is_err() { break; }
                conn = conn_rx.borrow_and_update().clone();
            }
            r = sock.recv_from(&mut buf) => {
                let Ok((n, src)) = r else { continue };
                // The server is bound to 127.0.0.1; only this machine can reach it.
                match turn.on_client(src, &buf[..n], Instant::now()) {
                    Some(Out::Reply(to, pkt)) => { let _ = sock.send_to(&pkt, to).await; }
                    Some(Out::Relay { from, to, data }) => {
                        let Some(c) = conn.as_ref() else { continue };
                        let max = c.max_datagram_size().unwrap_or(0);
                        let parts = encode(tag, from, to, &data, max, &mut next_id);
                        let mut s = stats.lock().unwrap_or_else(|p| p.into_inner());
                        s.max_datagram = max;
                        if parts.is_empty() { s.send_dropped += 1; continue; }
                        if parts.len() > 1 { s.fragmented_out += 1; }
                        s.packets_out += 1;
                        s.bytes_out += data.len() as u64;
                        let now = Instant::now();
                        if let Some(prev) = s.last_tx { let g = now.duration_since(prev).as_millis() as u64; let s = &mut *s; note_gap(&mut s.gaps_from_engine, &mut s.max_gap_from_engine_ms, g); }
                        s.last_tx = Some(now);
                        for p in parts {
                            if c.send_datagram(p).is_err() { s.send_dropped += 1; }
                        }
                    }
                    None => {}
                }
            }
            d = async move { match reading { Some(c) => c.read_datagram().await.ok(), None => std::future::pending().await } } => {
                let Some(d) = d else {
                    // Closed: wait for a replacement connection (or the end).
                    conn = None;
                    continue;
                };
                let now = Instant::now();
                if let Some((from, to, payload)) = reasm.push(tag, &d, now) {
                    {
                        let mut s = stats.lock().unwrap_or_else(|p| p.into_inner());
                        s.packets_in += 1;
                        s.bytes_in += payload.len() as u64;
                        if let Some(prev) = s.last_rx { let g = now.duration_since(prev).as_millis() as u64; let s = &mut *s; note_gap(&mut s.gaps_from_peer, &mut s.max_gap_from_peer_ms, g); }
                        s.last_rx = Some(now);
                    }
                    if let Some((client, pkt)) = turn.on_remote(from, to, &payload, now) {
                        let _ = sock.send_to(&pkt, client).await;
                    }
                }
            }
            _ = tick.tick() => {
                let now = Instant::now();
                turn.expire(now);
                reasm.expire(now);
                let mut s = stats.lock().unwrap_or_else(|p| p.into_inner());
                s.reassembly_dropped = reasm.dropped;
                s.turn = turn.counters;
            }
        }
    }
}

fn note_gap(b: &mut [u64; 3], max: &mut u64, g: u64) {
    if g > 30 { b[0] += 1; }
    if g > 60 { b[1] += 1; }
    if g > 120 { b[2] += 1; }
    *max = (*max).max(g);
}

/// For tests: the client side of a TURN server on a real socket.
#[allow(dead_code)]
pub(crate) fn local(port: u16) -> SocketAddr { SocketAddr::from((Ipv4Addr::LOCALHOST, port)) }

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> SocketAddrV4 { s.parse().unwrap() }

    #[test]
    fn whole_and_fragmented_round_trip() {
        let (from, to) = (a("10.77.0.1:40000"), a("10.77.0.2:40001"));
        let mut id = 0;
        let small = encode(7, from, to, b"hi", 1200, &mut id);
        assert_eq!(small.len(), 1);
        let mut r = Reassembler::default();
        let now = Instant::now();
        assert_eq!(r.push(7, &small[0], now), Some((from, to, b"hi".to_vec())));
        // The wrong call's datagrams are ignored.
        assert_eq!(r.push(8, &small[0], now), None);

        let big: Vec<u8> = (0..3000u32).map(|i| i as u8).collect();
        let parts = encode(7, from, to, &big, 1100, &mut id);
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| p.len() <= 1100));
        // Out of order still reassembles.
        assert_eq!(r.push(7, &parts[2], now), None);
        assert_eq!(r.push(7, &parts[0], now), None);
        assert_eq!(r.push(7, &parts[1], now), Some((from, to, big.clone())));

        // A lost piece expires and is counted.
        let parts = encode(7, from, to, &big, 1100, &mut id);
        assert_eq!(r.push(7, &parts[0], now), None);
        r.expire(now + Duration::from_secs(3));
        assert_eq!(r.dropped, 1);
        // Too big to ever fit is refused rather than sent as junk.
        assert!(encode(7, from, to, &vec![0; 20000], 1100, &mut id).is_empty());
    }

    #[test]
    fn malformed_datagrams_are_ignored() {
        let mut r = Reassembler::default();
        let now = Instant::now();
        assert_eq!(r.push(1, &[MARK; 5], now), None);
        let mut d = vec![MARK];
        d.extend_from_slice(&1u64.to_be_bytes());
        d.extend_from_slice(&[1, 0, 0, 5, 3]); // index 5 of 3
        d.resize(40, 0);
        assert_eq!(r.push(1, &d, now), None);
        d[12] = 0; d[13] = 200; // too many pieces
        assert_eq!(r.push(1, &d, now), None);
        d[9] = 9; // unknown kind
        assert_eq!(r.push(1, &d, now), None);
        // Bounded pending set.
        for i in 0..(MAX_PENDING as u16 + 10) {
            let mut f = vec![MARK];
            f.extend_from_slice(&1u64.to_be_bytes());
            f.push(1);
            f.extend_from_slice(&i.to_be_bytes());
            f.extend_from_slice(&[0, 2]);
            f.resize(40, 0);
            r.push(1, &f, now);
        }
        assert!(r.pending.len() <= MAX_PENDING);
        assert_eq!(r.dropped, 10);
    }
}
