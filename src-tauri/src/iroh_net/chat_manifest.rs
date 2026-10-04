//! A friend send's whole-transfer manifest, sent ONCE (audit T1).
//!
//! Every push of a linked friend send used to carry the full `chatTransfer`
//! manifest (every name + size of the whole folder) in its header. A folder of
//! ~5k–15k+ files pushed that header past the 1 MiB frame cap, so such sends
//! always failed — and smaller ones re-sent the same list with every push.
//!
//! Now, when the manifest is big, the sender pages it to the receiver once per
//! connection + attempt on a `chat-manifest` stream, and each push header
//! carries a compact `chatTransfer` (`manifest: []` + `manifestRef`). The
//! receiver hydrates the header from its store before anything reads it.
//!
//! Compatibility: small manifests stay inline, byte-for-byte as before. An
//! older receiver answers the unknown kind with `{"kind":"ok"}`; the sender
//! then pushes WITHOUT a chat link (files still land; the chat card just
//! doesn't track them) instead of failing the whole send as it did before.
use super::*;

/// Manifests whose estimated JSON exceeds this go out of band.
pub(super) const INLINE_LIMIT: usize = 256 * 1024;
/// One page frame stays far below the 1 MiB frame cap.
const PAGE_BYTES: usize = 384 * 1024;
const PAGE_ITEMS: usize = 4000;
/// Abuse bounds on what a peer may park in our memory.
const MAX_FILES: u64 = 500_000;
const MAX_DIRS: u64 = 200_000;
const MAX_STORED: usize = 32;
const STORE_TTL: Duration = Duration::from_secs(6 * 3600);

struct Stored {
    files: Vec<crate::models::ChatFile>,
    dirs: Vec<String>,
    at: Instant,
}

/// Receiver side: `(peer endpoint id, sender's link id, attempt)` → manifest.
type StoreKey = (String, String, u64);
static STORE: std::sync::LazyLock<Mutex<HashMap<StoreKey, Stored>>> = std::sync::LazyLock::new(Default::default);
/// Sender side: `(connection, link id, attempt)` → receiver stored it (true) or
/// doesn't speak this (false). One offer per connection and attempt.
type OfferKey = (usize, String, u64);
static OFFERED: std::sync::LazyLock<Mutex<HashMap<OfferKey, bool>>> = std::sync::LazyLock::new(Default::default);

fn estimate(link: &crate::models::ChatTransferLink) -> usize {
    link.manifest.iter().map(|f| f.name.len() + 40).sum::<usize>()
        + link.directories.iter().map(|d| d.len() + 4).sum::<usize>()
}

/// Does this link's manifest need to travel out of band?
pub(super) fn needs_out_of_band(link: &crate::models::ChatTransferLink) -> bool {
    estimate(link) > INLINE_LIMIT
}

/// `link` without its (possibly huge) lists — never clones the manifest.
pub(super) fn light(link: &crate::models::ChatTransferLink) -> crate::models::ChatTransferLink {
    crate::models::ChatTransferLink {
        id: link.id.clone(), attempt: link.attempt, manifest: vec![], directories: vec![],
        batch_state: link.batch_state, bytes_done: link.bytes_done,
        completed_files: vec![], completed_paths: Default::default(),
        item_offset: link.item_offset, offset: link.offset, total: link.total, last: link.last,
    }
}

/// The per-push `chatTransfer` once the receiver holds the manifest.
pub(super) fn compact(link: &crate::models::ChatTransferLink) -> Result<serde_json::Value> {
    let mut v = serde_json::to_value(light(link))?;
    v["manifestRef"] = serde_json::json!({"v": 1, "count": link.manifest.len(), "dirs": link.directories.len()});
    Ok(v)
}

/// The `chatTransfer` header value for one push of `link` on `conn`: inline
/// (small, or any peer), compact (receiver holds it), or `None` = push without
/// a chat link (an older receiver and a manifest too big to inline).
pub(super) async fn header_value(conn: &Connection, link: &crate::models::ChatTransferLink) -> Result<Option<serde_json::Value>> {
    if !needs_out_of_band(link) {
        return Ok(Some(serde_json::to_value(link)?));
    }
    let key = (conn.stable_id(), link.id.clone(), link.attempt);
    let known = OFFERED.lock().unwrap().get(&key).copied();
    let stored = match known {
        Some(stored) => stored,
        None => {
            let stored = match offer(conn, link).await {
                Ok(stored) => stored,
                // A dead connection fails the push itself; don't cache that.
                Err(e) if conn.close_reason().is_some() => return Err(e),
                Err(e) => {
                    // A live peer that broke the stream doesn't speak this.
                    log::warn!("chat manifest offer failed: {e:#}");
                    false
                }
            };
            let mut offered = OFFERED.lock().unwrap();
            if offered.len() > 256 { offered.clear(); }
            offered.insert(key, stored);
            stored
        }
    };
    if stored { Ok(Some(compact(link)?)) } else {
        log::warn!("receiver predates paged chat manifests; sending {} files without a chat link", link.manifest.len());
        Ok(None)
    }
}

