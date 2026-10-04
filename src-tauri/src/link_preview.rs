//! Link previews in chat (GitHub #47), the iMessage way: only the SENDER's
//! device fetches the page, and a small preview (title, description, site
//! name, a downscaled JPEG thumbnail) travels inside the chat message. The
//! receiver never contacts the site, so it can't learn their IP address.
//!
//! The fetch is deliberately narrow: http(s) only, public addresses only (the
//! host is resolved once, every address checked, and the connection pinned to
//! it — no DNS-rebinding into the LAN), redirects followed by hand and
//! re-checked, short timeouts, capped bodies, HTML read as text (no JS), and an
//! image re-encoded from scratch. Incoming previews are re-validated
//! (`sanitize`) before they're stored or shown.

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tauri::Url;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LinkPreview {
    /// The page the preview describes (after redirects).
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site_name: Option<String>,
    /// `data:image/jpeg;base64,…` — at most `MAX_IMAGE_B64` characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default)]
    pub image_w: u32,
    #[serde(default)]
    pub image_h: u32,
}

const MAX_URL: usize = 2048;
const MAX_TITLE: usize = 200;
const MAX_DESC: usize = 300;
const MAX_SITE: usize = 80;
/// Base64 characters of the thumbnail (≈ 90 KB of JPEG) — keeps a message well
/// under every frame cap, direct or through a Transfer Server.
const MAX_IMAGE_B64: usize = 120_000;
const MAX_HTML: usize = 768 * 1024;
const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
const THUMB_MAX_SIDE: u32 = 480;
const MAX_REDIRECTS: usize = 4;
/// Everything — page, redirects, image — must finish within this.
pub const BUDGET: Duration = Duration::from_secs(8);
const UA: &str = "Mozilla/5.0 (compatible; DropBeam link preview; +https://github.com/lman80/dropbeam) facebookexternalhit/1.1";

/// The first http(s) link in a message, as typed (trailing punctuation off).
pub fn first_url(text: &str) -> Option<String> {
    for word in text.split(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == '"') {
        let lower = word.to_ascii_lowercase();
        let Some(start) = lower.find("https://").or_else(|| lower.find("http://")) else { continue };
        let mut url = &word[start..];
        while let Some(last) = url.chars().last() {
            let unbalanced_paren = last == ')' && url.matches('(').count() < url.matches(')').count();
            if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | ']' | '}') || unbalanced_paren {
                url = &url[..url.len() - last.len_utf8()];
            } else {
                break;
            }
        }
        if url.len() > MAX_URL {
            continue;
        }
        if let Ok(parsed) = Url::parse(url) {
            if matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some_and(|h| h.contains('.') || h.starts_with('[')) {
                return Some(url.to_string());
            }
        }
    }
    None
}

/// Only addresses on the public internet: never this machine, the LAN, CGNAT,
/// link-local (cloud metadata), multicast or reserved space.
pub fn ip_allowed(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || (o[0] == 192 && o[1] == 0 && o[2] == 2) // documentation
                || (o[0] == 198 && o[1] == 51 && o[2] == 100)
                || (o[0] == 203 && o[1] == 0 && o[2] == 113)
                || o[0] == 0
                || (o[0] == 100 && (64..128).contains(&o[1])) // CGNAT 100.64/10
                || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0/24
                || (o[0] == 198 && (18..20).contains(&o[1])) // benchmarking
                || o[0] >= 240) // reserved
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return ip_allowed(IpAddr::V4(v4));
            }
            let s = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local fc00::/7
                || (s[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
                || (s[0] == 0x64 && s[1] == 0xff9b) // NAT64 (can reach v4 private)
                || (s[0] == 0x2001 && s[1] == 0x0db8)) // documentation
        }
    }
}

