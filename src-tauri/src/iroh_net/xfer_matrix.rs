//! Transfer edge-case MATRIX — every user-facing send path crossed with the
//! hostile names, sizes and timing that have broken (or could break) real
//! transfers. Everything runs over two REAL in-process iroh endpoints on
//! loopback (relay + discovery disabled), through the same engine functions the
//! app runs:
//!
//!  * `Push`     — a single-shot friend push (`send_files` → negotiated receive).
//!  * `Friend`   — the app's friend/chat-attachment path: a chat-linked push, and
//!                 for >1 file the split `send_friend_batch` (files.stat skip +
//!                 small batches + per-big-file resumable pushes).
//!  * `Quick`    — Quick Send: a ticket pull with integrity (`serve_pull_verified`
//!                 → `read_pull_files_negotiated`).
//!  * `Location` — an upload into a hosted Location (headless host `accept_loop`,
//!                 `LocationBatch::send_attempt`).
//!
//! Every case asserts, for every file the sender's manifest carried: the landed
//! bytes are sha256-identical, the landed NAME is the receiver's deterministic
//! mapping of the sent name, the modified-time survived, and NOTHING else
//! appeared at the destination (no stray partials, no duplicate copies).
//! Every transfer runs under a deadline — a hang is a failure, never a stuck CI.
use super::*;
use sha2::{Digest, Sha256};

// ── harness ─────────────────────────────────────────────────────────────────

/// Test-only receive throttle, keyed by destination directory: the engine's
/// receive loops call `throttle(<file being written>)` per chunk, so ONE test
/// can slow its own receiver (to break a transfer provably mid-stream) without
/// touching any other test running in the same process.
static THROTTLED: std::sync::LazyLock<Mutex<HashMap<PathBuf, Duration>>> = std::sync::LazyLock::new(Default::default);
pub(super) async fn throttle(writing: &Path) {
    let delay = {
        let map = THROTTLED.lock().unwrap();
        if map.is_empty() { return; }
        writing.ancestors().find_map(|a| map.get(a).copied())
    };
    if let Some(d) = delay { tokio::time::sleep(d).await; }
}
/// The upload limiter is process-global and loopback counts as "not LAN", so
/// a test that dials the limit down (`upload_limiter_throttles_and_never_stalls`)
/// would both slow every matrix transfer and be slowed by them. Matrix
/// transfers share this gate; that test takes it exclusively.
pub(crate) static PACE_GATE: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());

pub(super) struct Throttle(PathBuf);
impl Throttle {
    pub(super) fn new(dest: &Path, per_chunk: Duration) -> Self {
        THROTTLED.lock().unwrap().insert(dest.to_path_buf(), per_chunk);
        Throttle(dest.to_path_buf())
    }
}
impl Drop for Throttle { fn drop(&mut self) { THROTTLED.lock().unwrap().remove(&self.0); } }

