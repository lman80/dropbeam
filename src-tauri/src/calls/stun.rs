//! STUN / TURN wire format (RFC 8489, RFC 8656) — only what a WebRTC engine's
//! TURN client needs from the call bridge's 127.0.0.1 TURN server. Parsing is
//! bounds-checked everywhere: the only sender is a process on this machine,
//! but nothing here trusts it.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

pub const MAGIC: u32 = 0x2112_A442;
const MAGIC_BYTES: [u8; 4] = MAGIC.to_be_bytes();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class { Request, Indication, Success, Error }

pub mod method {
    pub const BINDING: u16 = 0x001;
    pub const ALLOCATE: u16 = 0x003;
    pub const REFRESH: u16 = 0x004;
    pub const SEND: u16 = 0x006;
    pub const DATA: u16 = 0x007;
    pub const CREATE_PERMISSION: u16 = 0x008;
    pub const CHANNEL_BIND: u16 = 0x009;
}

pub mod attr {
    pub const USERNAME: u16 = 0x0006;
    pub const MESSAGE_INTEGRITY: u16 = 0x0008;
    pub const ERROR_CODE: u16 = 0x0009;
    pub const CHANNEL_NUMBER: u16 = 0x000C;
    pub const LIFETIME: u16 = 0x000D;
    pub const XOR_PEER_ADDRESS: u16 = 0x0012;
    pub const DATA: u16 = 0x0013;
    pub const REALM: u16 = 0x0014;
    pub const NONCE: u16 = 0x0015;
    pub const XOR_RELAYED_ADDRESS: u16 = 0x0016;
    pub const REQUESTED_ADDRESS_FAMILY: u16 = 0x0017;
    pub const REQUESTED_TRANSPORT: u16 = 0x0019;
    pub const XOR_MAPPED_ADDRESS: u16 = 0x0020;
    pub const SOFTWARE: u16 = 0x8022;
    pub const FINGERPRINT: u16 = 0x8028;
}

/// Method + class → the 14-bit STUN message type (RFC 8489 §5).
pub fn msg_type(method: u16, class: Class) -> u16 {
    let c: u16 = match class { Class::Request => 0, Class::Indication => 1, Class::Success => 2, Class::Error => 3 };
    (method & 0x000F) | ((method & 0x0070) << 1) | ((method & 0x0F80) << 2) | ((c & 1) << 4) | ((c & 2) << 7)
}

pub fn split_type(t: u16) -> (u16, Class) {
    let method = (t & 0x000F) | ((t >> 1) & 0x0070) | ((t >> 2) & 0x0F80);
    let class = match ((t >> 4) & 1) | ((t >> 7) & 2) { 0 => Class::Request, 1 => Class::Indication, 2 => Class::Success, _ => Class::Error };
    (method, class)
}

/// A parsed STUN message borrowing its packet.
#[derive(Debug)]
pub struct Msg<'a> {
    pub method: u16,
    pub class: Class,
    pub txid: [u8; 12],
    /// (type, value, offset of the attribute header in `raw`)
    pub attrs: Vec<(u16, &'a [u8], usize)>,
    pub raw: &'a [u8],
}

/// A STUN message starts with two zero bits and carries the magic cookie.
pub fn is_stun(b: &[u8]) -> bool {
    b.len() >= 20 && b[0] & 0xC0 == 0 && b[4..8] == MAGIC_BYTES
}

pub fn parse(b: &[u8]) -> Option<Msg<'_>> {
    if !is_stun(b) { return None; }
    let len = u16::from_be_bytes([b[2], b[3]]) as usize;
    if len % 4 != 0 || 20 + len > b.len() { return None; }
    let raw = &b[..20 + len];
    let (method, class) = split_type(u16::from_be_bytes([b[0], b[1]]));
    let mut txid = [0u8; 12];
    txid.copy_from_slice(&b[8..20]);
    let mut attrs = Vec::new();
    let mut i = 20;
    while i + 4 <= raw.len() {
        let t = u16::from_be_bytes([raw[i], raw[i + 1]]);
        let l = u16::from_be_bytes([raw[i + 2], raw[i + 3]]) as usize;
        let end = i + 4 + l;
        if end > raw.len() || attrs.len() >= 64 { return None; }
        attrs.push((t, &raw[i + 4..end], i));
        i = end + (4 - l % 4) % 4;
    }
    Some(Msg { method, class, txid, attrs, raw })
}