/// Resolve `url`'s host and check every address; the first one is used for the
/// connection (pinned), so a second lookup can't swap in a private address.
async fn resolve_public(url: &Url) -> Option<SocketAddr> {
    if !matches!(url.scheme(), "http" | "https") || url.as_str().len() > MAX_URL {
        return None;
    }
    let host = url.host_str()?.trim_start_matches('[').trim_end_matches(']').to_string();
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") || host.ends_with(".localhost") {
        return None;
    }
    let port = url.port_or_known_default()?;
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port)).await.ok()?.collect();
    if addrs.is_empty() || !addrs.iter().all(|a| ip_allowed(a.ip())) {
        log::info!("link preview: skipped a non-public host");
        return None;
    }
    addrs.first().copied()
}

/// GET with hand-followed, re-checked redirects. Returns the final URL + response.
async fn get(url: &Url, accept: &str) -> Option<(Url, reqwest::Response)> {
    let mut url = url.clone();
    for _ in 0..=MAX_REDIRECTS {
        let addr = resolve_public(&url).await?;
        let host = url.host_str()?.to_string();
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(6))
            .resolve(&host, addr)
            .user_agent(UA)
            .build()
            .ok()?;
        let resp = client.get(url.clone()).header("Accept", accept).send().await.ok()?;
        if resp.status().is_redirection() {
            let next = resp.headers().get(reqwest::header::LOCATION)?.to_str().ok()?;
            url = url.join(next).ok()?;
            continue;
        }
        if !resp.status().is_success() {
            return None;
        }
        return Some((url, resp));
    }
    None
}

/// Read at most `cap` bytes of a body (stops early instead of failing).
async fn read_capped(mut resp: reqwest::Response, cap: usize, stop_at_head_end: bool) -> Vec<u8> {
    let mut out = Vec::new();
    while let Ok(Some(chunk)) = resp.chunk().await {
        let room = cap.saturating_sub(out.len());
        out.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if out.len() >= cap {
            break;
        }
        if stop_at_head_end && contains_ci(&out, b"</head") {
            break;
        }
    }
    out
}

fn contains_ci(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
}

/// Fetch a preview for `url`. None on any failure — the message just goes
/// without one. Callers bound this with `BUDGET`.
pub async fn fetch(url: &str) -> Option<LinkPreview> {
    let start = Url::parse(url).ok()?;
    let (final_url, resp) = get(&start, "text/html,application/xhtml+xml;q=0.9,*/*;q=0.1").await?;
    let ctype = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
    if ctype.starts_with("image/") {
        // A direct image link: the thumbnail is the preview.
        let bytes = read_capped(resp, MAX_IMAGE_BYTES, false).await;
        let (image, w, h) = thumbnail(bytes).await?;
        return Some(LinkPreview {
            site_name: final_url.host_str().map(display_host),
            // The link as written: the receiver only shows a card whose host
            // matches the link in the text (S6), so a redirect must not change it.
            url: start.to_string(),
            title: None,
            description: None,
            image: Some(image),
            image_w: w,
            image_h: h,
        });
    }
    if !(ctype.contains("text/html") || ctype.contains("application/xhtml")) {
        return None;
    }
    let body = read_capped(resp, MAX_HTML, true).await;
    let meta = parse_html(&String::from_utf8_lossy(&body));
    let mut preview = LinkPreview {
        url: start.to_string(),
        title: meta.title,
        description: meta.description,
        site_name: meta.site_name.or_else(|| final_url.host_str().map(display_host)),
        image: None,
        image_w: 0,
        image_h: 0,
    };
    if let Some(img) = meta.image.and_then(|i| final_url.join(&i).ok()) {
        if let Some((_, resp)) = get(&img, "image/*").await {
            let ok = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok())
                .is_some_and(|t| t.to_ascii_lowercase().starts_with("image/"));
            if ok {
                if let Some((data, w, h)) = thumbnail(read_capped(resp, MAX_IMAGE_BYTES, false).await).await {
                    preview.image = Some(data);
                    preview.image_w = w;
                    preview.image_h = h;
                }
            }
        }
    }
    sanitize(preview)
}