pub(super) struct Scratch(pub PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        // Read-only fixtures must not survive their test.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fn writable(p: &Path) {
                if let Ok(m) = std::fs::symlink_metadata(p) {
                    if m.file_type().is_symlink() { return; }
                    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(if m.is_dir() { 0o755 } else { 0o644 }));
                    if m.is_dir() { for e in std::fs::read_dir(p).into_iter().flatten().flatten() { writable(&e.path()); } }
                }
            }
            writable(&self.0);
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
pub(super) fn scratch(label: &str) -> Scratch {
    let p = std::env::temp_dir().join(format!("dropbeam-matrix-{label}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&p).unwrap();
    // Canonical so assertions compare /private/var/... with /private/var/... on macOS.
    Scratch(std::fs::canonicalize(&p).unwrap())
}

pub(super) async fn endpoint(accept: bool) -> Endpoint {
    endpoint_with(accept, SecretKey::generate()).await
}
pub(super) async fn endpoint_with(accept: bool, key: SecretKey) -> Endpoint {
    let mut b = Endpoint::builder(presets::Minimal)
        .secret_key(key)
        .path_selector(Arc::new(super::DirectPathSelector))
        .relay_mode(iroh::RelayMode::Disabled)
        .bind_addr("127.0.0.1:0")
        .unwrap();
    if accept {
        b = b.alpns(vec![ALPN.to_vec()]);
    }
    b.bind().await.expect("bind loopback endpoint")
}

/// Dial like the app's retrying senders do: a restarted peer's OLD address can
/// answer "refused" (closing endpoint) before the new one is tried.
pub(super) async fn dial(client: &Endpoint, server: &Endpoint) -> Connection {
    let mut last = None;
    for _ in 0..5 {
        match tokio::time::timeout(Duration::from_secs(10), client.connect(server.addr(), ALPN)).await {
            Ok(Ok(c)) => return c,
            Ok(Err(e)) => last = Some(format!("{e:#}")),
            Err(_) => last = Some("dial timed out".into()),
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("could not dial the receiver: {last:?}")
}

pub(super) fn payload(len: usize, seed: u64) -> Vec<u8> {
    let mut v = vec![0u8; len];
    for (i, b) in v.iter_mut().enumerate() {
        *b = ((i as u64).wrapping_mul(2654435761).wrapping_add(seed) % 251) as u8;
    }
    v
}

/// A distinctive past mtime per seed, so "mtime survived" is a real check and
/// not "both were written in the same second".
pub(super) fn stamp(seed: u64) -> u64 { 1_600_000_000 + seed * 7919 }

/// Write a fixture file (parents created) with deterministic content + mtime.
pub(super) fn put(root: &Path, rel: &str, len: usize, seed: u64) -> PathBuf {
    let p = root.join(rel);
    if let Some(parent) = p.parent() { std::fs::create_dir_all(parent).unwrap(); }
    std::fs::write(&p, payload(len, seed)).unwrap();
    set_mtime_secs(&p, stamp(seed));
    p
}

pub(super) fn sha(path: &Path) -> String {
    let mut f = std::fs::File::open(path).unwrap();
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).unwrap();
    hex::encode(h.finalize())
}

/// Every regular file under `root` (recursive, symlinks NOT followed), as
/// root-relative paths.
pub(super) fn tree(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            // A Location host's own `.dropbeam-*` bookkeeping (mount marker,
            // trash) lives in the share by design; it is not transfer output.
            if e.file_name().to_string_lossy().starts_with(".dropbeam-mount-") { continue; }
            if ft.is_dir() { walk(&p, root, out); } else { out.push(p.strip_prefix(root).unwrap().to_path_buf()); }
        }
    }
    let mut out = vec![];
    walk(root, root, &mut out);
    out.sort();
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Via { Push, Friend, Quick, Location }
pub(super) const ALL: [Via; 4] = [Via::Push, Via::Friend, Via::Quick, Via::Location];

/// What a Location host names a landed upload: the sender's rel verbatim
/// (hidden files included — it is a backup). Every other path runs `receive_rel`.
fn landed_rel(via: Via, sent: &str) -> PathBuf {
    match via {
        Via::Location => PathBuf::from(sent),
        _ => receive_rel(sent),
    }
}

/// The send manifest exactly as the engine builds it for this path.
fn manifest(via: Via, paths: &[PathBuf]) -> (Vec<SendItem>, Vec<String>, u64) {
    gather_items_with(paths, via == Via::Location).unwrap()
}

/// Run one transfer of `paths` into `dest` over `via`, bounded by `deadline`.
pub(super) async fn transfer(via: Via, paths: &[PathBuf], dest: &Path, deadline: Duration) -> Result<u64> {
    let fut = async {
        match via {
            Via::Push => push(paths, dest).await,
            Via::Friend => friend(paths, dest).await,
            Via::Quick => quick(paths, dest).await,
            Via::Location => location(paths, dest).await,
        }
    };
    tokio::time::timeout(deadline, fut).await.map_err(|_| anyhow::anyhow!("{via:?} transfer HUNG past {deadline:?}"))?
}

async fn push(paths: &[PathBuf], dest: &Path) -> Result<u64> {
    let server = endpoint(true).await;
    let client = endpoint(false).await;
    let (srv, target) = (server.clone(), dest.to_path_buf());
    let rx = tokio::spawn(async move {
        let conn = srv.accept().await.unwrap().await?;
        recv_files_negotiated(&conn, &target, &AtomicBool::new(false), &AtomicBool::new(false), |_, _| {}).await
    });
    let conn = dial(&client, &server).await;
    let sent = send_files(&conn, paths, &AtomicBool::new(false), |_, _| {}, "matrix", &AtomicBool::new(false)).await;
    let got = rx.await?;
    client.close().await;
    server.close().await;
    let sent = sent?; // the sender's own outcome is what its user sees
    got?;
    Ok(sent)
}

pub(super) fn chat_link(items: &[SendItem], dirs: &[String], total: u64) -> crate::models::ChatTransferLink {
    serde_json::from_value(serde_json::json!({
        "id": uuid::Uuid::new_v4().to_string(), "attempt": 1,
        "manifest": items.iter().map(|i| serde_json::json!({"name": i.1, "size": i.2})).collect::<Vec<_>>(),
        "directories": dirs, "offset": 0, "total": total, "last": true})).unwrap()
}

/// A receiver that serves the friend protocol arms the app serves for a known
/// friend: `files.stat` (resume skip) and `files` (negotiated receive). Returns
/// the per-push results so a failing push can't hide behind a later success.
pub(super) fn friend_receiver(server: Endpoint, dest: PathBuf) -> tokio::task::JoinHandle<Vec<Result<usize, String>>> {
    friend_receiver_cb(server, dest, Arc::new(|_, _| {}))
}
/// Receiver progress hook: lets a test throttle the receiver (so a sender is
/// provably mid-stream) and break the transfer at a byte position.
pub(super) type Hook = Arc<dyn Fn(u64, u64) + Send + Sync>;
pub(super) fn friend_receiver_cb(server: Endpoint, dest: PathBuf, hook: Hook) -> tokio::task::JoinHandle<Vec<Result<usize, String>>> {
    tokio::spawn(async move {
        let mut results = vec![];
        let Some(incoming) = server.accept().await else { return results };
        let Ok(conn) = incoming.await else { return results };
        loop {
            let Ok((mut send, mut recv)) = conn.accept_bi().await else { break };
            let Ok(header) = read_frame(&mut recv).await else { continue };
            if header["kind"] == "ping" {
                // A Location host checks the capability before a download push.
                let _ = write_frame(&mut send, &serde_json::json!({"kind": "pong", "locations_v": crate::locations::VERSION})).await;
                let _ = send.finish();
                continue;
            }
            if header["kind"] == "files.stat" {
                let reply = friend_stat_reply(&dest, &header).unwrap();
                let _ = write_frame(&mut send, &reply).await;
                let _ = send.finish();
                continue;
            }
            let hook = hook.clone();
            let got = integrity::scope(read_files_negotiated(&conn, &mut send, &mut recv, &header, &dest,
                &AtomicBool::new(false), &AtomicBool::new(false), move |d, t| hook(d, t))).await;
            let _ = send.finish();
            let _ = tokio::time::timeout(Duration::from_secs(5), send.stopped()).await;
            results.push(got.map(|p| p.len()).map_err(|e| format!("{e:#}")));
        }
        results
    })
}

/// The app's friend send exactly as `send_friend_inner` shapes it: one file →
/// a chat-linked single push; several → the split `send_friend_batch`.
pub(super) async fn friend_send(conn: &Connection, server_id: &str, paths: &[PathBuf], cancel: &AtomicBool) -> Result<u64> {
    friend_send_cb(conn, server_id, paths, cancel, |_, _| {}, |_| {}).await
}
pub(super) async fn friend_send_cb(conn: &Connection, server_id: &str, paths: &[PathBuf], cancel: &AtomicBool,
    progress: impl Fn(u64, u64), skipped: impl Fn(usize)) -> Result<u64> {
    let (items, dirs, total) = gather_items(paths)?;
    let link = chat_link(&items, &dirs, total);
    let state = IrohState::default();
    state.learn_progress(server_id, PROGRESS_V);
    let engaged = AtomicBool::new(false);
    let activity = AtomicU64::new(0);
    integrity::scope(async {
        if friend_split(&items) {
            send_friend_batch(conn, &items, &dirs, &link, cancel, "matrix", &engaged, &activity,
                Some(&state), progress, skipped, || Ok(())).await
        } else {
            send_files_linked(conn, paths, cancel, progress, "matrix", &engaged, &activity,
                Some(&state), Some(&link), None).await
        }
    }).await
}

async fn friend(paths: &[PathBuf], dest: &Path) -> Result<u64> {
    let server = endpoint(true).await;
    let client = endpoint(false).await;
    let rx = friend_receiver(server.clone(), dest.to_path_buf());
    let conn = dial(&client, &server).await;
    let sent = friend_send(&conn, &server.id().to_string(), paths, &AtomicBool::new(false)).await;
    conn.close(0u32.into(), b"done");
    let results = rx.await?;
    client.close().await;
    server.close().await;
    // The SENDER's own outcome is what the user sees; report it first.
    let sent = sent?;
    for r in results { r.map_err(|e| anyhow::anyhow!("receiver task: {e}"))?; }
    Ok(sent)
}

async fn quick(paths: &[PathBuf], dest: &Path) -> Result<u64> {
    let server = endpoint(true).await; // the Quick Send SENDER hosts the ticket
    let client = endpoint(false).await;
    let ticket = make_ticket(&server, "matrix-token")?;
    let (addr, token) = parse_ticket(&ticket)?;
    let (srv, staged) = (server.clone(), paths.to_vec());
    let serve = tokio::spawn(async move {
        let conn = srv.accept().await.unwrap().await?;
        let (mut send, mut recv) = conn.accept_bi().await?;
        let req = read_frame(&mut recv).await?;
        anyhow::ensure!(req["kind"] == "pull" && req["token"] == "matrix-token");
        let sent = integrity::scope(serve_pull_verified(&conn, &mut send, &mut recv, &staged,
            req["parallel"].as_bool().unwrap_or(false), &AtomicBool::new(false), |_, _| {})).await?;
        let ack = recv.read_to_end(256).await?;
        anyhow::ensure!(ack == b"ok", "Quick Send sender did not get its receipt");
        Ok(sent)
    });
    let conn = client.connect(addr, ALPN).await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, &serde_json::json!({"kind": "pull", "token": token, "parallel": true, "integrity_v": 1})).await?;
    let header = read_frame(&mut recv).await?;
    let got = read_pull_files_negotiated(&conn, &mut send, &mut recv, &header, dest,
        &AtomicBool::new(false), &AtomicBool::new(false), |_, _| {}).await;
    drop(conn);
    let sent = serve.await?;
    client.close().await;
    server.close().await;
    got?;
    sent
}

/// Upload into a Location hosted at `dest` (headless host = the real
/// `accept_loop` with a location config, exactly what a NAS box runs).
async fn location(paths: &[PathBuf], dest: &Path) -> Result<u64> {
    let config = dest.with_file_name(format!("{}-config", dest.file_name().unwrap().to_string_lossy()));
    std::fs::create_dir_all(&config)?;
    std::fs::create_dir_all(dest)?;
    let host = endpoint(true).await;
    let client = endpoint(false).await;
    let friend = crate::friends::upsert_by_endpoint(&config, &client.id().to_string(), "Matrix");
    crate::locations::save(&config, Some(crate::locations::Location {
        id: "nas".into(), name: "NAS".into(), path: dest.to_string_lossy().into_owned(),
        friend_ids: vec![friend.id], rights: crate::locations::Rights::default(),
        byte_cap: crate::locations::default_byte_cap(), device: None, marker: None, safe_publish: None,
    }), None)?;
    let state = Arc::new(IrohState::default());
    state.location_config.set(config.clone()).unwrap();
    let listener = tokio::spawn(accept_loop(host.clone(), state));
    let conn = client.connect(host.addr(), ALPN).await?;
    let (items, dirs, _) = manifest(Via::Location, paths);
    let mut batch = LocationBatch { items, dirs, next_file: 0, dirs_pending: true };
    let options = LocationSend { target: Some(crate::locations::Target { location_id: "nas".into(), rel_path: "".into() }),
        transfer_id: uuid::Uuid::new_v4().to_string(), snapshot: None, replace_existing: false };
    let sent = batch.send_attempt(&conn, &options, &AtomicBool::new(false), "Matrix", &AtomicBool::new(false),
        &AtomicU64::new(0), None, |_, _| {}, |_| {}, |_| {}, |_| {}, || Ok(())).await;
    conn.close(0u32.into(), b"done");
    listener.abort();
    client.close().await;
    host.close().await;
    // The host keeps its bookkeeping (stages, trash, activity) out of the share.
    let _ = std::fs::remove_dir_all(&config);
    sent
}

/// Byte identity + name + mtime for every manifest item, and nothing extra at
/// `dest` beyond `extra_ok` (files the test put there itself).
pub(super) fn assert_landed(via: Via, paths: &[PathBuf], dest: &Path, extra_ok: &[PathBuf]) {
    let (items, dirs, _) = manifest(via, paths);
    let mut want: Vec<PathBuf> = extra_ok.to_vec();
    for (src, rel, size, mtime) in &items {
        let landed = dest.join(landed_rel(via, rel));
        let meta = std::fs::symlink_metadata(&landed)
            .unwrap_or_else(|e| panic!("{via:?}: {rel:?} did not land at {} ({e}); dest has {:?}", landed.display(), tree(dest)));
        assert!(meta.is_file(), "{via:?}: {rel:?} landed as a non-file");
        assert_eq!(meta.len(), *size, "{via:?}: {rel:?} size");
        assert_eq!(sha(&landed), sha(src), "{via:?}: {rel:?} bytes differ");
        assert_eq!(mtime_secs(&meta), *mtime, "{via:?}: {rel:?} modified-time was not preserved");
        want.push(landed.strip_prefix(dest).unwrap().to_path_buf());
    }
    for d in &dirs {
        let d = dest.join(landed_rel(via, d));
        assert!(d.is_dir(), "{via:?}: empty dir {} was not recreated", d.display());
    }
    want.sort();
    want.dedup();
    assert_eq!(tree(dest), want, "{via:?}: unexpected extra/missing files at the destination");
}

/// Send `paths` over every path and check the landing. `label` names the case.
pub(super) async fn over_all(label: &str, paths: &[PathBuf], deadline: Duration) {
    for via in ALL {
        let rx = scratch(&format!("{label}-{via:?}-rx"));
        let dest = rx.0.join("dest");
        transfer(via, paths, &dest, deadline).await
            .unwrap_or_else(|e| panic!("{label}: {via:?} failed: {e:#}"));
        assert_landed(via, paths, &dest, &[]);
    }
}

const QUICK: Duration = Duration::from_secs(60);

// ── sizes ───────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_tiny_sizes_zero_and_one_byte() {
    let _gate = PACE_GATE.read().await;
    let src = scratch("tiny");
    let paths = vec![put(&src.0, "empty.bin", 0, 1), put(&src.0, "one.bin", 1, 2)];
    over_all("tiny-batch", &paths, QUICK).await;
    // Alone, each is its own single-item push (a different code path).
    over_all("zero-alone", &paths[..1], QUICK).await;
    over_all("one-alone", &paths[1..], QUICK).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_chunk_and_block_boundaries() {
    let _gate = PACE_GATE.read().await;
    let src = scratch("bounds");
    let c = CHUNK;
    let b = integrity::BLOCK as usize;
    let mut paths = vec![];
    for (i, len) in [c - 1, c, c + 1, b - 1, b, b + 1].into_iter().enumerate() {
        paths.push(put(&src.0, &format!("n{i}-{len}.bin"), len, 10 + i as u64));
    }
    over_all("boundaries-batch", &paths, QUICK).await;
    // The 4 MiB friend small-file limit decides batch vs. own push: send the
    // straddling pair alone too.
    over_all("block-straddle", &paths[3..5], QUICK).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_parallel_threshold_boundaries() {
    let _gate = PACE_GATE.read().await;
    let src = scratch("par");
    let m = PARALLEL_MIN as usize;
    // Each alone: under → classic body, at/over → parallel resumable ranges
    // (and a segment split that isn't block-aligned).
    for (i, len) in [m - 1, m, m + 1, 3 * m + 7].into_iter().enumerate() {
        let p = put(&src.0, &format!("p{i}.bin"), len, 30 + i as u64);
        over_all(&format!("parallel-{len}"), &[p], QUICK).await;
    }
}

// ── names ───────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_unicode_and_odd_names() {
    let _gate = PACE_GATE.read().await;
    use unicode_normalization::UnicodeNormalization;
    let src = scratch("names");
    let nfd: String = "Café résumé ñ 한국어.txt".nfd().collect();
    let names = [
        nfd.as_str(), "🚀📦 emoji.bin", "مرحبا بالعالم.txt", "שלום.txt", "  leading spaces.txt",
        "trailing space .txt", "double  space.txt", "tab\there.txt", "new\nline.txt",
        "a:colon.txt", "star*.txt", "q?.txt", "quote\".txt", "lt<gt>.txt", "pipe|.txt", "back\\slash.txt",
        "CON", "NUL.txt", "COM1.tar.gz", "aux", "trailing dot.", "percent %20 & $HOME ~.txt", "#hash;semi'apos.txt",
    ];
    let paths: Vec<_> = names.iter().enumerate().map(|(i, n)| put(&src.0, n, 100 + i, 50 + i as u64)).collect();
    over_all("odd-names", &paths, QUICK).await;
    // The same names inside a sent FOLDER (rel paths with parents).
    let folder = src.0.join("Odd Folder ✓");
    for (i, n) in names.iter().enumerate() { put(&folder, n, 200 + i, 90 + i as u64); }
    over_all("odd-names-folder", &[folder], QUICK).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_long_names_and_deep_paths() {
    let _gate = PACE_GATE.read().await;
    let src = scratch("long");
    // 255 bytes = the APFS/ext4 per-name maximum, ASCII and multibyte.
    let ascii = format!("{}.bin", "L".repeat(251));
    let multi = format!("{}.bin", "é".repeat(125)); // 250 + 4 bytes
    assert_eq!(ascii.len(), 255);
    let mut paths = vec![put(&src.0, &ascii, 4096, 1), put(&src.0, &multi, 4096, 2)];
    // As long a path as this OS lets a file have (macOS PATH_MAX is 1024
    // bytes for the WHOLE absolute path; Linux takes > 1024 relative). A wire
    // path longer than the receiver allows is covered by
    // `matrix_wire_path_longer_than_receiver_allows_lands_flat`.
    let seg = "D".repeat(200);
    let room = if cfg!(target_os = "macos") { 1000 - 2 * src.0.as_os_str().len() } else { 1300 };
    let mut deep_rel = String::from("Long");
    while deep_rel.len() + 201 + 9 < room { deep_rel.push('/'); deep_rel.push_str(&seg); }
    deep_rel.push_str("/leaf.bin");
    put(&src.0, &deep_rel, 8192, 3);
    paths.push(src.0.join("Long"));
    over_all("long", &paths, QUICK).await;
    // 50 levels of nesting with an empty directory at the bottom and midway.
    let mut rel = String::from("Nest");
    for i in 0..50 { rel.push_str(&format!("/lvl{i:02}")); }
    put(&src.0, &format!("{rel}/bottom.bin"), 1000, 4);
    std::fs::create_dir_all(src.0.join(format!("{rel}/empty-at-bottom"))).unwrap();
    std::fs::create_dir_all(src.0.join("Nest/lvl00/lvl01/empty-midway")).unwrap();
    over_all("nest-50", &[src.0.join("Nest")], QUICK).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_dotfiles_junk_and_packages() {
    let _gate = PACE_GATE.read().await;
    let src = scratch("dots");
    let f = src.0.join("Project");
    put(&f, "visible.txt", 10, 1);
    put(&f, ".env", 11, 2);
    put(&f, ".git/config", 12, 3);
    put(&f, ".DS_Store", 13, 4);
    put(&f, "._visible.txt", 14, 5);
    put(&f, "Thumbs.db", 15, 6);
    put(&f, "desktop.ini", 16, 7);
    // macOS packages are directories: sent as trees, structure intact.
    let app = put(&f, "Tool.app/Contents/MacOS/Tool", 5000, 8);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    put(&f, "Tool.app/Contents/Info.plist", 300, 9);
    put(&f, "Lib.photoslibrary/database/Photos.sqlite", 7000, 10);
    std::fs::create_dir_all(f.join("Lib.photoslibrary/resources/empty")).unwrap();
    over_all("dots-folder", &[f.clone()], QUICK).await;
    // An explicitly chosen dotfile is sent (and un-hidden by friend receivers).
    over_all("dot-explicit", &[f.join(".env")], QUICK).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_symlinks_are_never_followed() {
    let _gate = PACE_GATE.read().await;
    #[cfg(unix)]
    {
        let src = scratch("links");
        let outside = src.0.join("outside");
        put(&outside, "secret.txt", 99, 1);
        let f = src.0.join("Shared");
        put(&f, "real.txt", 50, 2);
        std::os::unix::fs::symlink(outside.join("secret.txt"), f.join("file-link")).unwrap();
        std::os::unix::fs::symlink(&outside, f.join("dir-link")).unwrap();
        std::os::unix::fs::symlink(src.0.join("nope"), f.join("dangling")).unwrap();
        std::os::unix::fs::symlink("..", f.join("loop")).unwrap();
        for via in ALL {
            let rx = scratch("links-rx");
            let dest = rx.0.join("dest");
            transfer(via, &[f.clone()], &dest, QUICK).await.unwrap();
            assert_eq!(tree(&dest), vec![PathBuf::from("Shared/real.txt")], "{via:?}: only the real file travels");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_read_only_and_duplicate_names_across_folders() {
    let _gate = PACE_GATE.read().await;
    let src = scratch("ro");
    let ro = put(&src.0, "locked.txt", 4000, 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o444)).unwrap();
    }
    let a = src.0.join("A");
    let b = src.0.join("B");
    put(&a, "same.txt", 10, 2);
    put(&b, "same.txt", 20, 3);
    put(&a, "sub/same.txt", 30, 4);
    over_all("ro-dups", &[ro, a, b], QUICK).await;
}

/// Two DIFFERENT files that arrive under the same landed name (same leaf from
/// two source dirs; a dotfile and its un-hidden twin; names differing only by
/// case — which collide on a case-insensitive receiver like macOS/Windows).
/// Both must survive; neither may silently replace the other.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_colliding_names_keep_both() {
    let _gate = PACE_GATE.read().await;
    let src = scratch("collide");
    let one = put(&src.0.join("one"), "report.pdf", 100, 1);
    let two = put(&src.0.join("two"), "report.pdf", 200, 2);
    let dot = put(&src.0, ".config", 300, 3);
    let plain = put(&src.0, "config", 400, 4);
    let upper = put(&src.0.join("u"), "README.md", 500, 5);
    let lower = put(&src.0.join("l"), "readme.md", 600, 6);
    for via in [Via::Push, Via::Friend, Via::Quick] {
        let rx = scratch("collide-rx");
        let dest = rx.0.join("dest");
        let paths = [one.clone(), two.clone(), dot.clone(), plain.clone(), upper.clone(), lower.clone()];
        transfer(via, &paths, &dest, QUICK).await.unwrap();
        let landed: Vec<String> = tree(&dest).iter().map(|p| sha(&dest.join(p))).collect();
        let mut want: Vec<String> = paths.iter().map(|p| sha(p)).collect();
        let mut got = landed.clone();
        want.sort();
        got.sort();
        assert_eq!(got, want, "{via:?}: every colliding file must land, none overwritten: {:?}", tree(&dest));
    }
}

/// A file already at the destination is never overwritten: different content
/// lands beside it as "name (1).ext"; identical content is recognised and not
/// duplicated. Also: an existing DIRECTORY where a file wants to land, and an
/// existing FILE where a sent folder's directory needs to be.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_existing_destination_is_never_clobbered() {
    let _gate = PACE_GATE.read().await;
    for via in [Via::Push, Via::Friend, Via::Quick] {
        let src = scratch("exist");
        let rx = scratch("exist-rx");
        let dest = rx.0.join("dest");
        let sent = put(&src.0, "report.pdf", 1000, 1);
        let same = put(&src.0, "same.bin", 1000, 2);
        let clash = put(&src.0, "thing.bin", 1000, 3);
        let big = put(&src.0, "big.mov", PARALLEL_MIN as usize + 5, 4);
        let before = put(&dest, "report.pdf", 999, 77);
        put(&dest, "same.bin", 1000, 2);
        std::fs::create_dir_all(dest.join("thing.bin")).unwrap();
        let big_before = put(&dest, "big.mov", 12345, 78);
        let folder = src.0.join("Blocked");
        put(&folder, "inner/a.txt", 10, 5);
        put(&dest, "Blocked", 5, 79); // a FILE where the folder should go
        let old_report = sha(&before);
        let old_big = sha(&big_before);
        transfer(via, &[sent.clone(), same.clone(), clash.clone(), folder.clone()], &dest, QUICK).await
            .unwrap_or_else(|e| panic!("{via:?}: {e:#}"));
        transfer(via, &[big.clone()], &dest, QUICK).await.unwrap_or_else(|e| panic!("{via:?} big: {e:#}"));
        assert_eq!(sha(&dest.join("report.pdf")), old_report, "{via:?}: existing file overwritten");
        assert_eq!(sha(&dest.join("report (1).pdf")), sha(&sent), "{via:?}");
        assert!(!dest.join("same (1).bin").exists(), "{via:?}: identical re-send duplicated");
        assert!(dest.join("thing.bin").is_dir(), "{via:?}: existing directory replaced");
        assert_eq!(sha(&dest.join("thing (1).bin")), sha(&clash), "{via:?}");
        assert_eq!(std::fs::read(dest.join("Blocked")).unwrap(), payload(5, 79), "{via:?}: blocking file clobbered");
        assert_eq!(sha(&dest.join("big.mov")), old_big, "{via:?}: existing big file overwritten");
        assert_eq!(sha(&dest.join("big (1).mov")), sha(&big), "{via:?}");
        // The blocked folder's file still arrives (flattened beside the blocker).
        let found = tree(&dest).into_iter().any(|p| p.file_name() == Some(std::ffi::OsStr::new("a.txt")) && sha(&dest.join(&p)) == sha(&folder.join("inner/a.txt")));
        assert!(found, "{via:?}: file under a blocked folder was lost: {:?}", tree(&dest));
    }
}

/// Colliding with an existing 255-byte name: the "(1)" suffix must still fit
/// the filesystem's name limit — one long name must never fail the transfer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_collision_on_a_max_length_name() {
    let _gate = PACE_GATE.read().await;
    let name = format!("{}.bin", "N".repeat(251));
    for via in [Via::Push, Via::Friend, Via::Quick] {
        let src = scratch("maxcol");
        let rx = scratch("maxcol-rx");
        let dest = rx.0.join("dest");
        let sent = put(&src.0, &name, 500, 1);
        let big = put(&src.0.join("b"), &name, PARALLEL_MIN as usize + 1, 2);
        put(&dest, &name, 400, 3);
        transfer(via, &[sent.clone()], &dest, QUICK).await.unwrap_or_else(|e| panic!("{via:?}: {e:#}"));
        transfer(via, &[big.clone()], &dest, QUICK).await.unwrap_or_else(|e| panic!("{via:?} big: {e:#}"));
        let shas: Vec<String> = tree(&dest).iter().map(|p| sha(&dest.join(p))).collect();
        assert_eq!(shas.len(), 3, "{via:?}: {:?}", tree(&dest));
        assert!(shas.contains(&sha(&sent)) && shas.contains(&sha(&big)), "{via:?}");
        assert!(tree(&dest).iter().all(|p| p.as_os_str().len() <= 255), "{via:?}");
    }
}

// ── scale ───────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_thousands_of_tiny_files() {
    let _gate = PACE_GATE.read().await;
    let src = scratch("many");
    let f = src.0.join("Many");
    for i in 0..1500u64 {
        put(&f, &format!("d{:02}/f{i:04}.txt", i % 37), (i % 97) as usize, 1000 + i);
    }
    std::fs::create_dir_all(f.join("zz-empty")).unwrap();
    let started = Instant::now();
    over_all("many-1500", &[f], Duration::from_secs(240)).await;
    eprintln!("many-1500 over 4 paths: {:?}", started.elapsed());
}

/// A path longer than the RECEIVER's filesystem allows (a Linux sender's
/// 1300-byte rel arriving on macOS, PATH_MAX 1024): the file must still land
/// (flattened into the destination), never fail the batch or vanish. Built by
/// crafting the manifest names, since no single OS here can hold both sides.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_wire_path_longer_than_receiver_allows_lands_flat() {
    let _gate = PACE_GATE.read().await;
    let src = scratch("wirelong");
    let rx = scratch("wirelong-rx");
    let dest = rx.0.join("dest");
    let seg = "W".repeat(250);
    let a = put(&src.0, "a.bin", 3000, 1);
    let b = put(&src.0, "b.bin", 4000, 2);
    let long_a = format!("Deep/{seg}/{seg}/{seg}/{seg}/{seg}/{seg}/a.bin");
    let items: Vec<SendItem> = vec![(a.clone(), long_a.clone(), 3000, stamp(1)), (b.clone(), "Deep/b.bin".into(), 4000, stamp(2))];
    let server = endpoint(true).await;
    let client = endpoint(false).await;
    let receiver = friend_receiver(server.clone(), dest.clone());
    let conn = dial(&client, &server).await;
    let state = IrohState::default();
    state.learn_progress(&server.id().to_string(), PROGRESS_V);
    let link = chat_link(&items, &[], 7000);
    tokio::time::timeout(QUICK, integrity::scope(send_friend_batch(&conn, &items, &[], &link, &AtomicBool::new(false),
        "matrix", &AtomicBool::new(false), &AtomicU64::new(0), Some(&state), |_, _| {}, |_| {}, || Ok(()))))
        .await.expect("hung").expect("a too-long path must not fail the batch");
    conn.close(0u32.into(), b"done");
    for r in receiver.await.unwrap() { r.unwrap(); }
    let landed = tree(&dest);
    let shas: Vec<String> = landed.iter().map(|p| sha(&dest.join(p))).collect();
    assert!(shas.contains(&sha(&a)) && shas.contains(&sha(&b)), "{landed:?}");
    assert_eq!(landed.len(), 2, "{landed:?}");
    client.close().await;
    server.close().await;
}

