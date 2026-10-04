//! Quick Send link rules (audit T13) and paged headers (audit T1).
//!
//! * A link lives for a TTL (default 24 h), then it is gone.
//! * The file list is frozen when the link is made: a puller gets exactly what
//!   the sender shared, and a file that changed or vanished since is refused
//!   with a clear sentence instead of streaming something else.
//! * The first device to pull owns the link; another device is told the link
//!   is in use instead of kicking the first one off mid-transfer (the old
//!   generation bump made two pullers supersede each other forever). The same
//!   device re-pulling — a resume — is always allowed.
//! * A puller that hits a canceled, expired or used link gets an explicit
//!   refusal frame, so it stops at once instead of retrying a dead link.
//! * Headers too big for one 1 MiB frame (thousands of files) are paged — only
//!   to pullers that advertise `pages_v`; older ones get the refusal.
use super::*;

pub(crate) const DEFAULT_TTL_HOURS: u64 = 24;
/// Receiver-side prefix of every refusal: never retried.
pub(crate) const REFUSED: &str = "link refused: ";
const PAGE_ITEMS: usize = 4000;
const PAGE_BYTES: usize = 512 * 1024;
/// A header this big (or bigger) goes out paged.
const SINGLE_FRAME_LIMIT: usize = 900 * 1024;
const MAX_PAGES: u64 = 4096;

static CANCELED: std::sync::LazyLock<Mutex<std::collections::VecDeque<String>>> = std::sync::LazyLock::new(Default::default);

/// Remember a canceled link's token so a late puller is told "canceled".
pub(crate) fn note_canceled(token: &str) {
    let mut c = CANCELED.lock().unwrap_or_else(|p| p.into_inner());
    c.push_back(token.to_owned());
    while c.len() > 256 { c.pop_front(); }
}

fn was_canceled(token: &str) -> bool {
    CANCELED.lock().unwrap_or_else(|p| p.into_inner()).iter().any(|t| t == token)
}

/// The configured TTL: `quick_send_ttl_hours` (0 / missing → 24 h).
pub(crate) fn ttl(config: Option<&Path>) -> Duration {
    let hours = config.map(|c| crate::settings::load(c, "", "").quick_send_ttl_hours).filter(|h| *h > 0)
        .map(u64::from).unwrap_or(DEFAULT_TTL_HOURS);
    Duration::from_secs(hours.min(24 * 30) * 3600)
}

/// Why a pull can't be served, as the frame the puller gets.
pub(crate) fn refusal(text: &str, canceled: bool) -> serde_json::Value {
    serde_json::json!({"error": text, "canceled": canceled, "final": true})
}

/// Decide whether `puller` may pull `token` right now.
pub(crate) fn admit(pending: Option<&PendingSend>, token: &str, puller: &str, now: Instant) -> std::result::Result<(), serde_json::Value> {
    let Some(p) = pending else {
        return Err(if was_canceled(token) { refusal("The sender canceled this transfer.", true) }
            else { refusal("This link has expired or was already used — ask for a new one.", false) });
    };
    if now >= p.expires_at {
        return Err(refusal("This link has expired — ask for a new one.", false));
    }
    let mut owner = p.puller.lock().unwrap_or_else(|e| e.into_inner());
    match owner.as_deref() {
        Some(o) if o != puller => Err(refusal("This link is already being received on another device.", false)),
        _ => { *owner = Some(puller.to_owned()); Ok(()) }
    }
}

/// The frozen item list still describes the files on disk (same size and
/// modified time, still regular files). `Err` names the first that changed.
pub(crate) fn unchanged(items: &[SendItem]) -> std::result::Result<(), String> {
    for (path, name, size, mtime) in items {
        let ok = std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == *size && mtime_secs(&m) == *mtime);
        if !ok {
            return Err(format!("\"{}\" changed or was removed since this link was made — ask for a new link.",
                name.rsplit('/').next().unwrap_or(name)));
        }
    }
    Ok(())
}