/// A friend's preview, kept only when it describes the link actually in the
/// message text (same host, ignoring case and a leading "www."): otherwise a
/// message linking evil.example could carry a card that claims to be a bank (S6).
pub fn for_text(p: LinkPreview, text: &str) -> Option<LinkPreview> {
    let host = |u: &str| Url::parse(u).ok().and_then(|u| u.host_str().map(|h| display_host(&h.to_ascii_lowercase())));
    let linked = host(&first_url(text)?)?;
    (host(&p.url)? == linked).then_some(p)
}

fn display_host(h: &str) -> String {
    h.trim_start_matches("www.").to_string()
}

/// Decode any supported image and re-encode a small JPEG as a data URL.
async fn thumbnail(bytes: Vec<u8>) -> Option<(String, u32, u32)> {
    if bytes.is_empty() {
        return None;
    }
    tokio::task::spawn_blocking(move || {
        let img = image::load_from_memory(&bytes).ok()?;
        for side in [THUMB_MAX_SIDE, 360, 240] {
            let t = if img.width() > side || img.height() > side { img.thumbnail(side, side) } else { img.clone() };
            let rgb = t.to_rgb8();
            let mut out = Vec::new();
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 72);
            enc.encode_image(&rgb).ok()?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&out);
            if b64.len() + 23 <= MAX_IMAGE_B64 {
                return Some((format!("data:image/jpeg;base64,{b64}"), rgb.width(), rgb.height()));
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

#[derive(Debug, Default, PartialEq)]
pub struct Meta {
    pub title: Option<String>,
    pub description: Option<String>,
    pub site_name: Option<String>,
    pub image: Option<String>,
}

/// Open Graph / Twitter / plain-HTML metadata from a page's markup. Text only —
/// nothing is executed.
pub fn parse_html(html: &str) -> Meta {
    use std::sync::OnceLock;
    static META: OnceLock<regex::Regex> = OnceLock::new();
    static ATTR: OnceLock<regex::Regex> = OnceLock::new();
    static TITLE: OnceLock<regex::Regex> = OnceLock::new();
    let meta_re = META.get_or_init(|| regex::Regex::new(r"(?is)<meta\b[^>]*>").unwrap());
    let attr_re = ATTR.get_or_init(|| {
        regex::Regex::new(r#"(?is)([a-z_:.-]+)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>/]+))"#).unwrap()
    });
    let title_re = TITLE.get_or_init(|| regex::Regex::new(r"(?is)<title\b[^>]*>(.*?)</title>").unwrap());

    let mut tags: std::collections::HashMap<String, String> = Default::default();
    for m in meta_re.find_iter(html) {
        let (mut key, mut content) = (None, None);
        for c in attr_re.captures_iter(m.as_str()) {
            let name = c[1].to_ascii_lowercase();
            let value = c.get(2).or(c.get(3)).or(c.get(4)).map(|v| v.as_str()).unwrap_or("");
            match name.as_str() {
                "property" | "name" | "itemprop" => key = Some(value.to_ascii_lowercase()),
                "content" => content = Some(value.to_string()),
                _ => {}
            }
        }
        if let (Some(k), Some(v)) = (key, content) {
            tags.entry(k).or_insert(v);
        }
    }
    let pick = |keys: &[&str]| keys.iter().find_map(|k| tags.get(*k)).map(|v| clean(v)).filter(|v| !v.is_empty());
    let page_title = title_re.captures(html).map(|c| clean(&c[1])).filter(|t| !t.is_empty());
    Meta {
        title: pick(&["og:title", "twitter:title"]).or(page_title),
        description: pick(&["og:description", "twitter:description", "description"]),
        site_name: pick(&["og:site_name", "application-name"]),
        image: tags.get("og:image:secure_url").or(tags.get("og:image")).or(tags.get("og:image:url"))
            .or(tags.get("twitter:image")).or(tags.get("twitter:image:src"))
            .map(|v| decode_entities(v.trim())).filter(|v| !v.is_empty() && !v.starts_with("data:")),
    }
}

/// Entities decoded, whitespace collapsed.
fn clean(s: &str) -> String {
    decode_entities(s).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn clip(s: Option<String>, max: usize) -> Option<String> {
    let s = s?;
    let s: String = s.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string();
    if s.is_empty() {
        return None;
    }
    Some(if s.chars().count() > max { format!("{}…", s.chars().take(max - 1).collect::<String>().trim_end()) } else { s })
}

/// Bound and check a preview — ours before sending, a friend's before storing.
/// Anything malformed is dropped; a preview with nothing to show is None.
pub fn sanitize(p: LinkPreview) -> Option<LinkPreview> {
    let url = Url::parse(&p.url).ok().filter(|u| matches!(u.scheme(), "http" | "https") && p.url.len() <= MAX_URL)?;
    let image = p.image.filter(|i| {
        i.len() <= MAX_IMAGE_B64
            && i.strip_prefix("data:image/jpeg;base64,")
                .is_some_and(|b| b.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'=')))
    });
    let out = LinkPreview {
        url: url.to_string(),
        title: clip(p.title, MAX_TITLE),
        description: clip(p.description, MAX_DESC),
        site_name: clip(p.site_name, MAX_SITE),
        image_w: if image.is_some() { p.image_w.min(4096) } else { 0 },
        image_h: if image.is_some() { p.image_h.min(4096) } else { 0 },
        image,
    };
    (out.title.is_some() || out.image.is_some()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preview_must_describe_the_link_in_the_text() {
        let p = |url: &str| LinkPreview { url: url.into(), title: Some("PayPal — Log in".into()), description: None,
            site_name: Some("PayPal".into()), image: None, image_w: 0, image_h: 0 };
        assert!(for_text(p("https://www.paypal.com/signin"), "pay here https://evil.example/login").is_none());
        assert!(for_text(p("https://paypal.com.evil.example/"), "https://paypal.com").is_none());
        assert!(for_text(p("https://paypal.com/"), "no link at all").is_none());
        assert!(for_text(p("https://www.Example.com/a"), "see https://example.com/b").is_some());
        assert!(for_text(p("https://example.com/a"), "see HTTPS://WWW.EXAMPLE.COM").is_some());
    }

    #[test]
    fn finds_the_first_link() {
        assert_eq!(first_url("look https://example.com/a?b=1."), Some("https://example.com/a?b=1".into()));
        assert_eq!(first_url("(see https://en.wikipedia.org/wiki/Rust_(programming_language))"),
            Some("https://en.wikipedia.org/wiki/Rust_(programming_language)".into()));
        assert_eq!(first_url("HTTP://Example.com, then"), Some("HTTP://Example.com".into()));
        assert_eq!(first_url("no link here"), None);
        assert_eq!(first_url("ftp://example.com file://x"), None);
        assert_eq!(first_url("http://localhost:3000 is local"), None, "a bare single-label host isn't a web link");
    }

    #[test]
    fn only_public_addresses_are_fetched() {
        for bad in ["127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.10", "169.254.169.254", "100.64.0.1",
            "0.0.0.0", "224.0.0.1", "255.255.255.255", "::1", "fe80::1", "fd00::1", "::ffff:192.168.0.1", "64:ff9b::a00:1"] {
            assert!(!ip_allowed(bad.parse().unwrap()), "{bad} must be refused");
        }
        for good in ["93.184.216.34", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(ip_allowed(good.parse().unwrap()), "{good} is public");
        }
    }

    #[test]
    fn parses_open_graph_and_falls_back_to_html() {
        let html = r#"<html><head><title> Plain &amp; Title </title>
            <meta property="og:title" content="The &quot;Real&quot; Title">
            <meta content='A description' name='description'>
            <meta property="og:site_name" content="Example">
            <meta property="og:image" content="/img/card.png">
            <script>document.title = "nope"</script></head><body></body></html>"#;
        let m = parse_html(html);
        assert_eq!(m.title.as_deref(), Some("The \"Real\" Title"));
        assert_eq!(m.description.as_deref(), Some("A description"));
        assert_eq!(m.site_name.as_deref(), Some("Example"));
        assert_eq!(m.image.as_deref(), Some("/img/card.png"));
        let plain = parse_html("<title>Just &#39;this&#x21;</title>");
        assert_eq!(plain.title.as_deref(), Some("Just 'this!"));
        assert_eq!(plain.image, None);
    }

    #[test]
    fn sanitize_bounds_and_rejects() {
        let long = "x".repeat(1000);
        let p = sanitize(LinkPreview {
            url: "https://example.com/".into(),
            title: Some(format!("{long}\u{7}")),
            description: Some(long.clone()),
            site_name: Some(long.clone()),
            image: Some("data:image/jpeg;base64,AAAA".into()),
            image_w: 99_999,
            image_h: 10,
        })
        .unwrap();
        assert!(p.title.as_ref().unwrap().chars().count() <= MAX_TITLE);
        assert!(p.description.as_ref().unwrap().chars().count() <= MAX_DESC);
        assert_eq!(p.image_w, 4096);
        // A script URL, a non-JPEG / oversized / non-base64 image, or nothing to show → dropped.
        let base = LinkPreview { url: "https://e.com/".into(), title: Some("t".into()), description: None, site_name: None, image: None, image_w: 0, image_h: 0 };
        assert!(sanitize(LinkPreview { url: "javascript:alert(1)".into(), ..base.clone() }).is_none());
        assert_eq!(sanitize(LinkPreview { image: Some("data:image/svg+xml;base64,AAAA".into()), ..base.clone() }).unwrap().image, None);
        assert_eq!(sanitize(LinkPreview { image: Some(format!("data:image/jpeg;base64,{}", "A".repeat(MAX_IMAGE_B64))), ..base.clone() }).unwrap().image, None);
        assert_eq!(sanitize(LinkPreview { image: Some("data:image/jpeg;base64,<script>".into()), ..base.clone() }).unwrap().image, None);
        assert!(sanitize(LinkPreview { title: None, ..base }).is_none());
    }

    #[test]
    fn old_and_new_wire_shapes_round_trip() {
        // A message frame from an older build has no preview; a newer one's extra
        // field is plain JSON an older build simply never reads.
        let p = LinkPreview { url: "https://e.com/".into(), title: Some("T".into()), description: None, site_name: None, image: None, image_w: 0, image_h: 0 };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v, serde_json::json!({ "url": "https://e.com/", "title": "T", "imageW": 0, "imageH": 0 }));
        let back: LinkPreview = serde_json::from_value(serde_json::json!({ "url": "https://e.com/", "title": "T", "future": 1 })).unwrap();
        assert_eq!(back, p);
    }

    /// Network: `cargo test --lib link_preview -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn fetches_a_real_page() {
        for url in ["https://github.com/tauri-apps/tauri", "https://www.youtube.com/watch?v=dQw4w9WgXcQ", "http://127.0.0.1:1/x"] {
            let p = tokio::time::timeout(BUDGET, fetch(url)).await.ok().flatten();
            println!("{url} → {:?}", p.as_ref().map(|p| (&p.title, &p.site_name, p.image.as_ref().map(|i| i.len()), p.image_w, p.image_h)));
        }
    }

    #[test]
    fn a_preview_rides_the_chat_frame() {
        let mut m: crate::chat::ChatMessage = serde_json::from_value(serde_json::json!({
            "id": "m1", "peerId": "f1", "fromMe": true, "kind": "text", "text": "see https://e.com", "ts": 1
        })).unwrap();
        assert!(crate::iroh_net::chat_payload(&m, "f1", "Me").get("linkPreview").is_none());
        m.link_preview = Some(LinkPreview { url: "https://e.com/".into(), title: Some("E".into()), description: None, site_name: None, image: None, image_w: 0, image_h: 0 });
        let frame = crate::iroh_net::chat_payload(&m, "f1", "Me");
        assert_eq!(frame["linkPreview"]["title"], "E");
        assert_eq!(frame["text"], "see https://e.com", "the text is unchanged for builds that ignore the preview");
    }
}