impl<'a> Msg<'a> {
    pub fn get(&self, t: u16) -> Option<&'a [u8]> {
        self.attrs.iter().find(|a| a.0 == t).map(|a| a.1)
    }
    pub fn all(&self, t: u16) -> impl Iterator<Item = &'a [u8]> + '_ {
        self.attrs.iter().filter(move |a| a.0 == t).map(|a| a.1)
    }
    pub fn str(&self, t: u16) -> Option<&'a str> {
        self.get(t).and_then(|v| std::str::from_utf8(v).ok())
    }
    pub fn xor_addr(&self, t: u16) -> Option<SocketAddr> {
        decode_xor_addr(self.get(t)?, &self.txid)
    }
    /// MESSAGE-INTEGRITY (HMAC-SHA1) checks out with `key` (RFC 8489 §14.5):
    /// computed over everything before the attribute, with the header length
    /// covering up to and including it.
    pub fn integrity_ok(&self, key: &[u8]) -> bool {
        let Some(&(_, value, off)) = self.attrs.iter().find(|a| a.0 == attr::MESSAGE_INTEGRITY) else { return false };
        if value.len() != 20 { return false; }
        let mut head = self.raw[..off].to_vec();
        let len = (off + 24 - 20) as u16;
        head[2..4].copy_from_slice(&len.to_be_bytes());
        let k = ring::hmac::Key::new(ring::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, key);
        ring::hmac::verify(&k, &head, value).is_ok()
    }
}

pub fn decode_xor_addr(v: &[u8], txid: &[u8; 12]) -> Option<SocketAddr> {
    if v.len() < 8 { return None; }
    let port = u16::from_be_bytes([v[2], v[3]]) ^ (MAGIC >> 16) as u16;
    match v[1] {
        0x01 => {
            let x = u32::from_be_bytes([v[4], v[5], v[6], v[7]]) ^ MAGIC;
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(x)), port))
        }
        0x02 if v.len() >= 20 => {
            let mut mask = [0u8; 16];
            mask[..4].copy_from_slice(&MAGIC_BYTES);
            mask[4..].copy_from_slice(txid);
            let mut ip = [0u8; 16];
            for i in 0..16 { ip[i] = v[4 + i] ^ mask[i]; }
            Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(ip)), port))
        }
        _ => None,
    }
}

pub fn encode_xor_addr(a: SocketAddr, txid: &[u8; 12]) -> Vec<u8> {
    let port = a.port() ^ (MAGIC >> 16) as u16;
    match a.ip() {
        IpAddr::V4(ip) => {
            let mut v = vec![0, 0x01];
            v.extend_from_slice(&port.to_be_bytes());
            v.extend_from_slice(&(u32::from(ip) ^ MAGIC).to_be_bytes());
            v
        }
        IpAddr::V6(ip) => {
            let mut v = vec![0, 0x02];
            v.extend_from_slice(&port.to_be_bytes());
            let mut mask = [0u8; 16];
            mask[..4].copy_from_slice(&MAGIC_BYTES);
            mask[4..].copy_from_slice(txid);
            for (i, b) in ip.octets().iter().enumerate() { v.push(b ^ mask[i]); }
            v
        }
    }
}

/// Builds one STUN message; integrity and fingerprint go last, in that order.
pub struct Builder { buf: Vec<u8>, txid: [u8; 12] }

