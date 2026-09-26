//! End-to-end envelope for Transfer Server items (see docs/TRANSFER-SERVER-PLAN.md §5.5).
//!
//! The server stores only what this module produces: a signed header naming the
//! sender and the recipient DEVICES, one key "stanza" per device, the encrypted
//! metadata (chat frame / file manifest), and a segmented ciphertext payload. It
//! never holds a key that opens any of it.
//!
//! ```text
//! file_key   = 32 random bytes (one per item)
//! stanza(R)  = eph X25519 keypair; ss = X25519(eph, R.mailbox_pub)
//!              wrap = HKDF-SHA256(ss, salt = eph_pub || R_pub, "dropbeam-mbx-wrap-v1")
//!              ct   = ChaCha20-Poly1305(wrap, nonce 0, aad = item_id || R.eid, file_key)
//! meta_ct    = ChaCha20-Poly1305(HKDF(file_key, item_id, "...meta-v1"), nonce 0, aad = item_id, meta)
//! segment i  = ChaCha20-Poly1305(HKDF(file_key, item_id, "...payload-v1"),
//!              nonce = 11-byte BE i || last_flag, aad = item_id, plaintext[i*SEG..])
//! sig        = ed25519(sender endpoint key, domain || length-prefixed header fields)
//! ```
//!
//! The per-segment counter + last flag stop reordering and truncation; the
//! signature binds every header field (who, to whom, what kind, how big) to the
//! sender's pinned identity, so a server can drop or delay an item but never
//! forge, alter or read it.

use anyhow::{bail, ensure, Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use curve25519_dalek::montgomery::MontgomeryPoint;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305};
use ring::hkdf;
use serde::{Deserialize, Serialize};

/// Plaintext bytes per payload segment.
pub const SEG: u64 = 1 << 20;
/// AEAD tag appended to every sealed segment.
pub const TAG: u64 = 16;
/// Ciphertext bytes of one FULL segment (every segment but the last).
pub const CT_SEG: u64 = SEG + TAG;
const SIG_DOMAIN: &[u8] = b"dropbeam-mbx-env-v1";
const KEY_DOMAIN: &[u8] = b"dropbeam-mailbox-key-v1";
const PUSH_INFO: &[u8] = b"dropbeam-push-v1";

pub fn b64(bytes: &[u8]) -> String {
    B64.encode(bytes)
}
pub fn unb64(s: &str) -> Result<Vec<u8>> {
    B64.decode(s.as_bytes()).context("bad base64")
}
pub fn key32(s: &str) -> Result<[u8; 32]> {
    let v = unb64(s)?;
    <[u8; 32]>::try_from(v.as_slice()).ok().context("expected a 32-byte key")
}

/// X25519 public key for a (clamped) secret scalar.
pub fn x25519_public(secret: &[u8; 32]) -> [u8; 32] {
    MontgomeryPoint::mul_base_clamped(*secret).to_bytes()
}

/// X25519 shared secret. `None` for a low-order peer key (all-zero output) —
/// a malicious key must never yield a predictable wrap key.
pub fn x25519(secret: &[u8; 32], public: &[u8; 32]) -> Option<[u8; 32]> {
    let ss = MontgomeryPoint(*public).mul_clamped(*secret).to_bytes();
    (ss != [0u8; 32]).then_some(ss)
}

struct Len32;
impl hkdf::KeyType for Len32 {
    fn len(&self) -> usize {
        32
    }
}

fn hkdf32(ikm: &[u8], salt: &[u8], info: &[u8]) -> [u8; 32] {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(ikm);
    let mut out = [0u8; 32];
    prk.expand(&[info], Len32).expect("32 bytes is a valid HKDF-SHA256 length").fill(&mut out)
        .expect("fill matches the requested length");
    out
}

pub fn hkdf32_pub(ikm: &[u8], salt: &[u8], info: &[u8]) -> [u8; 32] {
    hkdf32(ikm, salt, info)
}

fn aead_key(k: &[u8; 32]) -> LessSafeKey {
    LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, k).expect("32-byte ChaCha20 key"))
}

fn seal_with(k: &[u8; 32], nonce: [u8; 12], aad: &[u8], pt: &[u8]) -> Vec<u8> {
    let mut buf = pt.to_vec();
    aead_key(k)
        .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(aad), &mut buf)
        .expect("ChaCha20-Poly1305 seal cannot fail for in-range input");
    buf
}

