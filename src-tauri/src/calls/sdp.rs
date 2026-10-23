//! Session descriptions crossing the call stream, checked and tuned in one
//! place for every platform's call screen.
//!
//! - Every SDP must carry a DTLS fingerprint (sha-256 or stronger). It travels
//!   only inside the authenticated iroh stream, so WebRTC's DTLS handshake proves
//!   the media keys belong to the device we dialed — the relay path in between
//!   (our bridge, iroh's relay) can't swap them.
//! - Only relay candidates on the call's virtual addresses (10.77.0.x) survive:
//!   a peer can't steer this device's WebRTC engine at other hosts (the local
//!   TURN server refuses them too).
//! - Our own Opus line asks for in-band FEC, no DTX, mono, 48 kHz and up to
//!   128 kbps — the fmtp of a description says what its author wants to
//!   receive, so each side tunes its own.

use anyhow::{bail, ensure, Result};

pub const MAX_SDP: usize = 64 * 1024;
const OPUS_FMTP: &str = "minptime=10;useinbandfec=1;usedtx=0;stereo=0;sprop-stereo=0;maxaveragebitrate=128000;maxplaybackrate=48000";

fn lines(s: &str) -> impl Iterator<Item = &str> { s.split('\n').map(|l| l.trim_end_matches('\r')).filter(|l| !l.is_empty()) }

fn check(s: &str) -> Result<()> {
    ensure!(s.len() <= MAX_SDP, "Call description too large.");
    ensure!(s.starts_with("v=0"), "Not a call description.");
    let strong = lines(s).filter_map(|l| l.strip_prefix("a=fingerprint:"))
        .any(|f| { let alg = f.split(' ').next().unwrap_or("").to_ascii_lowercase(); matches!(alg.as_str(), "sha-256" | "sha-384" | "sha-512") });
    if !strong { bail!("The call isn't encrypted (no DTLS fingerprint)."); }
    if lines(s).any(|l| l.starts_with("a=fingerprint:") && l.to_ascii_lowercase().starts_with("a=fingerprint:sha-1 ")) {
        bail!("The call uses a weak fingerprint.");
    }
    Ok(())
}

/// A candidate line we keep: relay type on a 10.77.0.x virtual address.
fn keep_candidate(l: &str) -> bool {
    let f: Vec<&str> = l.split_whitespace().collect();
    // a=candidate:<foundation> <component> <transport> <priority> <address> <port> typ <type> …
    f.len() >= 8 && f[2].eq_ignore_ascii_case("udp") && f[6] == "typ" && f[7] == "relay"
        && f[4].starts_with("10.77.0.") && f[4].parse::<std::net::Ipv4Addr>().is_ok()
}

fn filter(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for l in lines(s) {
        if l.starts_with("a=candidate:") && !keep_candidate(l) { continue; }
        out.push_str(l);
        out.push_str("\r\n");
    }
    out
}

/// A description arriving from the other side.
pub fn incoming(s: &str) -> Result<String> {
    check(s)?;
    Ok(filter(s))
}

/// Our own call screen's description, on its way out.
pub fn outgoing(s: &str) -> Result<String> {
    check(s)?;
    let s = filter(s);
    let opus: Vec<String> = lines(&s).filter_map(|l| l.strip_prefix("a=rtpmap:"))
        .filter(|r| r.to_ascii_lowercase().contains(" opus/48000")).filter_map(|r| r.split(' ').next().map(String::from)).collect();
    let mut out = String::with_capacity(s.len() + 128);
    let mut tuned: Vec<&str> = Vec::new();
    for l in lines(&s) {
        if let Some(pt) = opus.iter().find(|pt| l.starts_with(&format!("a=fmtp:{pt} "))) {
            out.push_str(&format!("a=fmtp:{pt} {OPUS_FMTP}\r\n"));
            tuned.push(pt);
            continue;
        }
        out.push_str(l);
        out.push_str("\r\n");
        if let Some(pt) = l.strip_prefix("a=rtpmap:").and_then(|r| r.split(' ').next()).filter(|pt| opus.iter().any(|o| o == pt)) {
            // No fmtp line follows for this payload? Then add one right here.
            let has = lines(&s).any(|x| x.starts_with(&format!("a=fmtp:{pt} ")));
            if !has { out.push_str(&format!("a=fmtp:{pt} {OPUS_FMTP}\r\n")); }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SDP: &str = "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\na=group:BUNDLE 0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111 0\r\nc=IN IP4 0.0.0.0\r\n\
a=candidate:1 1 udp 41885439 10.77.0.1 40000 typ relay raddr 192.168.1.5 rport 5000 generation 0\r\n\
a=candidate:2 1 udp 2122260223 192.168.1.5 5000 typ host generation 0\r\n\
a=candidate:3 1 udp 41885439 8.8.8.8 40000 typ relay raddr 0.0.0.0 rport 0\r\n\
a=candidate:4 1 tcp 41885439 10.77.0.1 40000 typ relay\r\n\
a=fingerprint:sha-256 AB:CD\r\na=setup:actpass\r\na=mid:0\r\na=rtpmap:111 opus/48000/2\r\na=rtcp-fb:111 transport-cc\r\na=fmtp:111 minptime=10;useinbandfec=1\r\na=rtpmap:0 PCMU/8000\r\n";

    #[test]
    fn only_virtual_relay_candidates_survive() {
        let s = incoming(SDP).unwrap();
        let c: Vec<&str> = s.lines().filter(|l| l.starts_with("a=candidate")).collect();
        assert_eq!(c.len(), 1);
        assert!(c[0].contains("10.77.0.1 40000 typ relay"));
        assert!(s.ends_with("\r\n"));
    }

    #[test]
    fn opus_is_tuned_for_quality() {
        let s = outgoing(SDP).unwrap();
        assert!(s.contains("a=fmtp:111 minptime=10;useinbandfec=1;usedtx=0;stereo=0;sprop-stereo=0;maxaveragebitrate=128000;maxplaybackrate=48000\r\n"));
        assert_eq!(s.matches("a=fmtp:111").count(), 1);
        // No fmtp at all → one is added after the rtpmap.
        let bare = SDP.replace("a=fmtp:111 minptime=10;useinbandfec=1\r\n", "");
        let s = outgoing(&bare).unwrap();
        assert!(s.contains("a=rtpmap:111 opus/48000/2\r\na=fmtp:111 minptime=10;useinbandfec=1;usedtx=0"));
    }

    #[test]
    fn unencrypted_or_odd_descriptions_are_refused() {
        assert!(incoming(&SDP.replace("a=fingerprint:sha-256 AB:CD\r\n", "")).is_err());
        assert!(incoming(&SDP.replace("sha-256", "sha-1")).is_err());
        assert!(incoming(&SDP.replace("sha-256 AB:CD", "sha-1 AB:CD\r\na=fingerprint:sha-256 AB:CD")).is_err());
        assert!(incoming("hello").is_err());
        assert!(incoming(&format!("{SDP}{}", "a=x\r\n".repeat(20000))).is_err());
    }
}
