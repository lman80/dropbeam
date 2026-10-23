//! The call bridge's TURN server: a minimal RFC 8656 server for ONE call, bound
//! to 127.0.0.1, that the webview's (or the iOS app's) WebRTC engine uses as its
//! only ICE server (`iceTransportPolicy: "relay"`). Nothing is relayed to the
//! internet: every allocation's "relayed address" is virtual (10.77.0.x), and a
//! packet the client sends to the other side's virtual address is handed to the
//! media bridge (media.rs), which carries it inside the authenticated iroh
//! connection as a QUIC datagram. Packets from the other side come back the same
//! way and are delivered as Data indications / ChannelData.
//!
//! Pure state machine (no sockets) so the protocol is unit-tested; the async
//! loop lives in media.rs. Long-term credentials are random per call and only
//! ever handed to this app's own call screen.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant};

use super::stun::{self, attr, method, Builder, Class};

pub const REALM: &str = "dropbeam";
const MAX_LIFETIME: u32 = 3600;
const DEFAULT_LIFETIME: u32 = 600;
const PERMISSION_LIFETIME: Duration = Duration::from_secs(300);
const CHANNEL_LIFETIME: Duration = Duration::from_secs(600);
/// A WebRTC engine makes one allocation per local network interface; anything
/// past this is not a call.
const MAX_ALLOCATIONS: usize = 16;
const FIRST_PORT: u16 = 40000;

/// The virtual address space: caller 10.77.0.1, callee 10.77.0.2.
pub fn virtual_ip(caller: bool) -> Ipv4Addr {
    if caller { Ipv4Addr::new(10, 77, 0, 1) } else { Ipv4Addr::new(10, 77, 0, 2) }
}

#[derive(Debug)]
struct Allocation {
    relayed: SocketAddrV4,
    expires: Instant,
    perms: HashMap<Ipv4Addr, Instant>,
    channels: HashMap<u16, (SocketAddrV4, Instant)>,
}

impl Allocation {
    fn channel_for(&self, peer: SocketAddrV4, now: Instant) -> Option<u16> {
        self.channels.iter().find(|(_, (p, exp))| *p == peer && *exp > now).map(|(c, _)| *c)
    }
}

/// What the server wants done after a packet.
#[derive(Debug, PartialEq, Eq)]
pub enum Out {
    /// A reply to the WebRTC client.
    Reply(SocketAddr, Vec<u8>),
    /// Application data for the other side: from our virtual relayed address to
    /// theirs.
    Relay { from: SocketAddrV4, to: SocketAddrV4, data: Vec<u8> },
}

#[derive(Default, Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnCounters {
    pub allocations: usize,
    pub auth_failures: u64,
    pub dropped: u64,
}

pub struct Turn {
    username: String,
    key: [u8; 16],
    nonce: String,
    local_ip: Ipv4Addr,
    peer_ip: Ipv4Addr,
    allocs: HashMap<SocketAddr, Allocation>,
    next_port: u16,
    pub counters: TurnCounters,
}

impl Turn {
    /// `caller` picks this side's virtual address; the other side is the only
    /// peer packets may go to.
    pub fn new(username: &str, password: &str, caller: bool) -> Self {
        Turn {
            username: username.to_string(),
            key: stun::long_term_key(username, REALM, password),
            nonce: hex::encode(rand::random::<[u8; 12]>()),
            local_ip: virtual_ip(caller),
            peer_ip: virtual_ip(!caller),
            allocs: HashMap::new(),
            next_port: FIRST_PORT,
            counters: TurnCounters::default(),
        }
    }

    pub fn local_ip(&self) -> Ipv4Addr { self.local_ip }