fn open_with(k: &[u8; 32], nonce: [u8; 12], aad: &[u8], ct: &[u8]) -> Result<Vec<u8>> {
    let mut buf = ct.to_vec();
    let n = aead_key(k)
        .open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(aad), &mut buf)
        .map_err(|_| anyhow::anyhow!("decryption failed"))?
        .len();
    buf.truncate(n);
    Ok(buf)
}

/// Signed statement that `x25519_pub` is this endpoint's mailbox key.
pub fn sign_mailbox_key(signer: &iroh::SecretKey, x25519_pub: &[u8; 32]) -> String {
    let mut msg = KEY_DOMAIN.to_vec();
    msg.extend_from_slice(x25519_pub);
    b64(&signer.sign(&msg).to_bytes())
}

/// True when `sig` proves endpoint `eid` published `x25519_pub`.
pub fn verify_mailbox_key(eid: &str, x25519_pub: &[u8; 32], sig: &str) -> bool {
    let Ok(pk) = eid.parse::<iroh::PublicKey>() else { return false };
    let Ok(sig) = unb64(sig) else { return false };
    let Ok(sig) = <[u8; 64]>::try_from(sig.as_slice()) else { return false };
    let mut msg = KEY_DOMAIN.to_vec();
    msg.extend_from_slice(x25519_pub);
    pk.verify(&msg, &iroh::Signature::from_bytes(&sig)).is_ok()
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Stanza {
    pub eid: String,
    pub epk: String,
    pub ct: String,
}

/// The sealed item header the server stores and relays verbatim.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub v: u32,
    pub item_id: String,
    pub from: String,
    /// "chat" | "op" | "file".
    pub kind: String,
    pub created_ms: u64,
    pub stanzas: Vec<Stanza>,
    pub meta_ct: String,
    /// Plaintext payload bytes (0 for chat/op).
    pub size: u64,
    pub seg: u64,
    pub segs: u64,
    pub sig: String,
}

pub fn has_payload(kind: &str) -> bool {
    kind == "file"
}

/// Segment count for a payload of `size` bytes. A file item always has at least
/// one (possibly empty) segment so the last-flag authenticates the end.
pub fn segs_for(kind: &str, size: u64) -> u64 {
    if !has_payload(kind) {
        0
    } else {
        size.div_ceil(SEG).max(1)
    }
}

impl Envelope {
    pub fn to(&self) -> Vec<String> {
        self.stanzas.iter().map(|s| s.eid.clone()).collect()
    }
    /// Bytes of ciphertext payload that follow the header on the wire / disk.
    pub fn ct_size(&self) -> u64 {
        self.size + self.segs * TAG
    }
    fn signed_bytes(&self) -> Vec<u8> {
        let mut out = SIG_DOMAIN.to_vec();
        let mut put = |b: &[u8]| {
            out.extend_from_slice(&(b.len() as u64).to_be_bytes());
            out.extend_from_slice(b);
        };
        put(&self.v.to_be_bytes());
        put(self.item_id.as_bytes());
        put(self.from.as_bytes());
        put(self.kind.as_bytes());
        put(&self.created_ms.to_be_bytes());
        put(&(self.stanzas.len() as u64).to_be_bytes());
        for s in &self.stanzas {
            put(s.eid.as_bytes());
            put(s.epk.as_bytes());
            put(s.ct.as_bytes());
        }
        put(self.meta_ct.as_bytes());
        put(&self.size.to_be_bytes());
        put(&self.seg.to_be_bytes());
        put(&self.segs.to_be_bytes());
        out
    }
    /// Structural checks every party runs before trusting a header.
    pub fn validate(&self) -> Result<()> {
        ensure!(self.v == 1, "unsupported envelope version");
        uuid::Uuid::parse_str(&self.item_id).context("bad item id")?;
        self.from.parse::<iroh::PublicKey>().ok().context("bad sender id")?;
        ensure!(matches!(self.kind.as_str(), "chat" | "op" | "file"), "bad item kind");
        ensure!(!self.stanzas.is_empty() && self.stanzas.len() <= 16, "bad recipient count");
        let mut seen = std::collections::HashSet::new();
        for s in &self.stanzas {
            s.eid.parse::<iroh::PublicKey>().ok().context("bad recipient id")?;
            ensure!(seen.insert(s.eid.as_str()), "duplicate recipient");
            ensure!(s.epk.len() <= 64 && s.ct.len() <= 96, "bad stanza");
        }
        ensure!(self.meta_ct.len() <= 512 * 1024, "metadata too large");
        ensure!(self.seg == SEG, "bad segment size");
        ensure!(self.segs == segs_for(&self.kind, self.size), "bad segment count");
        ensure!(self.size <= 1 << 50, "payload too large");
        Ok(())
    }
    pub fn verify_sig(&self) -> bool {
        let Ok(pk) = self.from.parse::<iroh::PublicKey>() else { return false };
        let Ok(sig) = unb64(&self.sig) else { return false };
        let Ok(sig) = <[u8; 64]>::try_from(sig.as_slice()) else { return false };
        pk.verify(&self.signed_bytes(), &iroh::Signature::from_bytes(&sig)).is_ok()
    }
}