// ── receive-name mapping (deterministic, both platforms) ───────────────────

#[test]
fn receive_names_are_deterministic_on_every_platform() {
    use unicode_normalization::UnicodeNormalization;
    let win = |raw: &str| receive_parts(raw, true).join("/");
    let unix = |raw: &str| receive_parts(raw, false).join("/");
    // A backslash is part of a NAME, never a separator: one file, not a folder.
    assert_eq!(win("a\\b.txt"), "a-b.txt");
    assert_eq!(unix("a\\b.txt"), "a\\b.txt");
    assert_eq!(win("..\\..\\Windows\\evil.dll"), "-..-Windows-evil.dll");
    assert_eq!(win("C:/Users/x"), "C-/Users/x");
    assert_eq!(unix("/etc/../passwd"), "etc/passwd");
    // Windows-illegal characters, reserved device names, trailing dots/spaces.
    assert_eq!(win("Report 7:3.pdf"), "Report 7-3.pdf");
    assert_eq!(win("a<b>c\"d|e?f*g.txt"), "a-b-c-d-e-f-g.txt");
    assert_eq!(win("CON"), "_CON");
    assert_eq!(win("com1.tar.gz"), "_com1.tar.gz");
    assert_eq!(win("Folder./name ."), "Folder/name");
    assert_eq!(unix("Report 7:3.pdf"), "Report 7:3.pdf");
    // NFD (what macOS hands a sender) lands NFC everywhere.
    let nfd: String = "Café.txt".nfd().collect();
    assert_eq!(unix(&nfd), "Café".nfc().collect::<String>() + ".txt");
    assert_eq!(win(&nfd), unix(&nfd));
    // Over-long names (a Windows sender's 255-UTF-16-unit CJK name is 765
    // bytes of UTF-8) are clamped to 255 bytes, keeping the extension.
    let cjk = format!("{}.docx", "文".repeat(250));
    for landed in [unix(&cjk), win(&cjk)] {
        assert!(landed.len() <= 255 && landed.ends_with(".docx"), "{landed}");
        assert!(landed.starts_with("文文文"));
    }
    // Idempotent: re-deriving a landed rel never changes it (resume matching
    // and the parallel finalizer both re-run the mapping).
    for raw in ["a\\b.txt", "x/.hidden/..y", &cjk, &nfd, "CON/aux.txt", "sp /tail. ", "文/文.txt", "", "..", "/"] {
        for w in [false, true] {
            let once = receive_parts(raw, w).join("/");
            assert_eq!(receive_parts(&once, w).join("/"), once, "{raw:?} windows={w}");
            assert!(once.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && c.len() <= 255));
        }
    }
}