    /// One packet from the WebRTC client at `src`.
    pub fn on_client(&mut self, src: SocketAddr, pkt: &[u8], now: Instant) -> Option<Out> {
        if let Some((ch, data)) = stun::channel_data(pkt) {
            let a = self.allocs.get(&src).filter(|a| a.expires > now)?;
            let (peer, exp) = *a.channels.get(&ch)?;
            if exp <= now { self.counters.dropped += 1; return None; }
            return Some(Out::Relay { from: a.relayed, to: peer, data: data.to_vec() });
        }
        let m = stun::parse(pkt)?;
        match (m.method, m.class) {
            (method::BINDING, Class::Request) => Some(Out::Reply(src, Builder::new(method::BINDING, Class::Success, m.txid)
                .xor_addr(attr::XOR_MAPPED_ADDRESS, src).fingerprint().build())),
            (method::SEND, Class::Indication) => {
                let a = self.allocs.get(&src).filter(|a| a.expires > now)?;
                let peer = as_v4(m.xor_addr(attr::XOR_PEER_ADDRESS)?)?;
                let data = m.get(attr::DATA)?;
                if peer.ip() != &self.peer_ip || !a.perms.get(peer.ip()).is_some_and(|e| *e > now) {
                    self.counters.dropped += 1;
                    return None;
                }
                Some(Out::Relay { from: a.relayed, to: peer, data: data.to_vec() })
            }
            (_, Class::Request) => self.request(src, &m, now).map(|r| Out::Reply(src, r)),
            _ => None,
        }
    }