pub struct Recipient {
    pub eid: String,
    pub key: [u8; 32],
}

fn stanza_aad(item_id: &str, eid: &str) -> Vec<u8> {
    let mut aad = item_id.as_bytes().to_vec();
    aad.push(0);
    aad.extend_from_slice(eid.as_bytes());
    aad
}

fn meta_key(file_key: &[u8; 32], item_id: &str) -> [u8; 32] {
    hkdf32(file_key, item_id.as_bytes(), b"dropbeam-mbx-meta-v1")
}

/// Seal a new item. `file_key` is supplied only when re-sealing an item whose
/// payload was already (partly) uploaded — normally a fresh random key is used.
pub fn seal(
    signer: &iroh::SecretKey,
    item_id: &str,
    kind: &str,
    created_ms: u64,
    recipients: &[Recipient],
    meta: &[u8],
    size: u64,
) -> Result<(Envelope, [u8; 32])> {
    ensure!(!recipients.is_empty(), "no recipient keys");
    let file_key: [u8; 32] = rand::random();
    let mut stanzas = Vec::with_capacity(recipients.len());
    for r in recipients {
        let eph: [u8; 32] = rand::random();
        let epk = x25519_public(&eph);
        let ss = x25519(&eph, &r.key).context("recipient mailbox key is invalid")?;
        let mut salt = epk.to_vec();
        salt.extend_from_slice(&r.key);
        let wrap = hkdf32(&ss, &salt, b"dropbeam-mbx-wrap-v1");
        let ct = seal_with(&wrap, [0; 12], &stanza_aad(item_id, &r.eid), &file_key);
        stanzas.push(Stanza { eid: r.eid.clone(), epk: b64(&epk), ct: b64(&ct) });
    }
    let meta_ct = seal_with(&meta_key(&file_key, item_id), [0; 12], item_id.as_bytes(), meta);
    let mut env = Envelope {
        v: 1,
        item_id: item_id.to_owned(),
        from: signer.public().to_string(),
        kind: kind.to_owned(),
        created_ms,
        stanzas,
        meta_ct: b64(&meta_ct),
        size,
        seg: SEG,
        segs: segs_for(kind, size),
        sig: String::new(),
    };
    env.sig = b64(&signer.sign(&env.signed_bytes()).to_bytes());
    Ok((env, file_key))
}

/// Verify + unwrap an envelope addressed to this device: returns the item's
/// file key and decrypted metadata. Signature is checked FIRST, so a forged or
/// altered header never reaches the key-unwrapping code.
pub fn open(env: &Envelope, my_eid: &str, my_secret: &[u8; 32]) -> Result<([u8; 32], Vec<u8>)> {
    env.validate()?;
    ensure!(env.verify_sig(), "bad sender signature");
    let st = env.stanzas.iter().find(|s| s.eid == my_eid).context("not addressed to this device")?;
    let epk = key32(&st.epk)?;
    let ss = x25519(my_secret, &epk).context("bad ephemeral key")?;
    let mut salt = epk.to_vec();
    salt.extend_from_slice(&x25519_public(my_secret));
    let wrap = hkdf32(&ss, &salt, b"dropbeam-mbx-wrap-v1");
    let fk = open_with(&wrap, [0; 12], &stanza_aad(&env.item_id, my_eid), &unb64(&st.ct)?)
        .context("this device's key does not open the item")?;
    let file_key = <[u8; 32]>::try_from(fk.as_slice()).ok().context("bad file key")?;
    let meta = open_with(&meta_key(&file_key, &env.item_id), [0; 12], env.item_id.as_bytes(), &unb64(&env.meta_ct)?)
        .context("metadata does not decrypt")?;
    Ok((file_key, meta))
}