impl Builder {
    pub fn new(method: u16, class: Class, txid: [u8; 12]) -> Self {
        let mut buf = Vec::with_capacity(128);
        buf.extend_from_slice(&msg_type(method, class).to_be_bytes());
        buf.extend_from_slice(&[0, 0]);
        buf.extend_from_slice(&MAGIC_BYTES);
        buf.extend_from_slice(&txid);
        Builder { buf, txid }
    }
    fn set_len(&mut self, extra: usize) {
        let len = (self.buf.len() - 20 + extra) as u16;
        self.buf[2..4].copy_from_slice(&len.to_be_bytes());
    }
    pub fn add(mut self, t: u16, v: &[u8]) -> Self {
        self.buf.extend_from_slice(&t.to_be_bytes());
        self.buf.extend_from_slice(&(v.len() as u16).to_be_bytes());
        self.buf.extend_from_slice(v);
        self.buf.resize(self.buf.len() + (4 - v.len() % 4) % 4, 0);
        self.set_len(0);
        self
    }
    pub fn xor_addr(self, t: u16, a: SocketAddr) -> Self {
        let v = encode_xor_addr(a, &self.txid);
        self.add(t, &v)
    }
    pub fn u32(self, t: u16, v: u32) -> Self { self.add(t, &v.to_be_bytes()) }
    pub fn error(self, code: u16, reason: &str) -> Self {
        let mut v = vec![0, 0, (code / 100) as u8, (code % 100) as u8];
        v.extend_from_slice(reason.as_bytes());
        self.add(attr::ERROR_CODE, &v)
    }
    pub fn integrity(mut self, key: &[u8]) -> Self {
        self.set_len(24);
        let k = ring::hmac::Key::new(ring::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, key);
        let tag = ring::hmac::sign(&k, &self.buf);
        let mac = tag.as_ref().to_vec();
        self.add(attr::MESSAGE_INTEGRITY, &mac)
    }
    pub fn fingerprint(mut self) -> Self {
        self.set_len(8);
        let crc = crc32fast::hash(&self.buf) ^ 0x5354_554E;
        self.add(attr::FINGERPRINT, &crc.to_be_bytes())
    }
    pub fn build(self) -> Vec<u8> { self.buf }
}

/// TURN ChannelData (RFC 8656 §12.4): first two bits 01.
pub fn channel_data(b: &[u8]) -> Option<(u16, &[u8])> {
    if b.len() < 4 || b[0] & 0xC0 != 0x40 { return None; }
    let ch = u16::from_be_bytes([b[0], b[1]]);
    let len = u16::from_be_bytes([b[2], b[3]]) as usize;
    if 4 + len > b.len() || !(0x4000..=0x7FFF).contains(&ch) { return None; }
    Some((ch, &b[4..4 + len]))
}

pub fn build_channel_data(ch: u16, data: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(4 + data.len());
    v.extend_from_slice(&ch.to_be_bytes());
    v.extend_from_slice(&(data.len() as u16).to_be_bytes());
    v.extend_from_slice(data);
    v
}

/// The TURN long-term credential key: MD5(username ":" realm ":" password).
pub fn long_term_key(user: &str, realm: &str, pass: &str) -> [u8; 16] {
    md5(format!("{user}:{realm}:{pass}").as_bytes())
}