#[test]
fn collision_suffix_always_fits_the_name_limit() {
    let long = format!("{}.bin", "N".repeat(251));
    assert_eq!(fit_name(&long, ""), long);
    let one = fit_name(&long, " (1)");
    assert_eq!(one.len(), 255);
    assert!(one.ends_with(" (1).bin"));
    let multi = format!("{}.txt", "é".repeat(125));
    let fitted = fit_name(&multi, " (12345)");
    assert!(fitted.len() <= 255 && fitted.ends_with(" (12345).txt"));
    assert_eq!(fit_name("photo.jpg", " (2)"), "photo (2).jpg");
    assert_eq!(fit_name("archive.tar.gz", " (1)"), "archive.tar (1).gz");
    assert_eq!(fit_name("README", " (1)"), "README (1)");
    let cands: Vec<_> = receive_candidates(Path::new("/d").join(&long).as_path(), 3).collect();
    assert!(cands.iter().all(|c| c.file_name().unwrap().len() <= 255));
}

// ── interruption, resume, cancel ─────────────────────────────────────────────

fn leftovers(dest: &Path) -> Vec<PathBuf> {
    tree(dest).into_iter().filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(".dropbeam-")).collect()
}

/// How the transfer is broken mid-flight.
#[derive(Clone, Copy, Debug)]
enum Break { NetworkDrop, ReceiverRestart, SenderRestart, Cancel }