/// Payload segment cipher for one item.
pub struct PayloadKey {
    key: LessSafeKey,
    aad: Vec<u8>,
}

pub fn payload_key(file_key: &[u8; 32], item_id: &str) -> PayloadKey {
    let k = hkdf32(file_key, item_id.as_bytes(), b"dropbeam-mbx-payload-v1");
    PayloadKey { key: aead_key(&k), aad: item_id.as_bytes().to_vec() }
}

fn seg_nonce(idx: u64, last: bool) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[3..11].copy_from_slice(&idx.to_be_bytes());
    n[11] = u8::from(last);
    n
}

impl PayloadKey {
    /// Encrypt segment `idx` in place (appends the tag).
    pub fn seal_segment(&self, idx: u64, last: bool, buf: &mut Vec<u8>) {
        self.key
            .seal_in_place_append_tag(Nonce::assume_unique_for_key(seg_nonce(idx, last)), Aad::from(&self.aad), buf)
            .expect("ChaCha20-Poly1305 seal cannot fail for in-range input");
    }
    /// Decrypt segment `idx` in place (strips the tag).
    pub fn open_segment(&self, idx: u64, last: bool, buf: &mut Vec<u8>) -> Result<()> {
        let n = self
            .key
            .open_in_place(Nonce::assume_unique_for_key(seg_nonce(idx, last)), Aad::from(&self.aad), buf)
            .map_err(|_| anyhow::anyhow!("payload segment {idx} failed authentication"))?
            .len();
        buf.truncate(n);
        Ok(())
    }
}

/// Ciphertext offset where segment `idx` starts.
pub fn ct_offset(idx: u64) -> u64 {
    idx * CT_SEG
}

/// Plaintext length of segment `idx` of a `size`-byte payload with `segs` segments.
pub fn seg_len(size: u64, segs: u64, idx: u64) -> u64 {
    if idx + 1 < segs {
        SEG
    } else {
        size - (segs.saturating_sub(1)) * SEG
    }
}

/// Seal a small preview (push notification body) to ONE X25519 key:
/// `eph_pub(32) || nonce(12, zero) || ciphertext || tag`, which CryptoKit opens
/// as `ChaChaPoly.SealedBox(combined: bytes[32...])` with key
/// `HKDF<SHA256>(X25519(sk, eph_pub), salt: eph_pub || my_pub, info: "dropbeam-push-v1")`.
pub fn seal_small(recipient: &[u8; 32], pt: &[u8]) -> Result<String> {
    let eph: [u8; 32] = rand::random();
    let epk = x25519_public(&eph);
    let ss = x25519(&eph, recipient).context("bad push key")?;
    let mut salt = epk.to_vec();
    salt.extend_from_slice(recipient);
    let k = hkdf32(&ss, &salt, PUSH_INFO);
    let mut out = epk.to_vec();
    out.extend_from_slice(&[0u8; 12]);
    out.extend_from_slice(&seal_with(&k, [0; 12], b"", pt));
    Ok(b64(&out))
}