/// MD5 (RFC 1321). Only for the TURN long-term key the WebRTC client derives
/// the same way — never for anything that needs a secure hash.
pub fn md5(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
        5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
        6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32).collect();
    let (mut a0, mut b0, mut c0, mut d0) = (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    let mut msg = input.to_vec();
    let bits = (input.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 { msg.push(0); }
    msg.extend_from_slice(&bits.to_le_bytes());
    for chunk in msg.chunks(64) {
        let m: Vec<u32> = chunk.chunks(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d; d = c; c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a); b0 = b0.wrapping_add(b); c0 = c0.wrapping_add(c); d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..].copy_from_slice(&d0.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_vectors() {
        assert_eq!(hex::encode(md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex::encode(md5(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(hex::encode(md5(b"12345678901234567890123456789012345678901234567890123456789012345678901234567890")),
            "57edf4a22be3c955ac49da2e2107b67a");
    }

    #[test]
    fn type_round_trips() {
        for m in [method::BINDING, method::ALLOCATE, method::REFRESH, method::SEND, method::DATA, method::CREATE_PERMISSION, method::CHANNEL_BIND] {
            for c in [Class::Request, Class::Indication, Class::Success, Class::Error] {
                assert_eq!(split_type(msg_type(m, c)), (m, c));
            }
        }
        assert_eq!(msg_type(method::BINDING, Class::Request), 0x0001);
        assert_eq!(msg_type(method::BINDING, Class::Success), 0x0101);
        assert_eq!(msg_type(method::ALLOCATE, Class::Error), 0x0113);
        assert_eq!(msg_type(method::DATA, Class::Indication), 0x0017);
    }

    /// RFC 5769 §2.1 sample request: integrity (short-term key = password) and
    /// fingerprint verify, and our builder reproduces both bytes-for-bytes.
    #[test]
    fn rfc5769_sample_request() {
        let pkt = hex::decode(concat!(
            "000100582112a442b7e7a701bc34d686fa87dfae",
            "802200105354554e207465737420636c69656e74",
            "002400046e0001ff",
            "80290008932ff9b151263b36",
            "000600096576746a3a68367659202020",
            "000800149aeaa70cbfd8cb56781ef2b5b2d3f249c1b571a2",
            "80280004e57a3bcf")).unwrap();
        let m = parse(&pkt).expect("parses");
        assert_eq!(m.method, method::BINDING);
        assert_eq!(m.class, Class::Request);
        assert_eq!(m.str(attr::USERNAME), Some("evtj:h6vY"));
        assert!(m.integrity_ok(b"VOkJxbRl1RmTxUk/WvJxBt"));
        assert!(!m.integrity_ok(b"wrong"));
        let crc = crc32fast::hash(&{ let mut h = pkt[..pkt.len() - 8].to_vec(); h[2..4].copy_from_slice(&((pkt.len() - 20) as u16).to_be_bytes()); h }) ^ 0x5354_554E;
        assert_eq!(&crc.to_be_bytes(), m.get(attr::FINGERPRINT).unwrap());
    }

    /// RFC 5769 §2.2 sample IPv4 response: XOR-MAPPED-ADDRESS decodes.
    #[test]
    fn rfc5769_sample_response_v4() {
        let pkt = hex::decode(concat!(
            "0101003c2112a442b7e7a701bc34d686fa87dfae",
            "8022000b7465737420766563746f7220",
            "002000080001a147e112a643",
            "000800142b91f599fd9e90c38c7489f92af9ba53f06be7d7",
            "80280004c07d4c96")).unwrap();
        let m = parse(&pkt).unwrap();
        assert_eq!(m.xor_addr(attr::XOR_MAPPED_ADDRESS), Some("192.0.2.1:32853".parse().unwrap()));
        assert!(m.integrity_ok(b"VOkJxbRl1RmTxUk/WvJxBt"));
    }

    #[test]
    fn builder_integrity_and_xor_v6_round_trip() {
        let txid = [7u8; 12];
        let a: SocketAddr = "[2001:db8::1]:4242".parse().unwrap();
        let pkt = Builder::new(method::ALLOCATE, Class::Success, txid)
            .xor_addr(attr::XOR_MAPPED_ADDRESS, a).u32(attr::LIFETIME, 600)
            .integrity(b"key").fingerprint().build();
        let m = parse(&pkt).unwrap();
        assert_eq!(m.xor_addr(attr::XOR_MAPPED_ADDRESS), Some(a));
        assert!(m.integrity_ok(b"key"));
        assert_eq!(m.get(attr::LIFETIME), Some(&600u32.to_be_bytes()[..]));
    }

    #[test]
    fn rejects_malformed() {
        assert!(parse(&[0u8; 19]).is_none());
        let mut p = Builder::new(method::BINDING, Class::Request, [1; 12]).add(attr::USERNAME, b"abc").build();
        p[3] += 4; // length past the end
        assert!(parse(&p).is_none());
        let mut p = Builder::new(method::BINDING, Class::Request, [1; 12]).build();
        p[4] = 0; // no cookie
        assert!(parse(&p).is_none());
        // An attribute claiming more bytes than the message holds.
        let mut p = Builder::new(method::BINDING, Class::Request, [1; 12]).add(attr::USERNAME, b"abcd").build();
        p[22..24].copy_from_slice(&200u16.to_be_bytes());
        assert!(parse(&p).is_none());
        assert!(channel_data(&[0x40, 0x00, 0x00, 0x09, 1, 2]).is_none());
        assert_eq!(channel_data(&[0x40, 0x01, 0x00, 0x02, 9, 8]), Some((0x4001, &[9u8, 8][..])));
    }
}