/// A big (parallel, resumable) friend send interrupted a third of the way in,
/// then sent again: the second attempt must RESUME (send well under the full
/// size), land one byte-identical file with the sender's mtime, and leave no
/// partial, sidecar or duplicate behind.
async fn interrupted_big_send_resumes(how: Break) {
    let src = scratch("resume");
    let rx = scratch("resume-rx");
    let dest = rx.0.join("dest");
    let total = 4 * PARALLEL_MIN as usize + 12345;
    let file = put(&src.0, "movie.mov", total, 7);
    std::fs::create_dir_all(&dest).unwrap();
    let slow = Throttle::new(&dest, Duration::from_millis(1));
    let (server_key, client_key) = (SecretKey::generate(), SecretKey::generate());
    let server = endpoint_with(true, server_key.clone()).await;
    let client = endpoint_with(false, client_key.clone()).await;
    let cancel = Arc::new(AtomicBool::new(false));
    let fired = Arc::new(AtomicBool::new(false));
    let live: Arc<std::sync::OnceLock<Connection>> = Arc::default();
    // The break is triggered by the RECEIVER once a third has landed, and the
    // receiver is throttled, so the sender is provably still mid-stream.
    let hook: Hook = {
        let (srv, cli, live, cancel, fired) = (server.clone(), client.clone(), live.clone(), cancel.clone(), fired.clone());
        Arc::new(move |done, _| {
            if done > total as u64 * 6 / 10 && !fired.swap(true, Ordering::SeqCst) {
                match how {
                    Break::NetworkDrop => live.get().unwrap().close(9u32.into(), b"wifi gone"),
                    Break::Cancel => cancel.store(true, Ordering::SeqCst),
                    Break::ReceiverRestart => { let s = srv.clone(); tokio::spawn(async move { s.close().await; }); }
                    Break::SenderRestart => { let c = cli.clone(); tokio::spawn(async move { c.close().await; }); }
                }
            }
        })
    };
    let receiver = friend_receiver_cb(server.clone(), dest.clone(), hook);
    let conn = dial(&client, &server).await;
    live.set(conn.clone()).unwrap();
    let first = tokio::time::timeout(QUICK, friend_send(&conn, &server.id().to_string(), &[file.clone()], &cancel))
        .await.expect("the interrupted send must end, not hang");
    assert!(fired.load(Ordering::SeqCst), "{how:?}: the break never fired");
    assert!(first.is_err(), "{how:?}: the interrupted attempt reports failure");
    conn.close(0u32.into(), b"retry");
    let _ = tokio::time::timeout(Duration::from_secs(20), receiver).await.expect("receiver must notice the break");
    assert!(!dest.join("movie.mov").exists(), "{how:?}: a truncated file must never appear at its real name");
    drop(slow);
    // Retry: restarted sides come back with the SAME identity (the fingerprint
    // that finds the partial is keyed by the sender's id).
    let server = if matches!(how, Break::ReceiverRestart) { endpoint_with(true, server_key).await } else { server };
    let client = if matches!(how, Break::SenderRestart) { endpoint_with(false, client_key).await } else { client };
    let receiver = friend_receiver(server.clone(), dest.clone());
    let conn = dial(&client, &server).await;
    tokio::time::timeout(QUICK, friend_send(&conn, &server.id().to_string(), &[file.clone()], &AtomicBool::new(false)))
        .await.expect("resume hung").unwrap_or_else(|e| panic!("{how:?}: resume failed: {e:#}"));
    let resent = conn.stats().udp_tx.bytes;
    conn.close(0u32.into(), b"done");
    for r in receiver.await.unwrap() { r.unwrap(); }
    assert!(resent < (total as u64) * 85 / 100, "{how:?}: the retry re-sent {resent} of {total} bytes instead of resuming");
    assert_landed(Via::Friend, &[file], &dest, &[]);
    assert!(leftovers(&dest).is_empty(), "{how:?}: {:?}", leftovers(&dest));
    client.close().await;
    server.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_resume_after_network_drop() { let _gate = PACE_GATE.read().await; interrupted_big_send_resumes(Break::NetworkDrop).await; }
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_resume_after_receiver_restart() { let _gate = PACE_GATE.read().await; interrupted_big_send_resumes(Break::ReceiverRestart).await; }
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_resume_after_sender_restart() { let _gate = PACE_GATE.read().await; interrupted_big_send_resumes(Break::SenderRestart).await; }
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_resume_after_cancel_or_pause() { let _gate = PACE_GATE.read().await; interrupted_big_send_resumes(Break::Cancel).await; }

/// A folder of small files canceled (or paused) part-way, then re-sent: every
/// file that already landed is recognised (`files.stat`: size + mtime) and
/// skipped, the rest arrive, and nothing is duplicated.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_small_file_folder_resend_skips_what_landed() {
    let _gate = PACE_GATE.read().await;
    let src = scratch("smallres");
    let rx = scratch("smallres-rx");
    let dest = rx.0.join("dest");
    let folder = src.0.join("Album");
    // 3 MiB each (< the 4 MiB small-file limit) → packed into 32 MiB pushes.
    for i in 0..24 { put(&folder, &format!("IMG_{i:04}.jpg"), 3 << 20, 300 + i); }
    let server = endpoint(true).await;
    let client = endpoint(false).await;
    let cancel = AtomicBool::new(false);
    std::fs::create_dir_all(&dest).unwrap();
    let slow = Throttle::new(&dest, Duration::from_micros(300));
    let receiver = friend_receiver(server.clone(), dest.clone());
    let conn = dial(&client, &server).await;
    let total_bytes = 24u64 * (3 << 20);
    let r = tokio::time::timeout(QUICK, friend_send_cb(&conn, &server.id().to_string(), &[folder.clone()], &cancel, |done, _| {
        if done > total_bytes / 2 { cancel.store(true, Ordering::SeqCst); }
    }, |_| {})).await.expect("hung");
    assert!(r.is_err());
    // The receiver unwinds the canceled push on its own schedule: wait for the
    // destination to settle, then count the whole files that made it.
    let mut last = vec![];
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let now = tree(&dest);
        if now == last { break; }
        last = now;
    }
    assert!(leftovers(&dest).is_empty(), "a canceled push left stage litter: {:?}", leftovers(&dest));
    let landed_first = tree(&dest).len();
    assert!(landed_first > 0, "some whole files landed before the cancel");
    drop(slow);
    let skipped = AtomicU64::new(0);
    tokio::time::timeout(QUICK, friend_send_cb(&conn, &server.id().to_string(), &[folder.clone()], &AtomicBool::new(false),
        |_, _| {}, |n| skipped.store(n as u64, Ordering::SeqCst))).await.expect("hung").unwrap();
    conn.close(0u32.into(), b"done");
    let _ = receiver.await.unwrap();
    assert_eq!(skipped.load(Ordering::SeqCst) as usize, landed_first, "every landed file is skipped on resend");
    assert_landed(Via::Friend, &[folder], &dest, &[]);
    client.close().await;
    server.close().await;
}