    /// Requests that need the long-term credential (RFC 8489 §9.2).
    fn request(&mut self, src: SocketAddr, m: &stun::Msg<'_>, now: Instant) -> Option<Vec<u8>> {
        let err = |code: u16, reason: &str| Builder::new(m.method, Class::Error, m.txid).error(code, reason);
        if m.get(attr::MESSAGE_INTEGRITY).is_none() {
            return Some(err(401, "Unauthorized").add(attr::REALM, REALM.as_bytes()).add(attr::NONCE, self.nonce.as_bytes()).build());
        }
        let user_ok = m.str(attr::USERNAME) == Some(self.username.as_str()) && m.str(attr::REALM) == Some(REALM);
        if !user_ok || !m.integrity_ok(&self.key) {
            self.counters.auth_failures += 1;
            return Some(err(401, "Unauthorized").add(attr::REALM, REALM.as_bytes()).add(attr::NONCE, self.nonce.as_bytes()).build());
        }
        if m.str(attr::NONCE) != Some(self.nonce.as_str()) {
            return Some(err(438, "Stale Nonce").add(attr::REALM, REALM.as_bytes()).add(attr::NONCE, self.nonce.as_bytes()).integrity(&self.key).build());
        }
        let key = self.key;
        let ok = |b: Builder| b.integrity(&key).build();
        let lifetime = |m: &stun::Msg<'_>| m.get(attr::LIFETIME).filter(|v| v.len() == 4)
            .map(|v| u32::from_be_bytes([v[0], v[1], v[2], v[3]])).unwrap_or(DEFAULT_LIFETIME);
        match m.method {
            method::ALLOCATE => {
                if m.get(attr::REQUESTED_TRANSPORT).and_then(|v| v.first()) != Some(&17) {
                    return Some(ok(err(442, "Unsupported Transport Protocol")));
                }
                if m.get(attr::REQUESTED_ADDRESS_FAMILY).and_then(|v| v.first()).is_some_and(|f| *f != 0x01) {
                    return Some(ok(err(440, "Address Family not Supported")));
                }
                let life = lifetime(m).clamp(DEFAULT_LIFETIME, MAX_LIFETIME);
                self.allocs.retain(|_, a| a.expires > now);
                let relayed = if let Some(a) = self.allocs.get_mut(&src) {
                    // A retransmitted Allocate (or a client re-allocating on the
                    // same 5-tuple): same relayed address, renewed.
                    a.expires = now + Duration::from_secs(life as u64);
                    a.relayed
                } else {
                    if self.allocs.len() >= MAX_ALLOCATIONS || self.next_port == u16::MAX {
                        return Some(ok(err(486, "Allocation Quota Reached")));
                    }
                    let relayed = SocketAddrV4::new(self.local_ip, self.next_port);
                    self.next_port += 1;
                    self.allocs.insert(src, Allocation { relayed, expires: now + Duration::from_secs(life as u64), perms: HashMap::new(), channels: HashMap::new() });
                    relayed
                };
                self.counters.allocations = self.allocs.len();
                Some(ok(Builder::new(m.method, Class::Success, m.txid)
                    .xor_addr(attr::XOR_RELAYED_ADDRESS, SocketAddr::V4(relayed))
                    .xor_addr(attr::XOR_MAPPED_ADDRESS, src)
                    .u32(attr::LIFETIME, life)))
            }
            method::REFRESH => {
                let life = lifetime(m).min(MAX_LIFETIME);
                if life == 0 {
                    self.allocs.remove(&src);
                    self.counters.allocations = self.allocs.len();
                    return Some(ok(Builder::new(m.method, Class::Success, m.txid).u32(attr::LIFETIME, 0)));
                }
                let Some(a) = self.allocs.get_mut(&src).filter(|a| a.expires > now) else {
                    return Some(ok(err(437, "Allocation Mismatch")));
                };
                let life = life.max(DEFAULT_LIFETIME);
                a.expires = now + Duration::from_secs(life as u64);
                Some(ok(Builder::new(m.method, Class::Success, m.txid).u32(attr::LIFETIME, life)))
            }
            method::CREATE_PERMISSION => {
                let peer_ip = self.peer_ip;
                let Some(a) = self.allocs.get_mut(&src).filter(|a| a.expires > now) else {
                    return Some(ok(err(437, "Allocation Mismatch")));
                };
                let peers: Vec<SocketAddr> = m.all(attr::XOR_PEER_ADDRESS).filter_map(|v| stun::decode_xor_addr(v, &m.txid)).collect();
                if peers.is_empty() { return Some(ok(err(400, "Bad Request"))); }
                // Only the other side of THIS call is reachable through us.
                if peers.iter().any(|p| p.ip() != IpAddr::V4(peer_ip)) {
                    return Some(ok(err(403, "Forbidden")));
                }
                a.perms.insert(peer_ip, now + PERMISSION_LIFETIME);
                Some(ok(Builder::new(m.method, Class::Success, m.txid)))
            }
            method::CHANNEL_BIND => {
                let peer_ip = self.peer_ip;
                let Some(a) = self.allocs.get_mut(&src).filter(|a| a.expires > now) else {
                    return Some(ok(err(437, "Allocation Mismatch")));
                };
                let ch = m.get(attr::CHANNEL_NUMBER).filter(|v| v.len() >= 2).map(|v| u16::from_be_bytes([v[0], v[1]]));
                let peer = m.xor_addr(attr::XOR_PEER_ADDRESS).and_then(as_v4);
                let (Some(ch), Some(peer)) = (ch, peer) else { return Some(ok(err(400, "Bad Request"))) };
                if !(0x4000..=0x7FFF).contains(&ch) { return Some(ok(err(400, "Bad Request"))); }
                if peer.ip() != &peer_ip { return Some(ok(err(403, "Forbidden"))); }
                // A channel is bound to one peer, and a peer to one channel.
                if a.channels.get(&ch).is_some_and(|(p, _)| *p != peer)
                    || a.channels.iter().any(|(c, (p, _))| *p == peer && *c != ch) {
                    return Some(ok(err(400, "Bad Request")));
                }
                a.channels.insert(ch, (peer, now + CHANNEL_LIFETIME));
                a.perms.insert(peer_ip, now + PERMISSION_LIFETIME);
                Some(ok(Builder::new(m.method, Class::Success, m.txid)))
            }
            _ => Some(ok(err(400, "Bad Request"))),
        }
    }

    /// A packet from the other side, addressed to one of our relayed addresses.
    /// Returns the client to hand it to and the bytes to send.
    pub fn on_remote(&mut self, from: SocketAddrV4, to: SocketAddrV4, data: &[u8], now: Instant) -> Option<(SocketAddr, Vec<u8>)> {
        if from.ip() != &self.peer_ip { self.counters.dropped += 1; return None; }
        let (client, a) = self.allocs.iter().find(|(_, a)| a.relayed == to && a.expires > now)?;
        // RFC 8656 drops data from peers without a permission. Here the only
        // possible peer is the authenticated other side, and the client may not
        // have installed its permission yet when the first connectivity check
        // arrives — delivering it lets ICE learn the pair a round trip sooner.
        let out = match a.channel_for(from, now) {
            Some(ch) => stun::build_channel_data(ch, data),
            None => Builder::new(method::DATA, Class::Indication, rand::random())
                .xor_addr(attr::XOR_PEER_ADDRESS, SocketAddr::V4(from))
                .add(attr::DATA, data).build(),
        };
        Some((*client, out))
    }