pub fn open_small(my_secret: &[u8; 32], sealed: &str) -> Result<Vec<u8>> {
    let raw = unb64(sealed)?;
    if raw.len() < 32 + 12 + 16 {
        bail!("sealed preview too short");
    }
    let epk = <[u8; 32]>::try_from(&raw[..32]).unwrap();
    let ss = x25519(my_secret, &epk).context("bad ephemeral key")?;
    let mut salt = epk.to_vec();
    salt.extend_from_slice(&x25519_public(my_secret));
    let k = hkdf32(&ss, &salt, PUSH_INFO);
    open_with(&k, [0; 12], b"", &raw[44..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> (iroh::SecretKey, [u8; 32]) {
        (iroh::SecretKey::generate(), rand::random())
    }

    #[test]
    fn seal_open_roundtrip_multi_device_and_payload() {
        let (sender, _) = device();
        let (b1, b1x) = device();
        let (b2, b2x) = device();
        let item = uuid::Uuid::new_v4().to_string();
        let recips = [
            Recipient { eid: b1.public().to_string(), key: x25519_public(&b1x) },
            Recipient { eid: b2.public().to_string(), key: x25519_public(&b2x) },
        ];
        let size = SEG * 2 + 123;
        let (env, fk) = seal(&sender, &item, "file", 42, &recips, b"{\"names\":[\"a\"]}", size).unwrap();
        assert_eq!(env.segs, 3);
        assert_eq!(env.ct_size(), size + 3 * TAG);
        for (eid, sk) in [(b1.public().to_string(), b1x), (b2.public().to_string(), b2x)] {
            let (k, meta) = open(&env, &eid, &sk).unwrap();
            assert_eq!(k, fk);
            assert_eq!(meta, b"{\"names\":[\"a\"]}");
        }
        // Payload roundtrip, including the short last segment.
        let pk = payload_key(&fk, &item);
        let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        let mut ct = Vec::new();
        for i in 0..env.segs {
            let start = (i * SEG) as usize;
            let len = seg_len(size, env.segs, i) as usize;
            let mut buf = data[start..start + len].to_vec();
            pk.seal_segment(i, i + 1 == env.segs, &mut buf);
            ct.extend_from_slice(&buf);
        }
        assert_eq!(ct.len() as u64, env.ct_size());
        let pk2 = payload_key(&fk, &item);
        let mut out = Vec::new();
        for i in 0..env.segs {
            let start = ct_offset(i) as usize;
            let len = (seg_len(size, env.segs, i) + TAG) as usize;
            let mut buf = ct[start..start + len].to_vec();
            pk2.open_segment(i, i + 1 == env.segs, &mut buf).unwrap();
            out.extend_from_slice(&buf);
        }
        assert_eq!(out, data);
        // Reordering / truncation are detected.
        let mut first = ct[..CT_SEG as usize].to_vec();
        assert!(pk2.open_segment(1, false, &mut first).is_err(), "segment swapped into another slot");
        let mut first = ct[..CT_SEG as usize].to_vec();
        assert!(pk2.open_segment(0, true, &mut first).is_err(), "truncation (non-last marked last)");
    }

    #[test]
    fn tampering_and_wrong_keys_are_rejected() {
        let (sender, _) = device();
        let (b, bx) = device();
        let (_, stranger_x) = device();
        let item = uuid::Uuid::new_v4().to_string();
        let recips = [Recipient { eid: b.public().to_string(), key: x25519_public(&bx) }];
        let (env, _) = seal(&sender, &item, "chat", 7, &recips, b"hello", 0).unwrap();
        let me = b.public().to_string();
        assert!(open(&env, &me, &stranger_x).is_err(), "someone else's key must not open it");
        for mutate in [
            (|e: &mut Envelope| e.created_ms += 1) as fn(&mut Envelope),
            |e| e.kind = "op".into(),
            |e| e.from = iroh::SecretKey::generate().public().to_string(),
            |e| e.meta_ct = b64(&[0u8; 21]),
            |e| e.stanzas[0].epk = b64(&x25519_public(&rand::random())),
        ] {
            let mut bad = env.clone();
            mutate(&mut bad);
            assert!(open(&bad, &me, &bx).is_err());
        }
        // A server re-signing with its own key is a different sender.
        let (forger, _) = device();
        let (forged, _) = seal(&forger, &item, "chat", 7, &recips, b"evil", 0).unwrap();
        assert_ne!(forged.from, env.from);
        // Low-order public keys never produce a usable secret.
        assert!(x25519(&bx, &[0u8; 32]).is_none());
    }

    #[test]
    fn mailbox_key_signature_binds_endpoint() {
        let (a, ax) = device();
        let pubk = x25519_public(&ax);
        let sig = sign_mailbox_key(&a, &pubk);
        assert!(verify_mailbox_key(&a.public().to_string(), &pubk, &sig));
        let other = iroh::SecretKey::generate().public().to_string();
        assert!(!verify_mailbox_key(&other, &pubk, &sig));
        assert!(!verify_mailbox_key(&a.public().to_string(), &x25519_public(&rand::random()), &sig));
    }

    #[test]
    fn small_seal_roundtrip() {
        let sk: [u8; 32] = rand::random();
        let sealed = seal_small(&x25519_public(&sk), b"{\"title\":\"Ashton\"}").unwrap();
        assert_eq!(open_small(&sk, &sealed).unwrap(), b"{\"title\":\"Ashton\"}");
        assert!(open_small(&rand::random(), &sealed).is_err());
    }
}