/// The same big file sent twice at once (double drop): both sends finish, and
/// the destination ends with byte-identical copies only — never a mix of the
/// two streams, never a leftover partial.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_duplicate_concurrent_sends_of_one_file() {
    let _gate = PACE_GATE.read().await;
    let src = scratch("dup");
    let rx = scratch("dup-rx");
    let dest = rx.0.join("dest");
    let file = put(&src.0, "same.mov", 2 * PARALLEL_MIN as usize + 99, 9);
    let small = put(&src.0, "note.txt", 5000, 10);
    let server = endpoint(true).await;
    let client = endpoint(false).await;
    // One receiver task per connection: two parallel connections, like two
    // send cards racing.
    let (srv, target) = (server.clone(), dest.clone());
    let receivers = tokio::spawn(async move {
        let mut hs = vec![];
        for _ in 0..2 {
            let conn = srv.accept().await.unwrap().await.unwrap();
            let target = target.clone();
            hs.push(tokio::spawn(async move {
                let mut n = 0;
                while let Ok((mut send, mut recv)) = conn.accept_bi().await {
                    let header = read_frame(&mut recv).await.unwrap();
                    integrity::scope(read_files_negotiated(&conn, &mut send, &mut recv, &header, &target,
                        &AtomicBool::new(false), &AtomicBool::new(false), |_, _| {})).await.unwrap();
                    let _ = send.finish();
                    let _ = send.stopped().await;
                    n += 1;
                }
                n
            }));
        }
        for h in hs { h.await.unwrap(); }
    });
    let c1 = dial(&client, &server).await;
    let c2 = dial(&client, &server).await;
    let id = server.id().to_string();
    let (big, note, no) = ([file.clone()], [small.clone()], AtomicBool::new(false));
    let (a, b, c, d) = tokio::time::timeout(QUICK, async {
        tokio::join!(
            friend_send(&c1, &id, &big, &no),
            friend_send(&c2, &id, &big, &no),
            friend_send(&c1, &id, &note, &no),
            friend_send(&c2, &id, &note, &no),
        )
    }).await.expect("concurrent sends hung");
    for r in [a, b, c, d] { r.unwrap(); }
    c1.close(0u32.into(), b"done");
    c2.close(0u32.into(), b"done");
    receivers.await.unwrap();
    let landed = tree(&dest);
    assert!(leftovers(&dest).is_empty(), "{landed:?}");
    for p in &landed {
        let want = if p.to_string_lossy().starts_with("same") { sha(&file) } else { sha(&small) };
        assert_eq!(sha(&dest.join(p)), want, "{p:?} is corrupt");
    }
    assert!(landed.iter().any(|p| p == Path::new("same.mov")) && landed.iter().any(|p| p == Path::new("note.txt")));
    client.close().await;
    server.close().await;
}