    /// Drop expired allocations, permissions and channels.
    pub fn expire(&mut self, now: Instant) {
        self.allocs.retain(|_, a| a.expires > now);
        for a in self.allocs.values_mut() {
            a.perms.retain(|_, e| *e > now);
            a.channels.retain(|_, (_, e)| *e > now);
        }
        self.counters.allocations = self.allocs.len();
    }
}

fn as_v4(a: SocketAddr) -> Option<SocketAddrV4> {
    match a { SocketAddr::V4(v) => Some(v), SocketAddr::V6(_) => None }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const USER: &str = "u1";
    const PASS: &str = "p1";

    /// A tiny TURN client, enough to drive the server like a WebRTC engine does.
    pub(crate) struct Client { pub key: [u8; 16], pub nonce: Vec<u8>, pub src: SocketAddr }
    impl Client {
        pub fn new(src: &str) -> Self { Client { key: stun::long_term_key(USER, REALM, PASS), nonce: vec![], src: src.parse().unwrap() } }
        pub fn req(&self, m: u16, f: impl FnOnce(Builder) -> Builder) -> Vec<u8> {
            let b = Builder::new(m, Class::Request, rand::random());
            f(b).add(attr::USERNAME, USER.as_bytes()).add(attr::REALM, REALM.as_bytes()).add(attr::NONCE, &self.nonce).integrity(&self.key).build()
        }
        /// Allocate with the 401 dance; returns the relayed address.
        pub fn allocate(&mut self, t: &mut Turn, now: Instant) -> SocketAddrV4 {
            let first = Builder::new(method::ALLOCATE, Class::Request, [1; 12]).add(attr::REQUESTED_TRANSPORT, &[17, 0, 0, 0]).build();
            let Some(Out::Reply(_, r)) = t.on_client(self.src, &first, now) else { panic!("no reply") };
            let r = stun::parse(&r).unwrap();
            assert_eq!(r.class, Class::Error);
            assert_eq!(&r.get(attr::ERROR_CODE).unwrap()[2..4], &[4, 1]);
            self.nonce = r.get(attr::NONCE).unwrap().to_vec();
            let p = self.req(method::ALLOCATE, |b| b.add(attr::REQUESTED_TRANSPORT, &[17, 0, 0, 0]));
            let Some(Out::Reply(_, r)) = t.on_client(self.src, &p, now) else { panic!("no reply") };
            let r = stun::parse(&r).unwrap();
            assert_eq!(r.class, Class::Success, "{r:?}");
            assert!(r.integrity_ok(&self.key));
            assert_eq!(r.xor_addr(attr::XOR_MAPPED_ADDRESS), Some(self.src));
            as_v4(r.xor_addr(attr::XOR_RELAYED_ADDRESS).unwrap()).unwrap()
        }
    }

    fn class_of(out: Option<Out>) -> (Class, Option<u16>) {
        let Some(Out::Reply(_, r)) = out else { panic!("expected a reply") };
        let m = stun::parse(&r).unwrap();
        let code = m.get(attr::ERROR_CODE).map(|e| e[2] as u16 * 100 + e[3] as u16);
        (m.class, code)
    }

    #[test]
    fn allocate_permission_send_and_receive() {
        let now = Instant::now();
        let mut caller = Turn::new(USER, PASS, true);
        let mut callee = Turn::new(USER, PASS, false);
        let mut a = Client::new("192.168.1.5:5000");
        let mut b = Client::new("127.0.0.1:6000");
        let ra = a.allocate(&mut caller, now);
        let rb = b.allocate(&mut callee, now);
        assert_eq!(ra, "10.77.0.1:40000".parse().unwrap());
        assert_eq!(rb, "10.77.0.2:40000".parse().unwrap());

        // Send indication before a permission is dropped.
        let send = Builder::new(method::SEND, Class::Indication, [3; 12])
            .xor_addr(attr::XOR_PEER_ADDRESS, SocketAddr::V4(rb)).add(attr::DATA, b"hello").build();
        assert_eq!(caller.on_client(a.src, &send, now), None);
        let perm = a.req(method::CREATE_PERMISSION, |x| x.xor_addr(attr::XOR_PEER_ADDRESS, SocketAddr::V4(rb)));
        assert_eq!(class_of(caller.on_client(a.src, &perm, now)).0, Class::Success);
        let out = caller.on_client(a.src, &send, now);
        assert_eq!(out, Some(Out::Relay { from: ra, to: rb, data: b"hello".to_vec() }));

        // The other side delivers it as a Data indication from the caller's address.
        let (dst, pkt) = callee.on_remote(ra, rb, b"hello", now).unwrap();
        assert_eq!(dst, b.src);
        let m = stun::parse(&pkt).unwrap();
        assert_eq!((m.method, m.class), (method::DATA, Class::Indication));
        assert_eq!(m.xor_addr(attr::XOR_PEER_ADDRESS), Some(SocketAddr::V4(ra)));
        assert_eq!(m.get(attr::DATA), Some(&b"hello"[..]));

        // After a ChannelBind, both directions use ChannelData.
        let bind = b.req(method::CHANNEL_BIND, |x| x.add(attr::CHANNEL_NUMBER, &[0x40, 0x01, 0, 0]).xor_addr(attr::XOR_PEER_ADDRESS, SocketAddr::V4(ra)));
        assert_eq!(class_of(callee.on_client(b.src, &bind, now)).0, Class::Success);
        let (_, pkt) = callee.on_remote(ra, rb, b"x", now).unwrap();
        assert_eq!(pkt, vec![0x40, 0x01, 0, 1, b'x']);
        let out = callee.on_client(b.src, &stun::build_channel_data(0x4001, b"back"), now);
        assert_eq!(out, Some(Out::Relay { from: rb, to: ra, data: b"back".to_vec() }));
        // An unbound channel is dropped.
        assert_eq!(callee.on_client(b.src, &stun::build_channel_data(0x4002, b"x"), now), None);
    }

    #[test]
    fn wrong_credentials_are_refused() {
        let now = Instant::now();
        let mut t = Turn::new(USER, PASS, true);
        let mut c = Client::new("127.0.0.1:1");
        c.allocate(&mut Turn::new(USER, PASS, true), now); // learn some nonce
        c.key = stun::long_term_key(USER, REALM, "guess");
        let p = c.req(method::ALLOCATE, |b| b.add(attr::REQUESTED_TRANSPORT, &[17, 0, 0, 0]));
        assert_eq!(class_of(t.on_client(c.src, &p, now)), (Class::Error, Some(401)));
        assert_eq!(t.counters.auth_failures, 1);
        assert_eq!(t.counters.allocations, 0);
        // Right password, wrong nonce → 438 so the client retries with the new one.
        let mut c = Client::new("127.0.0.1:1");
        c.nonce = b"old".to_vec();
        let p = c.req(method::ALLOCATE, |b| b.add(attr::REQUESTED_TRANSPORT, &[17, 0, 0, 0]));
        assert_eq!(class_of(t.on_client(c.src, &p, now)), (Class::Error, Some(438)));
        // Data from a client with no allocation goes nowhere.
        assert_eq!(t.on_client(c.src, &stun::build_channel_data(0x4000, b"x"), now), None);
    }

    #[test]
    fn only_the_other_side_is_reachable() {
        let now = Instant::now();
        let mut t = Turn::new(USER, PASS, true);
        let mut c = Client::new("127.0.0.1:1");
        let r = c.allocate(&mut t, now);
        for peer in ["8.8.8.8:53", "127.0.0.1:22", "10.77.0.1:40000"] {
            let p = c.req(method::CREATE_PERMISSION, |x| x.xor_addr(attr::XOR_PEER_ADDRESS, peer.parse().unwrap()));
            assert_eq!(class_of(t.on_client(c.src, &p, now)), (Class::Error, Some(403)), "{peer}");
            let p = c.req(method::CHANNEL_BIND, |x| x.add(attr::CHANNEL_NUMBER, &[0x40, 0, 0, 0]).xor_addr(attr::XOR_PEER_ADDRESS, peer.parse().unwrap()));
            assert_eq!(class_of(t.on_client(c.src, &p, now)), (Class::Error, Some(403)), "{peer}");
        }
        // Packets claiming to come from anywhere but the other side are dropped.
        assert_eq!(t.on_remote("10.77.0.9:40000".parse().unwrap(), r, b"x", now), None);
        // TCP and IPv6 allocations are refused.
        let mut c2 = Client::new("127.0.0.1:2");
        c2.nonce = c.nonce.clone();
        let p = c2.req(method::ALLOCATE, |b| b.add(attr::REQUESTED_TRANSPORT, &[6, 0, 0, 0]));
        assert_eq!(class_of(t.on_client(c2.src, &p, now)), (Class::Error, Some(442)));
        let p = c2.req(method::ALLOCATE, |b| b.add(attr::REQUESTED_TRANSPORT, &[17, 0, 0, 0]).add(attr::REQUESTED_ADDRESS_FAMILY, &[2, 0, 0, 0]));
        assert_eq!(class_of(t.on_client(c2.src, &p, now)), (Class::Error, Some(440)));
    }

    #[test]
    fn refresh_expiry_and_quota() {
        let now = Instant::now();
        let mut t = Turn::new(USER, PASS, false);
        let mut c = Client::new("127.0.0.1:1");
        let r = c.allocate(&mut t, now);
        // A retransmitted allocate keeps the same address.
        let p = c.req(method::ALLOCATE, |b| b.add(attr::REQUESTED_TRANSPORT, &[17, 0, 0, 0]));
        let Some(Out::Reply(_, x)) = t.on_client(c.src, &p, now) else { panic!() };
        assert_eq!(stun::parse(&x).unwrap().xor_addr(attr::XOR_RELAYED_ADDRESS), Some(SocketAddr::V4(r)));
        let later = now + Duration::from_secs(601);
        let refresh = c.req(method::REFRESH, |b| b.u32(attr::LIFETIME, 1200));
        assert_eq!(class_of(t.on_client(c.src, &refresh, now)).0, Class::Success);
        assert!(t.on_remote(SocketAddrV4::new(virtual_ip(true), 40000), r, b"x", later).is_some());
        let gone = now + Duration::from_secs(3 * 3600);
        t.expire(gone);
        assert_eq!(t.counters.allocations, 0);
        assert_eq!(class_of(t.on_client(c.src, &refresh, gone)), (Class::Error, Some(437)));
        // Lifetime 0 deletes.
        let r2 = c.allocate(&mut t, now);
        let del = c.req(method::REFRESH, |b| b.u32(attr::LIFETIME, 0));
        assert_eq!(class_of(t.on_client(c.src, &del, now)).0, Class::Success);
        assert!(t.on_remote(SocketAddrV4::new(virtual_ip(true), 40000), r2, b"x", now).is_none());
        // Quota.
        for i in 0..MAX_ALLOCATIONS { Client::new(&format!("127.0.0.1:{}", 100 + i)).allocate(&mut t, now); }
        let mut extra = Client::new("127.0.0.1:999");
        extra.nonce = c.nonce.clone();
        let p = extra.req(method::ALLOCATE, |b| b.add(attr::REQUESTED_TRANSPORT, &[17, 0, 0, 0]));
        assert_eq!(class_of(t.on_client(extra.src, &p, now)), (Class::Error, Some(486)));
    }

    #[test]
    fn binding_requests_are_answered_without_auth() {
        let mut t = Turn::new(USER, PASS, true);
        let src: SocketAddr = "127.0.0.1:7".parse().unwrap();
        let p = Builder::new(method::BINDING, Class::Request, [9; 12]).build();
        let Some(Out::Reply(to, r)) = t.on_client(src, &p, Instant::now()) else { panic!() };
        assert_eq!(to, src);
        assert_eq!(stun::parse(&r).unwrap().xor_addr(attr::XOR_MAPPED_ADDRESS), Some(src));
    }
}