/// Page the manifest to the receiver. Ok(false) = it doesn't speak this.
async fn offer(conn: &Connection, link: &crate::models::ChatTransferLink) -> Result<bool> {
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, &serde_json::json!({
        "kind": "chat-manifest", "v": 1, "id": link.id, "attempt": link.attempt,
        "count": link.manifest.len(), "dirs": link.directories.len(), "total": link.total,
    })).await?;
    let reply = tokio::time::timeout(Duration::from_secs(20), read_frame(&mut recv)).await
        .context("chat manifest: no answer")??;
    if reply["ready"] != true || reply["chat_manifest_v"] != 1 {
        let _ = send.finish();
        return Ok(false);
    }
    for page in pages(link) {
        write_frame(&mut send, &page).await?;
    }
    send.finish()?;
    let done = tokio::time::timeout(Duration::from_secs(60), read_frame(&mut recv)).await
        .context("chat manifest: not confirmed")??;
    anyhow::ensure!(done["ok"] == true, "chat manifest refused: {}", done["error"].as_str().unwrap_or("unknown"));
    Ok(true)
}

fn pages(link: &crate::models::ChatTransferLink) -> Vec<serde_json::Value> {
    let mut out = vec![];
    let (mut files, mut dirs, mut bytes) = (vec![], vec![], 0usize);
    let flush = |files: &mut Vec<serde_json::Value>, dirs: &mut Vec<serde_json::Value>, bytes: &mut usize, out: &mut Vec<serde_json::Value>| {
        out.push(serde_json::json!({"files": std::mem::take(files), "dirs": std::mem::take(dirs), "last": false}));
        *bytes = 0;
    };
    for f in &link.manifest {
        if files.len() + dirs.len() >= PAGE_ITEMS || bytes + f.name.len() + 40 > PAGE_BYTES { flush(&mut files, &mut dirs, &mut bytes, &mut out); }
        bytes += f.name.len() + 40;
        files.push(serde_json::json!({"name": f.name, "size": f.size}));
    }
    for d in &link.directories {
        if files.len() + dirs.len() >= PAGE_ITEMS || bytes + d.len() + 8 > PAGE_BYTES { flush(&mut files, &mut dirs, &mut bytes, &mut out); }
        bytes += d.len() + 8;
        dirs.push(serde_json::json!(d));
    }
    flush(&mut files, &mut dirs, &mut bytes, &mut out);
    if let Some(last) = out.last_mut() { last["last"] = serde_json::json!(true); }
    out
}

/// Receiver: take a paged manifest from `peer` and keep it for its pushes.
pub(super) async fn serve(peer: &str, req: &serde_json::Value, send: &mut SendStream, recv: &mut RecvStream) -> Result<()> {
    let refuse = |e: &str| serde_json::json!({"ok": false, "error": e});
    let id = req["id"].as_str().unwrap_or("");
    let attempt = req["attempt"].as_u64().unwrap_or(0);
    let (count, dirs, total) = (req["count"].as_u64().unwrap_or(u64::MAX), req["dirs"].as_u64().unwrap_or(u64::MAX), req["total"].as_u64());
    if req["v"] != 1 || uuid::Uuid::parse_str(id).is_err() || attempt == 0 || count > MAX_FILES || dirs > MAX_DIRS || total.is_none() {
        write_frame(send, &refuse("invalid manifest")).await?;
        let _ = send.finish();
        return Ok(());
    }
    write_frame(send, &serde_json::json!({"ready": true, "chat_manifest_v": 1})).await?;
    let mut files = Vec::with_capacity(count.min(50_000) as usize);
    let mut all_dirs = Vec::with_capacity(dirs.min(50_000) as usize);
    let read = async {
        loop {
            let page = read_frame(recv).await?;
            for f in page["files"].as_array().context("bad page")? {
                files.push(crate::models::ChatFile {
                    name: f["name"].as_str().context("bad name")?.to_owned(),
                    size: f["size"].as_u64().context("bad size")?,
                });
            }
            for d in page["dirs"].as_array().context("bad page")? {
                all_dirs.push(d.as_str().context("bad dir")?.to_owned());
            }
            anyhow::ensure!(files.len() as u64 <= count && all_dirs.len() as u64 <= dirs, "manifest longer than announced");
            if page["last"] == true { break; }
        }
        anyhow::Ok(())
    };
    let outcome = match tokio::time::timeout(Duration::from_secs(120), read).await {
        Ok(Ok(())) => {
            let sum = files.iter().try_fold(0u64, |n, f| n.checked_add(f.size));
            if files.len() as u64 != count || all_dirs.len() as u64 != dirs || sum != total {
                Err("manifest does not match its announcement")
            } else { Ok(()) }
        }
        Ok(Err(_)) => Err("manifest stream broke"),
        Err(_) => Err("manifest stream timed out"),
    };
    match outcome {
        Ok(()) => {
            {
                let mut store = STORE.lock().unwrap();
                store.retain(|_, s| s.at.elapsed() < STORE_TTL);
                while store.len() >= MAX_STORED {
                    let Some(oldest) = store.iter().min_by_key(|(_, s)| s.at).map(|(k, _)| k.clone()) else { break };
                    store.remove(&oldest);
                }
                store.insert((peer.to_owned(), id.to_owned(), attempt), Stored { files, dirs: all_dirs, at: Instant::now() });
            }
            write_frame(send, &serde_json::json!({"ok": true})).await?;
        }
        Err(e) => write_frame(send, &refuse(e)).await?,
    }
    let _ = send.finish();
    Ok(())
}