/// Write a Quick Send header: one frame when it fits, else paged when the
/// puller understands pages, else a refusal (and an error for the sender).
pub(crate) async fn write_header(send: &mut SendStream, header: &serde_json::Value, pages_ok: bool) -> Result<()> {
    if serde_json::to_vec(header)?.len() < SINGLE_FRAME_LIMIT {
        return write_frame(send, header).await;
    }
    if !pages_ok {
        let _ = write_frame(send, &refusal("This link has more files than your DropBeam can receive at once — update DropBeam, then try again.", false)).await;
        // Let the refusal reach the puller before the connection goes away.
        let _ = send.finish();
        let _ = tokio::time::timeout(Duration::from_secs(5), send.stopped()).await;
        anyhow::bail!("frame too large for the receiver's DropBeam version");
    }
    let items = header["items"].as_array().cloned().unwrap_or_default();
    let dirs = header["dirs"].as_array().cloned().unwrap_or_default();
    let mut pages: Vec<serde_json::Value> = vec![];
    let (mut page_items, mut page_dirs, mut bytes) = (vec![], vec![], 0usize);
    let mut push = |item: Option<serde_json::Value>, dir: Option<serde_json::Value>, pages: &mut Vec<serde_json::Value>, flush: bool| {
        if flush || page_items.len() + page_dirs.len() >= PAGE_ITEMS || bytes >= PAGE_BYTES {
            if !(page_items.is_empty() && page_dirs.is_empty()) {
                pages.push(serde_json::json!({"items": std::mem::take(&mut page_items), "dirs": std::mem::take(&mut page_dirs)}));
            }
            bytes = 0;
        }
        if let Some(i) = item { bytes += i.to_string().len(); page_items.push(i); }
        if let Some(d) = dir { bytes += d.to_string().len(); page_dirs.push(d); }
    };
    for i in items { push(Some(i), None, &mut pages, false); }
    for d in dirs { push(None, Some(d), &mut pages, false); }
    push(None, None, &mut pages, true);
    let mut first = header.clone();
    first["items"] = serde_json::json!([]);
    first["dirs"] = serde_json::json!([]);
    first["paged_v"] = serde_json::json!(1);
    first["pages"] = serde_json::json!(pages.len());
    write_frame(send, &first).await?;
    for page in &pages { write_frame(send, page).await?; }
    Ok(())
}

/// Read the rest of a Quick Send header after its first frame: a refusal
/// becomes a final error, a paged header is reassembled.
pub(crate) async fn read_header(recv: &mut RecvStream, first: serde_json::Value) -> Result<serde_json::Value> {
    if let Some(text) = first.get("error").and_then(|e| e.as_str()) {
        if first["canceled"] == true { return Err(errors::peer_stop_error(CancelReason::Cancel)); }
        anyhow::bail!("{REFUSED}{text}");
    }
    if first["paged_v"] != 1 { return Ok(first); }
    let pages = first["pages"].as_u64().unwrap_or(0);
    anyhow::ensure!(pages <= MAX_PAGES, "too many header pages");
    let mut header = first;
    let (mut items, mut dirs) = (vec![], vec![]);
    for _ in 0..pages {
        let page = read_frame(recv).await?;
        items.extend(page["items"].as_array().cloned().unwrap_or_default());
        dirs.extend(page["dirs"].as_array().cloned().unwrap_or_default());
    }
    header["items"] = serde_json::Value::Array(items);
    header["dirs"] = serde_json::Value::Array(dirs);
    if let Some(o) = header.as_object_mut() { o.remove("paged_v"); o.remove("pages"); }
    Ok(header)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(ttl: Duration) -> PendingSend {
        PendingSend { transfer_id: "t".into(), paths: vec![], names: vec![], total: 0,
            cancel: Arc::default(), gen: Arc::default(), items: Arc::default(), dirs: Arc::default(),
            expires_at: Instant::now() + ttl, puller: Arc::default() }
    }

    #[test]
    fn t13_ttl_binding_and_refusals() {
        let now = Instant::now();
        let p = pending(Duration::from_secs(60));
        assert!(admit(Some(&p), "tok", "dev-a", now).is_ok());
        assert!(admit(Some(&p), "tok", "dev-a", now).is_ok(), "the same device may re-pull (resume)");
        let busy = admit(Some(&p), "tok", "dev-b", now).unwrap_err();
        assert!(busy["error"].as_str().unwrap().contains("another device"));
        let expired = admit(Some(&p), "tok", "dev-a", now + Duration::from_secs(61)).unwrap_err();
        assert!(expired["error"].as_str().unwrap().contains("expired"));
        note_canceled("gone-tok");
        assert_eq!(admit(None, "gone-tok", "dev-a", now).unwrap_err()["canceled"], true);
        assert_eq!(admit(None, "never", "dev-a", now).unwrap_err()["canceled"], false);
    }

    #[test]
    fn t13_frozen_list_detects_changes() {
        let dir = std::env::temp_dir().join(format!("dropbeam-quick-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, b"one").unwrap();
        let (items, _, _) = gather_items(std::slice::from_ref(&f)).unwrap();
        assert!(unchanged(&items).is_ok());
        std::fs::write(&f, b"longer now").unwrap();
        assert!(unchanged(&items).unwrap_err().contains("a.txt"));
        std::fs::remove_file(&f).unwrap();
        assert!(unchanged(&items).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