/// A source that changes after the manifest was built. `mutate` runs once, as
/// soon as the first file's bytes are moving; the second file is the victim.
async fn source_changes_mid_send(label: &str, mutate: fn(&Path), expect_ok: bool) {
    for via in [Via::Push, Via::Friend] {
        let started = Instant::now();
        let src = scratch(label);
        let rx = scratch("change-rx");
        let dest = rx.0.join("dest");
        let first = put(&src.0, "a-first.bin", 32 << 20, 1);
        let victim = put(&src.0, "b-victim.bin", 300_000, 2);
        let server = endpoint(true).await;
        let client = endpoint(false).await;
        // The RECEIVER (throttled) mutates the victim once the first bytes land,
        // while the sender is provably still streaming the 32 MiB first file.
        std::fs::create_dir_all(&dest).unwrap();
        let _slow = Throttle::new(&dest, Duration::from_micros(300));
        let fired = Arc::new(AtomicBool::new(false));
        let hook: Hook = { let (fired, victim) = (fired.clone(), victim.clone()); Arc::new(move |done, _| {
            if done > 0 && !fired.swap(true, Ordering::SeqCst) { mutate(&victim) }
        }) };
        let receiver = if via == Via::Friend { Some(friend_receiver_cb(server.clone(), dest.clone(), hook.clone())) } else { None };
        let (srv, target) = (server.clone(), dest.clone());
        let push_rx = (via == Via::Push).then(|| tokio::spawn(async move {
            let conn = srv.accept().await.unwrap().await.unwrap();
            recv_files_negotiated(&conn, &target, &AtomicBool::new(false), &AtomicBool::new(false), move |d, t| hook(d, t)).await
        }));
        let conn = dial(&client, &server).await;
        let paths = [first.clone(), victim.clone()];
        let result = tokio::time::timeout(QUICK, async {
            if via == Via::Friend { friend_send(&conn, &server.id().to_string(), &paths, &AtomicBool::new(false)).await }
            else { send_files(&conn, &paths, &AtomicBool::new(false), |_, _| {}, "matrix", &AtomicBool::new(false)).await }
        }).await.unwrap_or_else(|_| panic!("{label} {via:?}: the send HUNG"));
        assert!(fired.load(Ordering::SeqCst));
        conn.close(0u32.into(), b"done");
        if let Some(r) = receiver { let _ = tokio::time::timeout(Duration::from_secs(20), r).await.expect("receiver hung"); }
        if let Some(r) = push_rx { let _ = tokio::time::timeout(Duration::from_secs(20), r).await.expect("receiver hung"); }
        assert_eq!(result.is_ok(), expect_ok, "{label} {via:?}: {result:?}");
        assert!(started.elapsed() < Duration::from_secs(30), "{label} {via:?}: took {:?} to settle", started.elapsed());
        if let Err(e) = &result { eprintln!("{label} {via:?}: {e:#}"); }
        assert!(leftovers(&dest).is_empty(), "{label} {via:?}: {:?}", leftovers(&dest));
        // Whatever landed is whole: the untouched first file is exact, and the
        // victim — if it landed at all — carries exactly the advertised bytes.
        for p in tree(&dest) {
            if p == Path::new("a-first.bin") { assert_eq!(sha(&dest.join(&p)), sha(&first)); }
            else { assert_eq!(p, Path::new("b-victim.bin"), "{label} {via:?}: stray {p:?}"); assert_eq!(std::fs::read(dest.join(&p)).unwrap(), payload(300_000, 2)); }
        }
        client.close().await;
        server.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_source_grows_mid_send_sends_the_advertised_bytes() {
    let _gate = PACE_GATE.read().await;
    source_changes_mid_send("grow", |p| {
        use std::io::Write;
        std::fs::OpenOptions::new().append(true).open(p).unwrap().write_all(&[7u8; 5000]).unwrap();
    }, true).await;
}

/// A file written to WHILE its bytes are being read (same inode): the send
/// must fail rather than deliver a silent mix of two versions, and a retry
/// then lands the file exactly as it is on disk now.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_source_edited_during_read_never_lands_a_mix() {
    let _gate = PACE_GATE.read().await;
    for via in [Via::Push, Via::Friend] {
        let src = scratch("editread");
        let rx = scratch("editread-rx");
        let dest = rx.0.join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        let head = put(&src.0, "a-head.bin", 1 << 20, 1);
        // Under the parallel threshold and > the small limit: a classic body.
        let doc = put(&src.0, "b-doc.bin", 12 << 20, 2);
        let slow = Throttle::new(&dest, Duration::from_millis(2));
        let edited = Arc::new(AtomicBool::new(false));
        let watcher = tokio::spawn({ let (dest, doc, edited) = (dest.clone(), doc.clone(), edited.clone()); async move {
            // Once the doc's bytes are landing, rewrite its tail in place.
            loop {
                tokio::time::sleep(Duration::from_millis(10)).await;
                let busy = std::fs::read_dir(&dest).into_iter().flatten().flatten().chain(std::fs::read_dir(dest.join("x")).into_iter().flatten().flatten())
                    .any(|e| e.file_name().to_string_lossy().starts_with(".dropbeam-recv-") && e.metadata().map(|m| m.len() > 1 << 20).unwrap_or(false));
                if busy {
                    use std::io::{Seek, Write};
                    let mut f = std::fs::OpenOptions::new().write(true).open(&doc).unwrap();
                    f.seek(std::io::SeekFrom::Start(11 << 20)).unwrap();
                    f.write_all(&[9u8; 4096]).unwrap();
                    f.sync_all().unwrap();
                    edited.store(true, Ordering::SeqCst);
                    break;
                }
            }
        }});
        let r = transfer(via, &[head.clone(), doc.clone()], &dest, QUICK).await;
        watcher.abort();
        drop(slow);
        assert!(edited.load(Ordering::SeqCst), "{via:?}: the edit never happened mid-read");
        let err = r.expect_err("a file edited mid-read must not land as a mix");
        assert!(format!("{err:#}").contains("changed while sending"), "{via:?}: {err:#}");
        assert!(!dest.join("b-doc.bin").exists(), "{via:?}: a mixed copy landed");
        transfer(via, &[head.clone(), doc.clone()], &dest, QUICK).await.unwrap_or_else(|e| panic!("{via:?} retry: {e:#}"));
        assert_eq!(sha(&dest.join("b-doc.bin")), sha(&doc), "{via:?}");
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_source_shrinks_mid_send_fails_cleanly() {
    let _gate = PACE_GATE.read().await;
    source_changes_mid_send("shrink", |p| { std::fs::OpenOptions::new().write(true).open(p).unwrap().set_len(10).unwrap(); }, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_source_deleted_mid_send_fails_cleanly() {
    let _gate = PACE_GATE.read().await;
    source_changes_mid_send("delete", |p| std::fs::remove_file(p).unwrap(), false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_source_replaced_by_directory_mid_send_fails_cleanly() {
    let _gate = PACE_GATE.read().await;
    source_changes_mid_send("to-dir", |p| { std::fs::remove_file(p).unwrap(); std::fs::create_dir(p).unwrap(); }, false).await;
}

/// The receiver can't write (read-only destination — the portable stand-in for
/// a full disk): the sender gets the receiver's error promptly, nothing is
/// left behind, and once the destination is writable again a retry lands it.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_unwritable_destination_fails_fast_then_retry_lands() {
    let _gate = PACE_GATE.read().await;
    use std::os::unix::fs::PermissionsExt;
    for via in [Via::Push, Via::Friend, Via::Quick] {
        for size in [4000usize, PARALLEL_MIN as usize + 3] {
            let src = scratch("rofs");
            let rx = scratch("rofs-rx");
            let dest = rx.0.join("dest");
            std::fs::create_dir_all(&dest).unwrap();
            let file = put(&src.0, "doc.pdf", size, 3);
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o555)).unwrap();
            let started = Instant::now();
            let err = transfer(via, &[file.clone()], &dest, QUICK).await.expect_err("an unwritable destination must fail");
            assert!(!format!("{err:#}").contains("HUNG"), "{via:?} {size}: {err:#}");
            assert!(started.elapsed() < Duration::from_secs(20), "{via:?} {size}: took {:?}", started.elapsed());
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(tree(&dest).is_empty(), "{via:?}: {:?}", tree(&dest));
            transfer(via, &[file.clone()], &dest, QUICK).await.unwrap_or_else(|e| panic!("{via:?} {size} retry: {e:#}"));
            assert_landed(via, &[file], &dest, &[]);
        }
    }
}

/// A BIG (parallel, resumable) source changed mid-send: shrinking fails the
/// send promptly (never a hang), the receiver keeps no visible file, and a
/// retry of the now-smaller file lands exactly what is on disk.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_big_source_shrinks_mid_send_fails_promptly_then_retry_lands() {
    let _gate = PACE_GATE.read().await;
    for via in [Via::Push, Via::Friend, Via::Quick] {
        let src = scratch("bigshrink");
        let rx = scratch("bigshrink-rx");
        let dest = rx.0.join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        let big = put(&src.0, "clip.mov", 4 * PARALLEL_MIN as usize, 5);
        let slow = Throttle::new(&dest, Duration::from_millis(1));
        let victim = big.clone();
        let fired = Arc::new(AtomicBool::new(false));
        let f2 = fired.clone();
        // Shrink from a side task once the first bytes are landing.
        let watcher = tokio::spawn({ let dest = dest.clone(); async move {
            loop {
                tokio::time::sleep(Duration::from_millis(20)).await;
                let busy = tree(&dest).iter().any(|p| p.to_string_lossy().contains(".dropbeam-"));
                if busy { std::fs::OpenOptions::new().write(true).open(&victim).unwrap().set_len(PARALLEL_MIN).unwrap(); f2.store(true, Ordering::SeqCst); break; }
            }
        }});
        let started = Instant::now();
        let r = transfer(via, &[big.clone()], &dest, QUICK).await;
        watcher.abort();
        drop(slow);
        assert!(fired.load(Ordering::SeqCst), "{via:?}");
        assert!(r.is_err(), "{via:?}: a file that shrank mid-send must not report success");
        assert!(!format!("{:#}", r.as_ref().unwrap_err()).contains("HUNG"), "{via:?}: {r:?}");
        assert!(started.elapsed() < Duration::from_secs(30), "{via:?}: took {:?}", started.elapsed());
        assert!(!dest.join("clip.mov").exists(), "{via:?}: a partial file appeared under its real name");
        transfer(via, &[big.clone()], &dest, QUICK).await.unwrap_or_else(|e| panic!("{via:?} retry: {e:#}"));
        assert_eq!(sha(&dest.join("clip.mov")), sha(&big), "{via:?}");
    }
}

/// Per-file receive bookkeeping cost (run with --ignored --nocapture). Was
/// ~9 ms/file on macOS with F_FULLFSYNC; ~1 ms with fsync(2).
#[test]
#[ignore = "benchmark"]
fn bench_receive_stage_create() {
    let d = scratch("stagebench");
    let t = Instant::now();
    for i in 0..200 {
        let (mut st, _f) = ReceiveStage::create(d.0.join(format!(".dropbeam-recv-{i}.part")), 10, "bench").unwrap();
        st.remove().unwrap();
    }
    eprintln!("200 stage create+remove: {:?} ({:?}/file)", t.elapsed(), t.elapsed() / 200);
}


/// Location DOWNLOAD: the host pins a selection (`download_snapshot`) and
/// pushes it from the pinned root to the requester (the app's
/// `send_location_to_friend` with a snapshot). Everything selected must land
/// byte-identical with the requester's naming rule and the host's mtimes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_location_download_lands_every_selected_file() {
    let _gate = PACE_GATE.read().await;
    use unicode_normalization::UnicodeNormalization;
    let base = scratch("locdl");
    let nas = base.0.join("nas");
    let config = base.0.join("config");
    let dest = base.0.join("dest");
    std::fs::create_dir_all(&config).unwrap();
    let nfd: String = "Résumé 한국.pdf".nfd().collect();
    put(&nas, &nfd, 5000, 1);
    put(&nas, "Photos/2024/IMG 0001.jpg", 300_000, 2);
    put(&nas, "Photos/2024/🚀 launch.mov", PARALLEL_MIN as usize + 77, 3);
    put(&nas, "Photos/a:b.txt", 10, 4);
    std::fs::create_dir_all(nas.join("Photos/empty album")).unwrap();
    put(&nas, "big alone.bin", PARALLEL_MIN as usize * 2 + 1, 5);
    let host = endpoint(false).await; // the host DIALS the requester
    let requester = endpoint(true).await;
    let friend = crate::friends::upsert_by_endpoint(&config, &requester.id().to_string(), "Requester");
    crate::locations::save(&config, Some(crate::locations::Location {
        id: "nas".into(), name: "NAS".into(), path: nas.to_string_lossy().into_owned(),
        friend_ids: vec![friend.id], rights: crate::locations::Rights::default(),
        byte_cap: crate::locations::default_byte_cap(), device: None, marker: None, safe_publish: None,
    }), None).unwrap();
    let receiver = friend_receiver(requester.clone(), dest.clone());
    let conn = dial(&host, &requester).await;
    for selection in [vec![nfd.as_str(), "Photos"], vec!["big alone.bin"]] {
        let snapshot = crate::locations::download_snapshot(&config, &requester.id().to_string(),
            &serde_json::json!({"locations_v": crate::locations::VERSION, "id": "nas", "paths": selection})).unwrap();
        assert!(snapshot.skipped.is_empty(), "{:?}", snapshot.skipped);
        let items = snapshot.source.items.clone();
        let options = LocationSend { target: None, transfer_id: uuid::Uuid::new_v4().to_string(), snapshot: Some(snapshot), replace_existing: false };
        tokio::time::timeout(QUICK, send_files_linked(&conn, &[], &AtomicBool::new(false), |_, _| {}, "Host",
            &AtomicBool::new(false), &AtomicU64::new(0), None, None, Some(&options))).await.expect("download hung").unwrap();
        for (src, rel, size, mtime) in &items {
            let landed = dest.join(receive_rel(rel));
            assert_eq!(std::fs::metadata(&landed).unwrap_or_else(|e| panic!("{rel}: {e} in {:?}", tree(&dest))).len(), *size);
            assert_eq!(sha(&landed), sha(src), "{rel}");
            assert_eq!(mtime_secs(&std::fs::metadata(&landed).unwrap()), *mtime, "{rel} mtime");
        }
    }
    assert!(dest.join("Photos/empty album").is_dir(), "{:?}", tree(&dest));
    assert!(leftovers(&dest).is_empty(), "{:?}", leftovers(&dest));
    assert_eq!(tree(&dest).len(), 5, "{:?}", tree(&dest));
    conn.close(0u32.into(), b"done");
    for r in receiver.await.unwrap() { r.unwrap(); }
    host.close().await;
    requester.close().await;
}

/// Re-sending a file that had to land beside an unrelated same-named file
/// ("notes (1).txt") must recognise its own earlier copy — not mint
/// "notes (2).txt", "(3)"… on every retry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matrix_resend_after_collision_does_not_pile_up_copies() {
    let _gate = PACE_GATE.read().await;
    for via in [Via::Push, Via::Friend, Via::Quick] {
        let src = scratch("recol");
        let rx = scratch("recol-rx");
        let dest = rx.0.join("dest");
        let small = put(&src.0, "notes.txt", 3000, 1);
        let big = put(&src.0, "movie.mov", PARALLEL_MIN as usize + 9, 2);
        let folder = src.0.join("Trip");
        put(&folder, "a.jpg", 1000, 3);
        put(&folder, "b.jpg", 2000, 4);
        put(&dest, "notes.txt", 111, 90);
        put(&dest, "movie.mov", 222, 91);
        put(&dest, "Trip/a.jpg", 333, 92);
        for round in 0..3 {
            for paths in [vec![small.clone()], vec![big.clone()], vec![folder.clone()]] {
                transfer(via, &paths, &dest, QUICK).await.unwrap_or_else(|e| panic!("{via:?} round {round}: {e:#}"));
            }
        }
        let mut names: Vec<String> = tree(&dest).iter().map(|p| p.to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["Trip/a (1).jpg", "Trip/a.jpg", "Trip/b.jpg", "movie (1).mov", "movie.mov", "notes (1).txt", "notes.txt"], "{via:?}");
        assert_eq!(sha(&dest.join("notes (1).txt")), sha(&small));
        assert_eq!(sha(&dest.join("movie (1).mov")), sha(&big));
    }
}

/// A REAL full disk (a tiny HFS+ image mounted in scratch; macOS only, run
/// explicitly): a receive that runs out of space fails promptly on both ends
/// with no visible partial file, keeps nothing half-written under a real name,
/// and once space is freed a retry lands byte-identical.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "mounts a disk image; run with --ignored"]
async fn matrix_disk_full_fails_cleanly_then_retry_lands() {
    let _gate = PACE_GATE.read().await;
    let base = scratch("diskfull");
    let image = base.0.join("tiny.dmg");
    let mount = base.0.join("vol");
    std::fs::create_dir_all(&mount).unwrap();
    let ok = |c: &mut std::process::Command| assert!(c.status().unwrap().success(), "{c:?}");
    ok(std::process::Command::new("hdiutil").args(["create", "-size", "12m", "-fs", "HFS+", "-volname", "dbfull", "-quiet"]).arg(&image));
    ok(std::process::Command::new("hdiutil").args(["attach", "-nobrowse", "-quiet", "-mountpoint"]).arg(&mount).arg(&image));
    struct Detach(PathBuf);
    impl Drop for Detach { fn drop(&mut self) { let _ = std::process::Command::new("hdiutil").args(["detach", "-force", "-quiet"]).arg(&self.0).status(); } }
    let _detach = Detach(mount.clone());
    let src = scratch("diskfull-src");
    // Too big for the ~10 MiB free: classic (under the parallel threshold) and
    // a small batch whose LAST file overflows.
    let big = put(&src.0, "too-big.bin", 14 << 20, 1);
    let batch = vec![put(&src.0, "fits-1.bin", 3 << 20, 2), put(&src.0, "fits-2.bin", 3 << 20, 3), put(&src.0, "overflows.bin", 8 << 20, 4)];
    for via in [Via::Push, Via::Friend, Via::Quick] {
        for paths in [vec![big.clone()], batch.clone()] {
            let dest = mount.join(format!("{via:?}"));
            std::fs::create_dir_all(&dest).unwrap();
            let started = Instant::now();
            let err = transfer(via, &paths, &dest, QUICK).await.expect_err("a full disk must fail the receive");
            assert!(!format!("{err:#}").contains("HUNG") && started.elapsed() < Duration::from_secs(30), "{via:?}: {err:#} after {:?}", started.elapsed());
            eprintln!("{via:?} {}: {err:#}", paths.len());
            if via == Via::Friend { assert!(format!("{err:#}").contains("their disk is full"), "{err:#}"); }
            for p in tree(&dest) {
                let name = p.file_name().unwrap().to_string_lossy().into_owned();
                assert!(!name.starts_with(".dropbeam-recv-"), "{via:?}: stage litter {p:?}");
                if !name.starts_with(".dropbeam-") {
                    // Anything visible is a COMPLETE file.
                    let src = paths.iter().find(|s| s.file_name().unwrap().to_string_lossy() == name).unwrap();
                    assert_eq!(sha(&dest.join(&p)), sha(src), "{via:?}: a truncated {name} is visible");
                }
            }
            let _ = std::fs::remove_dir_all(&dest);
        }
    }
    // Space freed: the same send now lands.
    let dest = mount.join("after");
    transfer(Via::Friend, &batch[..2], &dest, QUICK).await.unwrap();
    assert_landed(Via::Friend, &batch[..2], &dest, &[]);
}

#[test]
fn disk_full_is_recognised_through_context() {
    let raw = anyhow::Error::from(std::io::Error::from_raw_os_error(if cfg!(windows) { 112 } else { 28 })).context("writing stage");
    assert!(is_disk_full(&raw));
    assert!(!is_disk_full(&anyhow::anyhow!("permission denied")));
}