/// Receiver: before anything reads a push header, put back the manifest a
/// compact `chatTransfer` refers to. If we no longer hold it (restart, evicted),
/// the push degrades to a plain push at the same item offset — the files land,
/// only the chat card's tracking is lost.
pub(super) fn hydrate(peer: &str, mut req: serde_json::Value) -> serde_json::Value {
    let Some(link) = req.get("chatTransfer") else { return req };
    if link.get("manifestRef").is_none() { return req; }
    let key = (peer.to_owned(), link["id"].as_str().unwrap_or("").to_owned(), link["attempt"].as_u64().unwrap_or(0));
    let held = STORE.lock().unwrap().get_mut(&key).map(|s| {
        s.at = Instant::now();
        (serde_json::to_value(&s.files).unwrap_or_default(), serde_json::to_value(&s.dirs).unwrap_or_default())
    });
    match held {
        Some((files, dirs)) => {
            let link = &mut req["chatTransfer"];
            link["manifest"] = files;
            link["directories"] = dirs;
            if let Some(o) = link.as_object_mut() { o.remove("manifestRef"); }
        }
        None => {
            log::warn!("push refers to a chat manifest we don't hold; receiving it as a plain push");
            let offset = req["chatTransfer"]["itemOffset"].as_u64().unwrap_or(0);
            if let Some(o) = req.as_object_mut() { o.remove("chatTransfer"); }
            req["location_item_offset"] = serde_json::json!(offset);
        }
    }
    req
}

/// Tests: how many manifests `peer` has parked here.
#[cfg(test)]
pub(super) fn held_for(peer: &str) -> usize {
    STORE.lock().unwrap().keys().filter(|k| k.0 == peer).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hydrate_restores_or_degrades() {
        let id = uuid::Uuid::new_v4().to_string();
        STORE.lock().unwrap().insert(("peer-h".into(), id.clone(), 7), Stored {
            files: vec![crate::models::ChatFile { name: "a".into(), size: 1 }], dirs: vec!["d".into()], at: Instant::now() });
        let req = serde_json::json!({"kind": "files", "chatTransfer": {"id": id, "attempt": 7, "manifest": [], "directories": [], "itemOffset": 3, "manifestRef": {"v": 1}}});
        let full = hydrate("peer-h", req.clone());
        assert_eq!(full["chatTransfer"]["manifest"][0]["name"], "a");
        assert_eq!(full["chatTransfer"]["directories"][0], "d");
        assert!(full["chatTransfer"].get("manifestRef").is_none());
        // Another peer can't use it; an unknown ref degrades to a plain push.
        let other = hydrate("peer-x", req);
        assert!(other.get("chatTransfer").is_none());
        assert_eq!(other["location_item_offset"], 3);
        // Small manifests and plain pushes pass through untouched.
        let plain = serde_json::json!({"kind": "files", "chatTransfer": {"id": "x", "manifest": [{"name": "a", "size": 1}]}});
        assert_eq!(hydrate("peer-h", plain.clone()), plain);
    }

    #[test]
    fn pages_cover_everything_under_the_frame_cap() {
        let link: crate::models::ChatTransferLink = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4().to_string(), "attempt": 1,
            "manifest": (0..30_000).map(|i| serde_json::json!({"name": format!("{}/{i}", "x".repeat(80)), "size": i})).collect::<Vec<_>>(),
            "directories": (0..5000).map(|i| format!("dir {i}")).collect::<Vec<_>>(), "offset": 0, "total": 0, "last": true})).unwrap();
        let pages = pages(&link);
        assert!(pages.len() > 1);
        assert!(pages.iter().all(|p| serde_json::to_vec(p).unwrap().len() < MAX_HEADER));
        assert_eq!(pages.iter().map(|p| p["files"].as_array().unwrap().len()).sum::<usize>(), 30_000);
        assert_eq!(pages.iter().map(|p| p["dirs"].as_array().unwrap().len()).sum::<usize>(), 5000);
        assert!(pages.last().unwrap()["last"] == true && pages.iter().filter(|p| p["last"] == true).count() == 1);
        let compact = compact(&link).unwrap();
        assert!(serde_json::to_vec(&compact).unwrap().len() < 1024);
    }
}
