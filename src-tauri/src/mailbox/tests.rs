//! In-process LOOPBACK tests for the Transfer Server: three real iroh
//! endpoints (sender A, server S, recipient B) on 127.0.0.1 with relay and
//! discovery disabled, each with its own config dir and the REAL accept loop,
//! so every `mailbox.*` stream goes through `serve_stream` exactly as in the app.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::client::{self, DepositError, DepositFile, UsableServer};
use super::{keys, seal, server};
use crate::iroh_net::{self, IrohState};

struct Node {
    ep: iroh::Endpoint,
    state: Arc<IrohState>,
    config: PathBuf,
    listener: tokio::task::JoinHandle<()>,
}

impl Node {
    fn eid(&self) -> String {
        self.ep.id().to_string()
    }
}

struct World {
    base: PathBuf,
    a: Node,
    s: Node,
    b: Node,
    /// A's thread id for B.
    b_for_a: String,
}

impl Drop for World {
    fn drop(&mut self) {
        for n in [&self.a, &self.s, &self.b] {
            n.listener.abort();
        }
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

async fn node(base: &Path, name: &str) -> Node {
    let config = base.join(name);
    std::fs::create_dir_all(&config).unwrap();
    let ep = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .secret_key(iroh::SecretKey::generate())
        .alpns(vec![iroh_net::ALPN.to_vec()])
        .relay_mode(iroh::RelayMode::Disabled)
        .bind_addr("127.0.0.1:0").unwrap()
        .bind().await.unwrap();
    iroh_net::remember_addrs_for_tests(&ep.addr());
    let state = Arc::new(IrohState::default());
    state.location_config.set(config.clone()).unwrap();
    let _ = state.endpoint.set(ep.clone());
    let listener = tokio::spawn(iroh_net::accept_loop(ep.clone(), state.clone()));
    Node { ep, state, config, listener }
}

fn introduce(from: &Node, to: &Node) {
    // `to` learns `from`'s signed mailbox key + advertised servers, as a hello would.
    let fields = super::hello_fields(&from.config, from.ep.secret_key(), &to.eid());
    keys::learn(&to.config, &from.eid(), &fields);
}

/// A, S and B are mutual friends; S is a Transfer Server open to all friends;
/// A uses S ("Use it") and knows B's mailbox key.
async fn world(tag: &str) -> World {
    let base = std::env::temp_dir().join(format!("dropbeam-mbx-{tag}-{}", uuid::Uuid::new_v4()));
    let (a, s, b) = (node(&base, "a").await, node(&base, "s").await, node(&base, "b").await);
    let b_for_a = crate::friends::upsert_by_endpoint(&a.config, &b.eid(), "Bea").id;
    crate::friends::upsert_by_endpoint(&b.config, &a.eid(), "Ash");
    crate::friends::upsert_by_endpoint(&s.config, &a.eid(), "Ash");
    crate::friends::upsert_by_endpoint(&s.config, &b.eid(), "Bea");
    crate::friends::upsert_by_endpoint(&a.config, &s.eid(), "Box");
    crate::friends::upsert_by_endpoint(&b.config, &s.eid(), "Box");
    let mut c = server::ServerConfig { enabled: true, name: "Linux Box".into(), cap_bytes: 1 << 30, min_free: 1, ..Default::default() };
    server::init_root(&s.config, &mut c).unwrap();
    server::save_config(&s.config, &c).unwrap();
    introduce(&b, &a);
    // A learns what S grants it (from S's hello) and opts in.
    let grant = server::grant_for(&s.config, &a.eid());
    assert!(grant.is_some(), "a friend of an all-friends server is a member");
    client::learn_grant(&a.config, &s.eid(), grant.as_ref(), false);
    client::set_prefs(&a.config, &s.eid(), Some(true), None, Some("seen")).unwrap();
    // B hears about S from S's hello too (a member; it hasn't opted in to anything).
    client::learn_grant(&b.config, &s.eid(), server::grant_for(&s.config, &b.eid()).as_ref(), false);
    World { base, a, s, b, b_for_a }
}

fn chat_frame(id: &str, text: &str) -> Value {
    json!({"kind": "chat", "v": 2, "friendId": "x", "fromName": "Ash", "id": id, "ts": 1_700_000_000_000u64, "seq": 1, "msgKind": "text", "text": text})
}

fn server_items(w: &World) -> usize {
    server::status(&w.s.config).items
}

async fn fetch(w: &World) -> usize {
    tokio::time::timeout(Duration::from_secs(30), client::fetch_from(&w.b.state, &w.b.config, &w.s.eid())).await.unwrap().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat_is_held_then_delivered_once_with_receipt() {
    let w = world("chat").await;
    let msg = uuid::Uuid::new_v4().to_string();
    let held = client::deposit_chat(&w.a.state, &w.a.config, &w.b_for_a, "chat", &chat_frame(&msg, "running 10 min late"), Some(&msg)).await.unwrap();
    assert_eq!(held.name, "Linux Box");
    assert_eq!(server_items(&w), 1);
    // The server's copy is ciphertext: the text never appears on its disk.
    let root = server::root(&w.s.config, &server::load_config(&w.s.config)).unwrap();
    let mut on_disk = Vec::new();
    for e in walk(&root) {
        on_disk.extend(std::fs::read(e).unwrap_or_default());
    }
    assert!(!String::from_utf8_lossy(&on_disk).contains("running 10 min late"));

    assert_eq!(fetch(&w).await, 1);
    let b_thread = crate::friends::chat_sender(&w.b.config, &w.a.eid()).unwrap().id;
    let got = crate::chat::messages(&w.b.config, &b_thread);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].text, "running 10 min late");
    assert_eq!(got[0].id, msg);
    assert_eq!(got[0].via.as_deref(), Some("Linux Box"));
    assert_eq!(server_items(&w), 0, "delivered items are deleted from the server");
    // A fetch again finds nothing; the same message arriving directly dedupes.
    assert_eq!(fetch(&w).await, 0);
    iroh_net::apply_incoming_chat(&w.b.state, &w.b.config, &w.a.eid(), &chat_frame(&msg, "running 10 min late"), None, None);
    assert_eq!(crate::chat::messages(&w.b.config, &b_thread).len(), 1, "direct + server copies never duplicate");
    // A learns it was delivered.
    let receipts = client::refresh_status(&w.a.state, &w.a.config).await;
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].state, "delivered");
    assert_eq!(receipts[0].sent.msg_id.as_deref(), Some(msg.as_str()));
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() { out.extend(walk(&p)); } else { out.push(p); }
    }
    out
}

fn write(path: &Path, len: usize, seed: u8) -> Vec<u8> {
    let data: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, &data).unwrap();
    data
}

fn deposit_files_of(paths: &[(PathBuf, &str)]) -> Vec<DepositFile> {
    paths.iter().map(|(p, name)| {
        let m = std::fs::metadata(p).unwrap();
        DepositFile { path: p.clone(), name: (*name).into(), size: m.len(), mtime: iroh_net::mtime_secs(&m) }
    }).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn files_roundtrip_multi_segment_with_empty_file_and_folder() {
    let w = world("files").await;
    let src = w.base.join("src");
    let big = write(&src.join("Trip/clip.mov"), (seal::SEG as usize) * 2 + 12345, 7);
    let small = write(&src.join("Trip/note.txt"), 11, 9);
    write(&src.join("Trip/empty.bin"), 0, 0);
    let files = deposit_files_of(&[(src.join("Trip/clip.mov"), "Trip/clip.mov"), (src.join("Trip/empty.bin"), "Trip/empty.bin"), (src.join("Trip/note.txt"), "Trip/note.txt")]);
    let xfer = uuid::Uuid::new_v4().to_string();
    let seen = Arc::new(AtomicU64::new(0));
    let s2 = seen.clone();
    let progress = move |d: u64, _t: u64| { s2.fetch_max(d, Ordering::SeqCst); };
    let held = client::deposit_files(&w.a.state, &w.a.config, &w.b_for_a, &xfer, &xfer, &files, &["Trip/Sub".into()], &["Trip".into()],
        &progress, &|_| {}, &AtomicBool::new(false), None).await.unwrap();
    assert_eq!(held.name, "Linux Box");
    assert_eq!(seen.load(Ordering::SeqCst), big.len() as u64 + 11);
    assert_eq!(fetch(&w).await, 1);
    let dl = w.b.config.join("Downloads");
    assert_eq!(std::fs::read(dl.join("Trip/clip.mov")).unwrap(), big);
    assert_eq!(std::fs::read(dl.join("Trip/note.txt")).unwrap(), small);
    assert_eq!(std::fs::metadata(dl.join("Trip/empty.bin")).unwrap().len(), 0);
    assert!(dl.join("Trip/Sub").is_dir());
    assert!(!walk(&dl).iter().any(|p| p.to_string_lossy().contains(".dropbeam-mbx-")), "no staging leftovers");
    // A chat card exists on B's side, linked to the transfer.
    let b_thread = crate::friends::chat_sender(&w.b.config, &w.a.eid()).unwrap().id;
    let card = crate::chat::received_file(&w.b.config, &b_thread, &iroh_net::incoming_chat_id(&w.a.eid(), &xfer)).unwrap();
    assert_eq!(card.files, vec!["Trip".to_string()]);
    assert!(card.path.is_some());
    assert_eq!(server_items(&w), 0);
    assert!(std::fs::read_dir(w.b.config.join("mailbox-in")).unwrap().next().is_none(), "ciphertext partial removed");
    let r = client::refresh_status(&w.a.state, &w.a.config).await;
    assert_eq!((r.len(), r[0].state.as_str(), r[0].sent.kind.as_str()), (1, "delivered", "file"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupted_upload_resumes_and_restart_keeps_items() {
    let w = world("resume").await;
    let src = w.base.join("src/big.bin");
    let data = write(&src, (seal::SEG as usize) * 5 + 99, 3);
    let files = deposit_files_of(&[(src.clone(), "big.bin")]);
    let xfer = uuid::Uuid::new_v4().to_string();
    // Cancel partway through (a pause): the deposit stops, the partial stays.
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    let stop_at = seal::SEG * 2;
    let progress = move |d: u64, _t: u64| { if d >= stop_at { c2.store(true, Ordering::SeqCst); } };
    let r = client::deposit_files(&w.a.state, &w.a.config, &w.b_for_a, &xfer, &xfer, &files, &[], &["big.bin".into()],
        &progress, &|_| {}, &cancel, None).await;
    assert_eq!(r.unwrap_err(), DepositError::Canceled);
    // Server restarts (cache dropped): the partial upload is still there.
    tokio::time::sleep(Duration::from_millis(300)).await;
    server::unload(&w.s.config);
    assert_eq!(server_items(&w), 1);
    // Resume: the second run starts past what the server already has.
    let first = Arc::new(AtomicU64::new(u64::MAX));
    let f2 = first.clone();
    let progress = move |d: u64, _t: u64| { let _ = f2.compare_exchange(u64::MAX, d, Ordering::SeqCst, Ordering::SeqCst); };
    client::deposit_files(&w.a.state, &w.a.config, &w.b_for_a, &xfer, &xfer, &files, &[], &["big.bin".into()],
        &progress, &|_| {}, &AtomicBool::new(false), None).await.unwrap();
    assert!(first.load(Ordering::SeqCst) > seal::SEG, "resumed instead of starting over (first progress {})", first.load(Ordering::SeqCst));
    // Restart again before delivery: held items survive.
    server::unload(&w.s.config);
    assert_eq!(server_items(&w), 1);
    assert_eq!(fetch(&w).await, 1);
    assert_eq!(std::fs::read(w.b.config.join("Downloads/big.bin")).unwrap(), data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quotas_pause_and_limits_refuse_cleanly() {
    let w = world("quota").await;
    let msg = || uuid::Uuid::new_v4().to_string();
    // Paused.
    let mut c = server::load_config(&w.s.config);
    c.paused = true;
    server::save_config(&w.s.config, &c).unwrap();
    let m = msg();
    let e = client::deposit_chat(&w.a.state, &w.a.config, &w.b_for_a, "chat", &chat_frame(&m, "hi"), Some(&m)).await.unwrap_err();
    assert!(matches!(&e, DepositError::Refused { reason, .. } if reason == "paused"), "{e:?}");
    // Full: a cap smaller than the file.
    c.paused = false;
    c.cap_bytes = 4096;
    server::save_config(&w.s.config, &c).unwrap();
    let src = w.base.join("src/f.bin");
    write(&src, 100_000, 1);
    let files = deposit_files_of(&[(src.clone(), "f.bin")]);
    let x = msg();
    let e = client::deposit_files(&w.a.state, &w.a.config, &w.b_for_a, &x, &x, &files, &[], &["f.bin".into()], &|_, _| {}, &|_| {}, &AtomicBool::new(false), None).await.unwrap_err();
    assert!(matches!(&e, DepositError::Refused { reason, .. } if reason == "full"), "{e:?}");
    // Per-person share: a non-owner may use at most a quarter of the space.
    c.cap_bytes = 300_000;
    server::save_config(&w.s.config, &c).unwrap();
    let x = msg();
    let e = client::deposit_files(&w.a.state, &w.a.config, &w.b_for_a, &x, &x, &files, &[], &["f.bin".into()], &|_, _| {}, &|_| {}, &AtomicBool::new(false), None).await.unwrap_err();
    assert!(matches!(&e, DepositError::Refused { reason, .. } if reason == "user_quota"), "{e:?}");
    // Item size limit.
    c.cap_bytes = 1 << 30;
    c.item_max = 50_000;
    server::save_config(&w.s.config, &c).unwrap();
    let x = msg();
    let e = client::deposit_files(&w.a.state, &w.a.config, &w.b_for_a, &x, &x, &files, &[], &["f.bin".into()], &|_, _| {}, &|_| {}, &AtomicBool::new(false), None).await.unwrap_err();
    assert!(matches!(&e, DepositError::Refused { reason, .. } if reason == "too_big"), "{e:?}");
    assert_eq!(server_items(&w), 0, "refused deposits leave nothing behind");
    // Free-space floor: pretend the disk must keep more free than it has.
    c.item_max = 1 << 30;
    c.min_free = u64::MAX / 4;
    server::save_config(&w.s.config, &c).unwrap();
    let m = msg();
    let e = client::deposit_chat(&w.a.state, &w.a.config, &w.b_for_a, "chat", &chat_frame(&m, "hi"), Some(&m)).await.unwrap_err();
    assert!(matches!(&e, DepositError::Refused { reason, .. } if reason == "full"), "{e:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expiry_reports_back_to_the_sender() {
    let w = world("expiry").await;
    let m = uuid::Uuid::new_v4().to_string();
    client::deposit_chat(&w.a.state, &w.a.config, &w.b_for_a, "chat", &chat_frame(&m, "old news"), Some(&m)).await.unwrap();
    // Nothing is overdue yet.
    assert_eq!(server::gc(&w.s.config), 0);
    server::age_all_for_tests(&w.s.config, 31 * server::DAY_MS);
    assert_eq!(server::gc(&w.s.config), 1);
    assert_eq!(server_items(&w), 0);
    assert_eq!(fetch(&w).await, 0, "expired items are never delivered");
    let r = client::refresh_status(&w.a.state, &w.a.config).await;
    assert_eq!((r.len(), r[0].state.as_str()), (1, "expired"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn access_control_and_forgery_are_refused() {
    let w = world("auth").await;
    // A stranger (not S's friend) can't deposit, and gets nothing from a fetch.
    let stranger = node(&w.base, "x").await;
    introduce(&w.b, &stranger);
    crate::friends::upsert_by_endpoint(&stranger.config, &w.b.eid(), "Bea");
    client::set_server_for_tests(&stranger.config, UsableServer { eid: w.s.eid(), name: "Linux Box".into(), member: true, use_it: true, ..Default::default() });
    let b_for_x = crate::friends::upsert_by_endpoint(&stranger.config, &w.b.eid(), "Bea").id;
    let m = uuid::Uuid::new_v4().to_string();
    let e = client::deposit_chat(&stranger.state, &stranger.config, &b_for_x, "chat", &chat_frame(&m, "spam"), Some(&m)).await.unwrap_err();
    assert!(matches!(&e, DepositError::Refused { reason, .. } if reason == "denied"), "{e:?}");

    // A member may only leave things for other members unless "send through".
    let outsider = iroh::SecretKey::generate();
    let outsider_x: [u8; 32] = rand::random();
    let o_eid = outsider.public().to_string();
    let o_thread = crate::friends::upsert_by_endpoint(&w.a.config, &o_eid, "Olly").id;
    keys::learn(&w.a.config, &o_eid, &json!({"key": seal::b64(&seal::x25519_public(&outsider_x)), "sig": seal::sign_mailbox_key(&outsider, &seal::x25519_public(&outsider_x))}));
    let e = client::deposit_chat(&w.a.state, &w.a.config, &o_thread, "chat", &chat_frame(&m, "hi"), Some(&m)).await.unwrap_err();
    assert!(matches!(&e, DepositError::Refused { reason, .. } if reason == "recipient"), "{e:?}");
    let mut c = server::load_config(&w.s.config);
    let a_person = crate::friends::chat_sender(&w.s.config, &w.a.eid()).unwrap().id;
    c.through.push(a_person.clone());
    server::save_config(&w.s.config, &c).unwrap();
    client::learn_grant(&w.a.config, &w.s.eid(), server::grant_for(&w.s.config, &w.a.eid()).as_ref(), false);
    client::deposit_chat(&w.a.state, &w.a.config, &o_thread, "chat", &chat_frame(&m, "hi"), Some(&m)).await.unwrap();

    // Held for B: nobody else can list, download, ack or cancel it.
    let m2 = uuid::Uuid::new_v4().to_string();
    let held = client::deposit_chat(&w.a.state, &w.a.config, &w.b_for_a, "chat", &chat_frame(&m2, "for B only"), Some(&m2)).await.unwrap();
    let conn = w.a.ep.connect(iroh_net::dial_addr(w.s.ep.id()), iroh_net::ALPN).await.unwrap();
    let listed = rpc(&conn, json!({"kind": "mailbox.fetch"})).await;
    assert!(listed["items"].as_array().unwrap().iter().all(|i| i["item_id"] != held.item_id.as_str()));
    assert_eq!(rpc(&conn, json!({"kind": "mailbox.get", "item_id": held.item_id, "have": 0})).await["reason"], "gone");
    let _ = rpc(&conn, json!({"kind": "mailbox.ack", "item_id": held.item_id, "ok": true})).await;
    let xconn = stranger.ep.connect(iroh_net::dial_addr(w.s.ep.id()), iroh_net::ALPN).await.unwrap();
    assert_eq!(rpc(&xconn, json!({"kind": "mailbox.cancel", "item_id": held.item_id})).await["ok"], false);
    assert_eq!(server_items(&w), 2, "the through item and B's item are both still held");

    // A header "from" someone other than the depositor is refused outright.
    let forged_by = iroh::SecretKey::generate();
    let (env, _) = seal::seal(&forged_by, &uuid::Uuid::new_v4().to_string(), "chat", 1,
        &keys::recipients(&w.a.config, &[w.b.eid()]), b"{}", 0).unwrap();
    let reply = rpc(&conn, json!({"kind": "mailbox.deposit", "header": env})).await;
    assert_eq!(reply["reason"], "invalid");

    // Removing A from the server deletes what A left for others.
    server::remove_person(&w.s.config, &a_person);
    let mut c = server::load_config(&w.s.config);
    c.denied.push(a_person);
    server::save_config(&w.s.config, &c).unwrap();
    assert_eq!(server_items(&w), 0);
    assert!(server::grant_for(&w.s.config, &w.a.eid()).is_none(), "hello stops advertising access");
    let change = client::learn_grant(&w.a.config, &w.s.eid(), None, false);
    assert!(change.revoked);
    let e = client::deposit_chat(&w.a.state, &w.a.config, &w.b_for_a, "chat", &chat_frame(&m2, "again"), Some(&m2)).await.unwrap_err();
    assert_eq!(e, DepositError::NoRoute, "a revoked server is no longer a route");
    stranger.listener.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tampered_or_unknown_sender_items_are_refused_by_the_recipient() {
    let w = world("tamper").await;
    // B doesn't know this sender: the item is refused (and deleted), never shown.
    crate::friends::remove(&w.b.config, &crate::friends::chat_sender(&w.b.config, &w.a.eid()).unwrap().id).unwrap();
    let m = uuid::Uuid::new_v4().to_string();
    client::deposit_chat(&w.a.state, &w.a.config, &w.b_for_a, "chat", &chat_frame(&m, "who dis"), Some(&m)).await.unwrap();
    assert_eq!(fetch(&w).await, 0);
    assert_eq!(server_items(&w), 0, "refused items are removed");
    let r = client::refresh_status(&w.a.state, &w.a.config).await;
    assert_eq!(r[0].state, "rejected");
    // A header whose ciphertext was altered on the server fails verification.
    crate::friends::upsert_by_endpoint(&w.b.config, &w.a.eid(), "Ash");
    let m = uuid::Uuid::new_v4().to_string();
    client::deposit_chat(&w.a.state, &w.a.config, &w.b_for_a, "chat", &chat_frame(&m, "original"), Some(&m)).await.unwrap();
    let root = server::root(&w.s.config, &server::load_config(&w.s.config)).unwrap();
    let header_path = walk(&root).into_iter().find(|p| p.ends_with("header.json")).unwrap();
    let mut h: Value = serde_json::from_slice(&std::fs::read(&header_path).unwrap()).unwrap();
    h["created_ms"] = json!(h["created_ms"].as_u64().unwrap() + 1);
    std::fs::write(&header_path, serde_json::to_vec(&h).unwrap()).unwrap();
    assert_eq!(fetch(&w).await, 0);
    let b_thread = crate::friends::chat_sender(&w.b.config, &w.a.eid()).unwrap().id;
    assert!(crate::chat::messages(&w.b.config, &b_thread).iter().all(|m| m.text != "original"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn corrupt_items_are_dropped_on_restart() {
    let w = world("corrupt").await;
    let m = uuid::Uuid::new_v4().to_string();
    client::deposit_chat(&w.a.state, &w.a.config, &w.b_for_a, "chat", &chat_frame(&m, "x"), Some(&m)).await.unwrap();
    let root = server::root(&w.s.config, &server::load_config(&w.s.config)).unwrap();
    let item = walk(&root).into_iter().find(|p| p.ends_with("item.json")).unwrap();
    std::fs::write(&item, b"{torn").unwrap();
    server::unload(&w.s.config);
    assert_eq!(server_items(&w), 0);
    assert!(!item.parent().unwrap().exists(), "unreadable items are removed, not left to rot");
    // An unmounted/replaced storage folder fails closed.
    std::fs::remove_file(root.join(".dropbeam-server-marker")).unwrap();
    server::unload(&w.s.config);
    let m = uuid::Uuid::new_v4().to_string();
    let e = client::deposit_chat(&w.a.state, &w.a.config, &w.b_for_a, "chat", &chat_frame(&m, "x"), Some(&m)).await.unwrap_err();
    assert!(matches!(&e, DepositError::Refused { reason, .. } if reason == "storage"), "{e:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recipient_inbox_server_is_preferred_and_ops_follow_messages() {
    let w = world("route").await;
    // B says "hold my messages on S" — A routes there first, even with its own
    // (hypothetical) second server listed.
    let mut b_srv = UsableServer { eid: w.s.eid(), name: "Linux Box".into(), member: true, hold_for_me: true, ..Default::default() };
    b_srv.offer = "seen".into();
    client::set_server_for_tests(&w.b.config, b_srv);
    introduce(&w.b, &w.a);
    let other = iroh::SecretKey::generate().public().to_string();
    client::set_server_for_tests(&w.a.config, UsableServer { eid: other.clone(), name: "Other".into(), own: true, member: true, use_it: true, learned_ms: 0, ..Default::default() });
    let routes = client::routes(&w.a.config, &w.a.eid(), &[w.b.eid()]);
    assert_eq!(routes[0].server, w.s.eid(), "the recipient's inbox server comes first");
    // A message, then an edit to it, both held: delivered in order, the edit applies.
    let m = uuid::Uuid::new_v4().to_string();
    client::deposit_chat(&w.a.state, &w.a.config, &w.b_for_a, "chat", &chat_frame(&m, "helo"), Some(&m)).await.unwrap();
    let edit = json!({"kind": "chat", "v": 2, "msgKind": "edit", "friendId": "x", "fromName": "Ash", "targetId": m, "text": "hello"});
    client::deposit_chat(&w.a.state, &w.a.config, &w.b_for_a, "op", &edit, None).await.unwrap();
    assert_eq!(fetch(&w).await, 2);
    let b_thread = crate::friends::chat_sender(&w.b.config, &w.a.eid()).unwrap().id;
    let got = crate::chat::messages(&w.b.config, &b_thread);
    assert_eq!((got.len(), got[0].text.as_str(), got[0].edited), (1, "hello", true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delivery_poke_reaches_the_recipient() {
    let w = world("notify").await;
    let m = uuid::Uuid::new_v4().to_string();
    client::deposit_chat(&w.a.state, &w.a.config, &w.b_for_a, "chat", &chat_frame(&m, "poke"), Some(&m)).await.unwrap();
    let pending = server::pending_recipients(&w.s.config);
    assert_eq!(pending, vec![(w.b.eid(), 1)]);
    let conn = w.s.ep.connect(iroh_net::dial_addr(w.b.ep.id()), iroh_net::ALPN).await.unwrap();
    assert_eq!(rpc(&conn, json!({"kind": "mailbox.notify", "v": 1, "count": 1})).await["ok"], true);
    // A stranger can't make B dial out and pull.
    let x = node(&w.base, "x").await;
    let xconn = x.ep.connect(iroh_net::dial_addr(w.b.ep.id()), iroh_net::ALPN).await.unwrap();
    assert_eq!(rpc(&xconn, json!({"kind": "mailbox.notify", "v": 1, "count": 1})).await["ok"], false);
    x.listener.abort();
}

async fn rpc(conn: &iroh::endpoint::Connection, req: Value) -> Value {
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    iroh_net::write_frame(&mut send, &req).await.unwrap();
    send.finish().unwrap();
    tokio::time::timeout(Duration::from_secs(10), iroh_net::read_frame_cap(&mut recv, 1 << 20)).await.unwrap().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ask_first_friends_files_wait_for_a_yes() {
    let w = world("ask").await;
    let a_on_b = crate::friends::chat_sender(&w.b.config, &w.a.eid()).unwrap();
    crate::friends::set_auto_accept(&w.b.config, &a_on_b.id, false).unwrap();
    let src = w.base.join("src/pic.jpg");
    let data = write(&src, 5000, 4);
    let xfer = uuid::Uuid::new_v4().to_string();
    client::deposit_files(&w.a.state, &w.a.config, &w.b_for_a, &xfer, &xfer, &deposit_files_of(&[(src, "pic.jpg")]), &[], &["pic.jpg".into()],
        &|_, _| {}, &|_| {}, &AtomicBool::new(false), None).await.unwrap();
    assert_eq!(fetch(&w).await, 0, "nothing lands without a yes");
    let pending = client::pending_files(&w.b.config);
    assert_eq!(pending.len(), 1);
    assert_eq!(server_items(&w), 1, "it keeps waiting on the server");
    assert!(!w.b.config.join("Downloads/pic.jpg").exists());
    client::decide_file(&w.b.config, &pending[0].link_id, true);
    assert_eq!(fetch(&w).await, 1);
    assert_eq!(std::fs::read(w.b.config.join("Downloads/pic.jpg")).unwrap(), data);
    assert!(client::pending_files(&w.b.config).is_empty());
    // A decline refuses it on the server (the sender sees "not delivered").
    let src = w.base.join("src/no.jpg");
    write(&src, 10, 1);
    let x2 = uuid::Uuid::new_v4().to_string();
    client::deposit_files(&w.a.state, &w.a.config, &w.b_for_a, &x2, &x2, &deposit_files_of(&[(src, "no.jpg")]), &[], &["no.jpg".into()],
        &|_, _| {}, &|_| {}, &AtomicBool::new(false), None).await.unwrap();
    assert_eq!(fetch(&w).await, 0);
    let link = client::pending_files(&w.b.config)[0].link_id.clone();
    client::decide_file(&w.b.config, &link, false);
    assert_eq!(fetch(&w).await, 0);
    assert_eq!(server_items(&w), 0);
    assert!(!w.b.config.join("Downloads/no.jpg").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_past_empty_files_and_one_device_refusal_keeps_it_for_the_other() {
    let w = world("resume-empty").await;
    let src = w.base.join("src");
    let a = write(&src.join("a.bin"), (seal::SEG / 2) as usize, 1);
    write(&src.join("e.bin"), 0, 0);
    let b = write(&src.join("b.bin"), (seal::SEG / 3) as usize, 2);
    let c = write(&src.join("c.bin"), (seal::SEG * 2) as usize, 3);
    let files = deposit_files_of(&[(src.join("a.bin"), "a.bin"), (src.join("e.bin"), "e.bin"), (src.join("b.bin"), "b.bin"), (src.join("c.bin"), "c.bin")]);
    let xfer = uuid::Uuid::new_v4().to_string();
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    let progress = move |d: u64, _t: u64| { if d >= seal::SEG { c2.store(true, Ordering::SeqCst); } };
    assert_eq!(client::deposit_files(&w.a.state, &w.a.config, &w.b_for_a, &xfer, &xfer, &files, &[], &["a.bin".into()],
        &progress, &|_| {}, &cancel, None).await.unwrap_err(), DepositError::Canceled);
    tokio::time::sleep(Duration::from_millis(300)).await;
    client::deposit_files(&w.a.state, &w.a.config, &w.b_for_a, &xfer, &xfer, &files, &[], &["a.bin".into()],
        &|_, _| {}, &|_| {}, &AtomicBool::new(false), None).await.unwrap();
    assert_eq!(fetch(&w).await, 1);
    let dl = w.b.config.join("Downloads");
    assert_eq!(std::fs::read(dl.join("a.bin")).unwrap(), a);
    assert_eq!(std::fs::read(dl.join("b.bin")).unwrap(), b);
    assert_eq!(std::fs::read(dl.join("c.bin")).unwrap(), c);
    assert!(dl.join("e.bin").exists());

    // B has a second device; one device turning an item down leaves it for the other.
    let b2 = node(&w.base, "b2").await;
    introduce(&b2, &w.a);
    let person = crate::friends::upsert_by_endpoint(&w.a.config, &b2.eid(), "Bea phone");
    let _ = person;
    // Seal to both devices by hand and deposit.
    let m = uuid::Uuid::new_v4().to_string();
    let recips = keys::recipients(&w.a.config, &[w.b.eid(), b2.eid()]);
    assert_eq!(recips.len(), 2);
    crate::friends::upsert_by_endpoint(&w.s.config, &b2.eid(), "Bea phone");
    let (env, _) = seal::seal(w.a.ep.secret_key(), &uuid::Uuid::new_v4().to_string(), "chat", 1, &recips,
        &serde_json::to_vec(&chat_frame(&m, "for both")).unwrap(), 0).unwrap();
    let conn = w.a.ep.connect(iroh_net::dial_addr(w.s.ep.id()), iroh_net::ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    iroh_net::write_frame(&mut send, &json!({"kind": "mailbox.deposit", "header": env})).await.unwrap();
    assert_eq!(iroh_net::read_frame_cap(&mut recv, 4096).await.unwrap()["ok"], true);
    send.finish().unwrap();
    assert_eq!(iroh_net::read_frame_cap(&mut recv, 4096).await.unwrap()["state"], "held");
    // B (desktop) doesn't know A any more → refuses; the phone still gets it.
    crate::friends::remove(&w.b.config, &crate::friends::chat_sender(&w.b.config, &w.a.eid()).unwrap().id).unwrap();
    assert_eq!(fetch(&w).await, 0);
    assert_eq!(server_items(&w), 1, "one device's refusal doesn't delete it for the other");
    crate::friends::upsert_by_endpoint(&b2.config, &w.a.eid(), "Ash");
    let n = client::fetch_from(&b2.state, &b2.config, &w.s.eid()).await.unwrap();
    assert_eq!(n, 1);
    assert_eq!(server_items(&w), 0);
    b2.listener.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsending_a_held_message_takes_it_back() {
    let w = world("unsend").await;
    let m = uuid::Uuid::new_v4().to_string();
    client::deposit_chat(&w.a.state, &w.a.config, &w.b_for_a, "chat", &chat_frame(&m, "oops"), Some(&m)).await.unwrap();
    assert_eq!(client::held_server_of(&w.a.config, &m), Some(w.s.eid()));
    assert_eq!(client::unsend_held(&w.a.state, &w.a.config, &m).await, client::Unsend::Removed);
    assert_eq!(server_items(&w), 0);
    assert_eq!(fetch(&w).await, 0, "the friend never sees an unsent held message");
    // Once delivered, the server says so (the unsend then travels as an op).
    let m2 = uuid::Uuid::new_v4().to_string();
    client::deposit_chat(&w.a.state, &w.a.config, &w.b_for_a, "chat", &chat_frame(&m2, "hi"), Some(&m2)).await.unwrap();
    assert_eq!(fetch(&w).await, 1);
    assert_eq!(client::unsend_held(&w.a.state, &w.a.config, &m2).await, client::Unsend::Delivered);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fixed_udp_port_is_used() {
    let port = { let s = std::net::UdpSocket::bind("0.0.0.0:0").unwrap(); s.local_addr().unwrap().port() };
    let ep = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .relay_mode(iroh::RelayMode::Disabled)
        .bind_addr(std::net::SocketAddr::from(([0, 0, 0, 0], port))).unwrap()
        .bind().await.unwrap();
    let bound: Vec<u16> = ep.bound_sockets().iter().map(|a| a.port()).collect();
    assert!(bound.contains(&port), "{bound:?} should include {port}");
    ep.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn items_sealed_to_a_rotated_key_still_open() {
    let w = world("rotate").await;
    let m = uuid::Uuid::new_v4().to_string();
    client::deposit_chat(&w.a.state, &w.a.config, &w.b_for_a, "chat", &chat_frame(&m, "before rotation"), Some(&m)).await.unwrap();
    // B rotates (key file aged past a month) before fetching.
    let key = w.b.config.join("mailbox-key.key");
    let old = std::time::SystemTime::now() - Duration::from_secs(31 * 24 * 3600);
    std::fs::File::options().write(true).open(&key).unwrap().set_modified(old).unwrap();
    assert!(keys::maybe_rotate(&w.b.config));
    assert_eq!(keys::all_secrets(&w.b.config).len(), 2);
    assert_eq!(fetch(&w).await, 1);
    let b_thread = crate::friends::chat_sender(&w.b.config, &w.a.eid()).unwrap().id;
    assert_eq!(crate::chat::messages(&w.b.config, &b_thread)[0].text, "before rotation");
}

/// LIVE end-to-end against a real Transfer Server (the owner's Linux box), over
/// the real network (n0 discovery/relays). Two phases so the box can be told
/// about the test identities in between:
///   LIVE_DIR=/tmp/dbl cargo test --lib live_setup -- --ignored --nocapture
///   (add the printed endpoints as friends on the server)
///   LIVE_DIR=/tmp/dbl LIVE_SERVER=<eid> cargo test --lib live_run -- --ignored --nocapture
mod live {
    use super::*;

    fn dirs() -> (PathBuf, PathBuf) {
        let base = PathBuf::from(std::env::var("LIVE_DIR").unwrap_or_else(|_| "/tmp/dbl".into()));
        (base.join("a"), base.join("b"))
    }

    fn identity(dir: &Path) -> iroh::SecretKey {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join("iroh-identity.key");
        if !p.exists() {
            let seed: [u8; 32] = rand::random();
            std::fs::write(&p, seed).unwrap();
        }
        iroh::SecretKey::from_bytes(&std::fs::read(&p).unwrap().try_into().unwrap())
    }

    #[test]
    #[ignore]
    fn live_setup() {
        let (a, b) = dirs();
        println!("LIVE_A {}", identity(&a).public());
        println!("LIVE_B {}", identity(&b).public());
    }

    async fn up(dir: &Path) -> (Arc<IrohState>, iroh::Endpoint, tokio::task::JoinHandle<()>) {
        let ep = iroh_net::start(dir).await.unwrap();
        let state = Arc::new(IrohState::default());
        let _ = state.location_config.set(dir.to_path_buf());
        let _ = state.endpoint.set(ep.clone());
        let l = tokio::spawn(iroh_net::accept_loop(ep.clone(), state.clone()));
        (state, ep, l)
    }

    /// A friend-hello with our mailbox fields; the reply's fields are applied.
    async fn hello(state: &IrohState, ep: &iroh::Endpoint, dir: &Path, to: &str) {
        let conn = tokio::time::timeout(Duration::from_secs(30), ep.connect(iroh_net::dial_addr(to.parse().unwrap()), iroh_net::ALPN)).await.unwrap().unwrap();
        let req = json!({"kind": "friend-hello", "friend_id": "", "endpoint_id": ep.id().to_string(), "name": "Transfer test",
            "mailbox": super::super::hello_fields(dir, ep.secret_key(), to)});
        let reply = rpc(&conn, req).await;
        super::super::on_hello(state, dir, to, &reply);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore]
    async fn live_run() {
        let server = std::env::var("LIVE_SERVER").expect("LIVE_SERVER=<endpoint id>");
        let (a_dir, b_dir) = dirs();
        let (a_eid, b_eid) = (identity(&a_dir).public().to_string(), identity(&b_dir).public().to_string());
        let t0 = std::time::Instant::now();
        let b_for_a = crate::friends::upsert_by_endpoint(&a_dir, &b_eid, "Test B").id;
        crate::friends::upsert_by_endpoint(&a_dir, &server, "Linux Box");
        crate::friends::upsert_by_endpoint(&b_dir, &a_eid, "Test A");
        crate::friends::upsert_by_endpoint(&b_dir, &server, "Linux Box");
        // B comes online once, long enough to introduce itself, then goes away.
        {
            let (bs, bep, bl) = up(&b_dir).await;
            hello(&bs, &bep, &b_dir, &server).await;
            let (as_, aep, al) = up(&a_dir).await;
            hello(&as_, &aep, &a_dir, &b_eid).await; // A learns B's key (B's reply)
            bl.abort(); bep.close().await; al.abort(); aep.close().await;
        }
        println!("[{:>5.1}s] B introduced itself and went offline", t0.elapsed().as_secs_f64());
        let (as_, aep, _al) = up(&a_dir).await;
        hello(&as_, &aep, &a_dir, &server).await;
        let srv = client::servers(&a_dir).into_iter().find(|s| s.eid == server).expect("the box granted access in its hello");
        println!("[{:>5.1}s] A sees server {:?} member={} through={}", t0.elapsed().as_secs_f64(), srv.name, srv.member, srv.through);
        client::set_prefs(&a_dir, &server, Some(true), None, Some("seen")).unwrap();
        // Chat + a 24 MB file, while B is offline.
        let msg = uuid::Uuid::new_v4().to_string();
        let held = client::deposit_chat(&as_, &a_dir, &b_for_a, "chat", &chat_frame(&msg, "live: held on the Linux box"), Some(&msg)).await.unwrap();
        println!("[{:>5.1}s] chat held on {:?}", t0.elapsed().as_secs_f64(), held.name);
        let src = a_dir.join("live-24MB.bin");
        let data: Vec<u8> = (0..24 * 1024 * 1024).map(|_| rand::random::<u8>()).collect();
        std::fs::write(&src, &data).unwrap();
        let want = { use sha2::Digest; hex::encode(sha2::Sha256::digest(&data)) };
        let xfer = uuid::Uuid::new_v4().to_string();
        let up_t = std::time::Instant::now();
        let held = client::deposit_files(&as_, &a_dir, &b_for_a, &xfer, &xfer, &deposit_files_of(&[(src.clone(), "live-24MB.bin")]), &[], &["live-24MB.bin".into()],
            &|_, _| {}, &|_| {}, &AtomicBool::new(false), None).await.unwrap();
        println!("[{:>5.1}s] file held on {:?} (upload {:.1}s)", t0.elapsed().as_secs_f64(), held.name, up_t.elapsed().as_secs_f64());
        aep.close().await; // A goes offline too: delivery doesn't need the sender.
        // B comes back and pulls.
        let (bs, bep, _bl) = up(&b_dir).await;
        let n = client::fetch_from(&bs, &b_dir, &server).await.unwrap();
        println!("[{:>5.1}s] B received {n} item(s)", t0.elapsed().as_secs_f64());
        assert_eq!(n, 2);
        let got = std::fs::read(b_dir.join("Downloads/live-24MB.bin")).unwrap();
        let have = { use sha2::Digest; hex::encode(sha2::Sha256::digest(&got)) };
        assert_eq!(have, want);
        println!("sha256 match: {have}");
        let thread = crate::friends::chat_sender(&b_dir, &a_eid).unwrap().id;
        let texts: Vec<String> = crate::chat::messages(&b_dir, &thread).into_iter().map(|m| m.text).collect();
        assert_eq!(texts.iter().filter(|t| t.as_str() == "live: held on the Linux box").count(), 1);
        assert_eq!(client::fetch_from(&bs, &b_dir, &server).await.unwrap(), 0, "nothing twice");
        bep.close().await;
        let (as_, aep, _al) = up(&a_dir).await;
        let r = client::refresh_status(&as_, &a_dir).await;
        println!("receipts: {:?}", r.iter().map(|r| (r.sent.kind.clone(), r.state.clone())).collect::<Vec<_>>());
        assert!(r.len() == 2 && r.iter().all(|r| r.state == "delivered"));
        aep.close().await;
        println!("LIVE OK in {:.1}s", t0.elapsed().as_secs_f64());
    }
}

// ── sending to EVERY device of a friend (fanout) ────────────────────────────

use crate::fanout::{self, Delivery, Outcome, Record};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// A second device of B's (same account): an "iPhone" next to B's "Mac".
async fn second_device(w: &World, name: &str) -> Node {
    let n = node(&w.base, name).await;
    let account = "bea-account";
    crate::friends::upsert_by_endpoint(&w.a.config, &n.eid(), "Bea");
    crate::friends::set_device_info(&w.a.config, &w.b.eid(), Some("laptop"), Some(account));
    crate::friends::set_device_os(&w.a.config, &w.b.eid(), "macos");
    crate::friends::set_device_info(&w.a.config, &n.eid(), Some("phone"), Some(account));
    crate::friends::set_device_os(&w.a.config, &n.eid(), "ios");
    crate::friends::upsert_by_endpoint(&n.config, &w.a.eid(), "Ash");
    crate::friends::upsert_by_endpoint(&n.config, &w.s.eid(), "Box");
    crate::friends::upsert_by_endpoint(&w.s.config, &n.eid(), "Bea");
    introduce(&n, &w.a);
    client::learn_grant(&n.config, &w.s.eid(), server::grant_for(&w.s.config, &n.eid()).as_ref(), false);
    n
}

fn inbox(n: &Node) -> PathBuf {
    let dir = n.config.join("Downloads");
    std::fs::create_dir_all(&dir).unwrap();
    let _ = n.state.test_inbox.set(dir.clone());
    dir
}

/// Real direct transfers from A's endpoint; "offline" = the dial fails.
struct TestEnv {
    ep: iroh::Endpoint,
    reachable: Mutex<HashSet<String>>,
}

impl fanout::Env for TestEnv {
    fn direct(&self, job: fanout::Job, progress: fanout::Progress) -> fanout::BoxFut<Outcome> {
        let ep = self.ep.clone();
        let up = self.reachable.lock().unwrap().contains(&job.eid);
        Box::pin(async move {
            if !up {
                tokio::time::sleep(Duration::from_millis(50)).await;
                return Outcome::Offline;
            }
            let id: iroh::EndpointId = job.eid.parse().unwrap();
            let Ok(Ok(conn)) = tokio::time::timeout(job.first_dial, ep.connect(iroh_net::dial_addr(id), iroh_net::ALPN)).await else {
                return Outcome::Offline;
            };
            let paths: Vec<PathBuf> = job.record.paths.iter().map(PathBuf::from).collect();
            let r = iroh_net::send_files(&conn, &paths, &job.cancel, |d, _| progress(d, 0.0), "Ash", &AtomicBool::new(false)).await;
            conn.close(0u32.into(), b"done");
            match r {
                Ok(_) => Outcome::Delivered,
                Err(e) => Outcome::Failed(e.to_string()),
            }
        })
    }
    fn changed(&self, _config: &Path, _rec: &Record, _live: &HashMap<String, (u64, f64)>, _significant: bool) {}
    fn stop_legs(&self, _id: &str, _reason: iroh_net::CancelReason) {}
}

fn test_engine(w: &World, reachable: &[&Node]) -> Arc<fanout::Engine> {
    let env = TestEnv { ep: w.a.ep.clone(), reachable: Mutex::new(reachable.iter().map(|n| n.eid()).collect()) };
    fanout::engine_for_tests(Arc::new(env), w.a.state.clone(), w.a.config.clone())
}

fn sha(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(std::fs::read(path).unwrap()))
}

/// Start a send of `src` to every one of B's devices; returns its id.
fn send_to_bea(w: &World, engine: &Arc<fanout::Engine>, src: &Path) -> String {
    let (owner, devices) = fanout::targets(&w.a.config, &w.b_for_a, &w.a.eid(), None).unwrap();
    assert_eq!(devices.len(), 2, "both of Bea's devices are targets");
    let mut labels: Vec<&str> = devices.iter().map(|d| d.label.as_str()).collect();
    labels.sort_unstable();
    assert_eq!(labels, ["Mac", "iPhone"]);
    let (names, total) = iroh_net::card_summary(&[src.to_string_lossy().into_owned()]).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    engine.begin(Record {
        id: id.clone(), chat_id: id.clone(), peer_id: owner.id, friend_name: owner.name,
        paths: vec![src.to_string_lossy().into_owned()], names, total, attempt: 1, devices, ..Default::default()
    });
    id
}

async fn until_states(w: &World, id: &str, want: &[(&str, &str)]) -> Vec<Delivery> {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let devices = fanout::get(&w.a.config, id).unwrap().devices;
        if want.iter().all(|(eid, st)| devices.iter().any(|d| d.eid == *eid && d.state == *st)) {
            return devices;
        }
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {want:?}; have {devices:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fanout_two_online_devices_both_get_the_files() {
    let w = world("fan-online").await;
    let b2 = second_device(&w, "b2").await;
    let (in1, in2) = (inbox(&w.b), inbox(&b2));
    let src = w.base.join("src/holiday.mov");
    write(&src, 5 * 1024 * 1024 + 77, 3);
    let engine = test_engine(&w, &[&w.b, &b2]);
    let id = send_to_bea(&w, &engine, &src);
    until_states(&w, &id, &[(&w.b.eid(), fanout::DELIVERED), (&b2.eid(), fanout::DELIVERED)]).await;
    assert_eq!(sha(&in1.join("holiday.mov")), sha(&src));
    assert_eq!(sha(&in2.join("holiday.mov")), sha(&src));
    assert_eq!(server_items(&w), 0, "nothing went through the server");
    let card = fanout::card(&fanout::get(&w.a.config, &id).unwrap(), &HashMap::new());
    assert_eq!(card.state, crate::models::TransferState::Completed);
    b2.listener.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fanout_offline_devices_share_one_server_copy_until_each_has_it() {
    let w = world("fan-held").await;
    let b2 = second_device(&w, "b2").await;
    let src = w.base.join("src/report.pdf");
    write(&src, 1_300_000, 9);
    let engine = test_engine(&w, &[]);
    let id = send_to_bea(&w, &engine, &src);
    let devices = until_states(&w, &id, &[(&w.b.eid(), fanout::HELD), (&b2.eid(), fanout::HELD)]).await;
    assert_eq!(devices[0].item_id, devices[1].item_id, "ONE sealed copy for both devices");
    assert_eq!(devices[0].via.as_deref(), Some("Linux Box"));
    assert_eq!(server_items(&w), 1, "stored (and counted against the quota) once");
    // The Mac takes it first: the server keeps it for the iPhone.
    assert_eq!(fetch(&w).await, 1);
    assert_eq!(server_items(&w), 1, "still held for the other device");
    assert_eq!(server::pending_recipients(&w.s.config), vec![(b2.eid(), 1)]);
    for r in client::refresh_status(&w.a.state, &w.a.config).await {
        assert!(engine.receipt(&r));
    }
    until_states(&w, &id, &[(&w.b.eid(), fanout::DELIVERED), (&b2.eid(), fanout::HELD)]).await;
    // A second fetch by the Mac finds nothing new.
    assert_eq!(fetch(&w).await, 0);
    // The iPhone takes it: now it's gone from the server.
    let got = tokio::time::timeout(Duration::from_secs(30), client::fetch_from(&b2.state, &b2.config, &w.s.eid())).await.unwrap().unwrap();
    assert_eq!(got, 1);
    assert_eq!(server_items(&w), 0, "deleted once every device has it");
    for r in client::refresh_status(&w.a.state, &w.a.config).await {
        assert!(engine.receipt(&r));
    }
    until_states(&w, &id, &[(&w.b.eid(), fanout::DELIVERED), (&b2.eid(), fanout::DELIVERED)]).await;
    assert_eq!(sha(&w.b.config.join("Downloads/report.pdf")), sha(&src));
    assert_eq!(sha(&b2.config.join("Downloads/report.pdf")), sha(&src));
    b2.listener.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fanout_online_device_direct_offline_device_held_alone() {
    let w = world("fan-mixed").await;
    let b2 = second_device(&w, "b2").await;
    let in1 = inbox(&w.b);
    let src = w.base.join("src/song.m4a");
    write(&src, 800_000, 4);
    let engine = test_engine(&w, &[&w.b]);
    let id = send_to_bea(&w, &engine, &src);
    until_states(&w, &id, &[(&w.b.eid(), fanout::DELIVERED), (&b2.eid(), fanout::HELD)]).await;
    assert_eq!(sha(&in1.join("song.m4a")), sha(&src));
    // Sealed for the offline iPhone only (the Mac already has it).
    assert_eq!(server::pending_recipients(&w.s.config), vec![(b2.eid(), 1)]);
    let got = tokio::time::timeout(Duration::from_secs(30), client::fetch_from(&b2.state, &b2.config, &w.s.eid())).await.unwrap().unwrap();
    assert_eq!(got, 1);
    assert_eq!(server_items(&w), 0);
    assert_eq!(sha(&b2.config.join("Downloads/song.m4a")), sha(&src));
    let card = fanout::card(&fanout::get(&w.a.config, &id).unwrap(), &HashMap::new());
    assert_eq!(card.state, crate::models::TransferState::Completed, "reached the person");
    b2.listener.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fanout_without_a_server_waits_and_survives_a_restart() {
    let w = world("fan-queue").await;
    client::forget(&w.a.config, &w.s.eid());
    let b2 = second_device(&w, "b2").await;
    let (in1, in2) = (inbox(&w.b), inbox(&b2));
    let src = w.base.join("src/notes.txt");
    write(&src, 70_000, 1);
    let engine = test_engine(&w, &[&w.b]);
    let id = send_to_bea(&w, &engine, &src);
    until_states(&w, &id, &[(&w.b.eid(), fanout::DELIVERED), (&b2.eid(), fanout::WAITING)]).await;
    assert_eq!(sha(&in1.join("notes.txt")), sha(&src));
    assert!(!in2.join("notes.txt").exists());
    // It's on disk, not just in memory.
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(w.a.config.join("fanout.json")).unwrap()).unwrap();
    assert_eq!(saved[&id]["devices"].as_array().unwrap().iter().filter(|d| d["state"] == "waiting").count(), 1);
    let card = fanout::card(&fanout::get(&w.a.config, &id).unwrap(), &HashMap::new());
    assert_eq!(card.state, crate::models::TransferState::Completed);
    drop(engine);
    // "Restart": a fresh engine, and the iPhone is back.
    let engine = test_engine(&w, &[&w.b, &b2]);
    engine.recover();
    engine.retry_due();
    until_states(&w, &id, &[(&w.b.eid(), fanout::DELIVERED), (&b2.eid(), fanout::DELIVERED)]).await;
    assert_eq!(sha(&in2.join("notes.txt")), sha(&src));
    // The Mac got it exactly once.
    assert_eq!(std::fs::read_dir(&in1).unwrap().count(), 1);
    b2.listener.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn held_copy_of_a_file_that_already_landed_is_not_duplicated() {
    let w = world("fan-dedupe").await;
    let src = w.base.join("src/pic.jpg");
    let data = write(&src, 400_000, 8);
    // The same file already reached B (a direct copy of this send landed first).
    let dl = w.b.config.join("Downloads");
    std::fs::create_dir_all(&dl).unwrap();
    std::fs::write(dl.join("pic.jpg"), &data).unwrap();
    let xfer = uuid::Uuid::new_v4().to_string();
    client::deposit_files(&w.a.state, &w.a.config, &w.b_for_a, &xfer, &xfer, &deposit_files_of(&[(src.clone(), "pic.jpg")]), &[], &["pic.jpg".into()],
        &|_, _| {}, &|_| {}, &AtomicBool::new(false), None).await.unwrap();
    assert_eq!(fetch(&w).await, 1);
    let names: Vec<String> = std::fs::read_dir(&dl).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["pic.jpg"], "no \"pic (2).jpg\" copy");
    assert_eq!(std::fs::read(dl.join("pic.jpg")).unwrap(), data);
    assert_eq!(server_items(&w), 0);
    // Delivered twice (a replay): still one file.
    assert_eq!(fetch(&w).await, 0);
    assert_eq!(std::fs::read_dir(&dl).unwrap().count(), 1);
}

// ── chat to every device (iMessage-style) ───────────────────────────────────

fn b_thread_on(n: &Node, a: &Node) -> String {
    crate::friends::chat_sender(&n.config, &a.eid()).unwrap().id
}

/// S can sign relay requests (its identity key file, as the app keeps it) and
/// `phone` registered for push there.
fn register_phone(w: &World, phone: &Node) {
    std::fs::write(w.s.config.join("iroh-identity.key"), w.s.ep.secret_key().to_bytes()).unwrap();
    let c = server::load_config(&w.s.config);
    let r = super::push::register(&w.s.config, &c, &phone.eid(), &json!({"sealed_token": seal::b64(b"sealed-apns-token")}));
    assert_eq!(r["ok"], true);
}

fn wakes_for(eid: &str) -> Vec<Value> {
    super::push::SENT_FOR_TESTS.lock().unwrap().iter().filter(|(to, _)| to == eid).map(|(_, b)| b.clone()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat_reaches_online_device_directly_and_offline_phone_through_its_own_copy() {
    let w = world("chat-each").await;
    let b2 = second_device(&w, "b2").await; // Bea's iPhone, asleep (unseen → not dialed)
    register_phone(&w, &b2);
    let eids = crate::friends::person_endpoints(&w.a.config, &w.b_for_a);
    assert_eq!(eids.len(), 2);
    let m = uuid::Uuid::new_v4().to_string();
    let frame = chat_frame(&m, "on my way");
    let mut skip = HashSet::new();
    let out = iroh_net::deliver_chat_message(&w.a.state, &w.a.ep, &w.a.config, &w.b_for_a, &eids, &frame, &m, true, &mut skip).await;
    let iroh_net::ChatOutcome::Delivered { reach, copies } = out else { panic!("expected delivered, got {out:?}") };
    assert_eq!(reach.delivered, vec![w.b.eid()], "the Mac got it directly");
    assert_eq!(copies, vec![b2.eid()], "the sleeping iPhone gets its own server copy");
    assert!(skip.contains(&b2.eid()), "the rest of this round skips the phone");
    // Only the phone has something waiting; the server woke it.
    assert_eq!(server::pending_recipients(&w.s.config), vec![(b2.eid(), 1)]);
    let wakes = wakes_for(&b2.eid());
    assert_eq!(wakes.len(), 1, "one push for the phone");
    assert_eq!(wakes[0]["server"], w.s.eid());
    let sig = seal::unb64(wakes[0]["sig"].as_str().unwrap()).unwrap();
    let msg = super::push::request_message(&w.s.eid(), wakes[0]["sealed_token"].as_str().unwrap(), wakes[0]["collapse"].as_str().unwrap(),
        wakes[0]["payload"].as_str().unwrap(), wakes[0]["ts"].as_u64().unwrap());
    assert!(w.s.ep.secret_key().public().verify(&msg, &iroh::Signature::from_bytes(&sig.try_into().unwrap())).is_ok(), "the relay can verify it");
    // The sender's ledger marks it as a copy (its receipts never touch the bubble).
    let sent: Vec<client::Sent> = client::sent_all(&w.a.config).into_values().collect();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].copy && sent[0].to == vec![b2.eid()]);
    // The Mac has it once; nothing on the server for it.
    assert_eq!(crate::chat::messages(&w.b.config, &b_thread_on(&w.b, &w.a)).len(), 1);
    assert_eq!(fetch(&w).await, 0);
    // The phone fetches its copy: exactly one message, even if it also arrives
    // directly / synced from the Mac later.
    let got = tokio::time::timeout(Duration::from_secs(30), client::fetch_from(&b2.state, &b2.config, &w.s.eid())).await.unwrap().unwrap();
    assert_eq!(got, 1);
    let t2 = b_thread_on(&b2, &w.a);
    assert_eq!(crate::chat::messages(&b2.config, &t2)[0].text, "on my way");
    iroh_net::apply_incoming_chat(&b2.state, &b2.config, &w.a.eid(), &frame, None, None);
    assert_eq!(crate::chat::messages(&b2.config, &t2).len(), 1, "no duplicate");
    assert_eq!(server_items(&w), 0);
    b2.listener.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn edits_follow_a_phone_copy_and_unsend_takes_it_back() {
    let w = world("chat-each-ops").await;
    let b2 = second_device(&w, "b2").await;
    let eids = crate::friends::person_endpoints(&w.a.config, &w.b_for_a);
    // Message 1 → Mac direct + phone copy; then an edit follows the copy.
    let m = uuid::Uuid::new_v4().to_string();
    let mut skip = HashSet::new();
    let out = iroh_net::deliver_chat_message(&w.a.state, &w.a.ep, &w.a.config, &w.b_for_a, &eids, &chat_frame(&m, "helo"), &m, true, &mut skip).await;
    assert!(matches!(out, iroh_net::ChatOutcome::Delivered { .. }));
    let edit = json!({"kind": "chat", "v": 2, "msgKind": "edit", "friendId": "x", "fromName": "Ash", "targetId": m, "text": "hello"});
    iroh_net::op_follows_copies(&w.a.state, &w.a.config, &w.b_for_a, "edit", &m, &edit).await;
    // Message 2 → unsent before the phone fetched: its copy is simply taken back.
    let m2 = uuid::Uuid::new_v4().to_string();
    let out = iroh_net::deliver_chat_message(&w.a.state, &w.a.ep, &w.a.config, &w.b_for_a, &eids, &chat_frame(&m2, "oops"), &m2, true, &mut skip).await;
    assert!(matches!(out, iroh_net::ChatOutcome::Delivered { ref copies, .. } if copies == &vec![b2.eid()]));
    let del = json!({"kind": "chat", "v": 2, "msgKind": "delete", "friendId": "x", "fromName": "Ash", "targetId": m2});
    iroh_net::op_follows_copies(&w.a.state, &w.a.config, &w.b_for_a, "delete", &m2, &del).await;
    assert_eq!(server::pending_recipients(&w.s.config), vec![(b2.eid(), 2)], "message 1 + its edit; message 2 is gone");
    let got = tokio::time::timeout(Duration::from_secs(30), client::fetch_from(&b2.state, &b2.config, &w.s.eid())).await.unwrap().unwrap();
    assert_eq!(got, 2);
    let msgs = crate::chat::messages(&b2.config, &b_thread_on(&b2, &w.a));
    assert_eq!(msgs.len(), 1, "the unsent message never reached the phone");
    assert_eq!((msgs[0].text.as_str(), msgs[0].edited), ("hello", true));
    b2.listener.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn held_for_all_devices_stays_for_the_phone_after_the_mac_got_it() {
    let w = world("chat-all-held").await;
    let b2 = second_device(&w, "b2").await;
    // Both offline at send time: one item sealed for both, kept per device.
    let m = uuid::Uuid::new_v4().to_string();
    client::deposit_chat(&w.a.state, &w.a.config, &w.b_for_a, "chat", &chat_frame(&m, "hey"), Some(&m)).await.unwrap();
    let mut pending = server::pending_recipients(&w.s.config);
    pending.sort();
    let mut want = vec![(w.b.eid(), 1), (b2.eid(), 1)];
    want.sort();
    assert_eq!(pending, want);
    // The Mac showed up and got it directly: the server copy stays for the phone.
    client::delivered_directly(&w.a.state, &w.a.config, &m, &[w.b.eid()]);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(server_items(&w), 1, "not canceled while the phone still needs it");
    // The Mac fetching it too is harmless (dedupe); the phone gets its copy.
    assert_eq!(fetch(&w).await, 1);
    assert_eq!(server_items(&w), 1, "still waiting for the phone");
    let r = client::refresh_status(&w.a.state, &w.a.config).await;
    assert_eq!((r[0].state.as_str(), r[0].delivered_to.clone()), ("held", vec![w.b.eid()]), "reached one device → the bubble says Delivered");
    let got = tokio::time::timeout(Duration::from_secs(30), client::fetch_from(&b2.state, &b2.config, &w.s.eid())).await.unwrap().unwrap();
    assert_eq!(got, 1);
    assert_eq!(server_items(&w), 0);
    // Everyone else's copies were all taken: now it cancels normally.
    let m2 = uuid::Uuid::new_v4().to_string();
    client::deposit_chat(&w.a.state, &w.a.config, &w.b_for_a, "chat", &chat_frame(&m2, "x"), Some(&m2)).await.unwrap();
    client::delivered_directly(&w.a.state, &w.a.config, &m2, &[w.b.eid(), b2.eid()]);
    for _ in 0..50 {
        if server_items(&w) == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(server_items(&w), 0);
    b2.listener.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_transfer_server_holds_its_own_messages_for_a_sleeping_phone() {
    let w = world("chat-self").await;
    // The server's own app (the Linux box) chats with Bea, whose phone is asleep.
    introduce(&w.b, &w.s);
    let b_for_s = crate::friends::chat_sender(&w.s.config, &w.b.eid()).unwrap().id;
    let me = w.s.eid();
    assert!(client::can_hold_chat(&w.s.config, &me, &b_for_s), "it can hold chat on itself");
    assert!(!client::can_hold(&w.s.config, &me, &b_for_s), "files still need another device");
    register_phone(&w, &w.b);
    let m = uuid::Uuid::new_v4().to_string();
    let held = client::deposit_chat(&w.s.state, &w.s.config, &b_for_s, "chat", &chat_frame(&m, "box says hi"), Some(&m)).await.unwrap();
    assert_eq!((held.server.as_str(), held.name.as_str()), (client::SELF, "Linux Box"));
    assert_eq!(server::pending_recipients(&w.s.config), vec![(w.b.eid(), 1)]);
    assert_eq!(wakes_for(&w.b.eid()).len(), 1, "the phone was woken");
    assert_eq!(fetch(&w).await, 1);
    let got = crate::chat::messages(&w.b.config, &b_thread_on(&w.b, &w.s));
    assert_eq!(got[0].text, "box says hi");
    let r = client::refresh_status(&w.s.state, &w.s.config).await;
    assert_eq!(r[0].state, "delivered", "receipts work against itself too");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_phone_online_at_deposit_is_woken_once_when_it_stops_answering() {
    let w = world("chat-late-push").await;
    register_phone(&w, &w.b);
    super::note_seen(&w.b.eid()); // just connected → the deposit pokes instead
    let m = uuid::Uuid::new_v4().to_string();
    client::deposit_chat(&w.a.state, &w.a.config, &w.b_for_a, "chat", &chat_frame(&m, "you there?"), Some(&m)).await.unwrap();
    assert!(wakes_for(&w.b.eid()).is_empty(), "no push while it looked online");
    super::push::on_unreachable(&w.s.config, &w.b.eid());
    assert_eq!(wakes_for(&w.b.eid()).len(), 1, "the failed poke wakes it");
    super::push::on_unreachable(&w.s.config, &w.b.eid());
    assert_eq!(wakes_for(&w.b.eid()).len(), 1, "only once per item");
}

// ── the owner's friends may use the owner's server ──────────────────────────

/// O = the owner's phone (in an account), S = the owner's Transfer Server (NOT
/// linked; owned by O's account, friends only with O), F = a friend of O who
/// has never met S.
struct OwnerWorld {
    base: PathBuf,
    o: Node,
    s: Node,
    f: Node,
    account_key: iroh::SecretKey,
    /// F's thread for O / O's thread for F.
    o_on_f: String,
    f_on_o: String,
}

impl Drop for OwnerWorld {
    fn drop(&mut self) {
        for n in [&self.o, &self.s, &self.f] {
            n.listener.abort();
        }
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// `who` proves `key`'s account to `to` (the signed hello field).
fn prove_account(to: &Node, who: &str, key: &iroh::SecretKey) {
    crate::friends::apply_device_hello(&to.config, who, &json!({"device_kind": "phone", "device_os": "ios",
        "account_pub": hex::encode(key.public().as_bytes()),
        "account_sig": hex::encode(key.sign(who.as_bytes()).to_bytes())}));
}

async fn owner_world(tag: &str) -> OwnerWorld {
    let base = std::env::temp_dir().join(format!("dropbeam-mbx-own-{tag}-{}", uuid::Uuid::new_v4()));
    let (o, s, f) = (node(&base, "o").await, node(&base, "s").await, node(&base, "f").await);
    let account_key = iroh::SecretKey::generate();
    crate::link::adopt_key_for_tests(&o.config, &account_key);
    let account = crate::link::account_pub(&o.config).unwrap();
    // S knows only O (and O proved its account); O and F are friends.
    crate::friends::upsert_by_endpoint(&s.config, &o.eid(), "Ashton");
    prove_account(&s, &o.eid(), &account_key);
    crate::friends::upsert_by_endpoint(&o.config, &s.eid(), "Linux Box");
    let f_on_o = crate::friends::upsert_by_endpoint(&o.config, &f.eid(), "Fay").id;
    let o_on_f = crate::friends::upsert_by_endpoint(&f.config, &o.eid(), "Ashton").id;
    let mut c = server::ServerConfig { enabled: true, name: "Linux Box".into(), cap_bytes: 1 << 30, min_free: 1,
        owner_account: account, ..Default::default() };
    server::init_root(&s.config, &mut c).unwrap();
    server::save_config(&s.config, &c).unwrap();
    OwnerWorld { base, o, s, f, account_key, o_on_f, f_on_o }
}

fn server_of<'a>(list: &'a [UsableServer], eid: &str) -> Option<&'a UsableServer> {
    list.iter().find(|s| s.eid == eid)
}

/// O learns S is its own server and says yes to sharing it; F hears about it
/// from O's hello.
async fn owner_shares(w: &OwnerWorld) {
    let grant = server::grant_for(&w.s.config, &w.o.eid()).expect("the owner's device is told");
    assert_eq!(grant["owner"], true);
    client::learn_grant(&w.o.config, &w.s.eid(), Some(&grant), false);
    let mine = client::servers(&w.o.config);
    let entry = server_of(&mine, &w.s.eid()).unwrap();
    assert!(entry.owner && !entry.use_it && !entry.hold_for_me, "a server's 'you own me' changes nothing by itself: {entry:?}");
    assert_eq!(entry.offer, "share", "the owner is asked once whether friends may use it");
    // Nothing is told to friends (or used as our inbox) before the owner says yes.
    let fields = super::hello_fields(&w.o.config, w.o.ep.secret_key(), &w.f.eid());
    assert!(fields.get("shared").is_none());
    assert_eq!(fields["inbox"], json!([]));
    client::set_prefs_full(&w.o.config, &w.s.eid(), None, None, Some("seen"), Some(true)).unwrap();
    let entry = client::servers(&w.o.config).into_iter().find(|s| s.eid == w.s.eid()).unwrap();
    assert!(entry.use_it && entry.hold_for_me && entry.share_friends, "yes = it's ours: {entry:?}");
    // F and O swap hellos (keys, inbox servers, and now "shared").
    introduce(&w.o, &w.f);
    introduce(&w.f, &w.o);
    assert_eq!(keys::peers(&w.f.config)[&w.o.eid()].shared.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_friend_of_the_owner_gets_the_offer_can_deposit_and_register_push() {
    let w = owner_world("offer").await;
    owner_shares(&w).await;
    // Before O told S who its friends are, S turns F down and F shows nothing.
    assert!(!client::check_introduced(&w.f.state, &w.f.config).await);
    assert!(client::servers(&w.f.config).is_empty(), "no offer until the server agrees");
    // O's device vouches for its friends.
    assert!(client::push_members(&w.o.state, &w.o.config).await);
    let c = server::load_config(&w.s.config);
    assert!(server::rights_for(&w.s.config, &c, &w.f.eid()).member);
    assert!(!client::push_members(&w.o.state, &w.o.config).await, "nothing new → nothing sent");
    // F asks S and gets a one-time offer, labeled with who shared it.
    client::recheck_introduced(&w.s.eid());
    assert!(client::check_introduced(&w.f.state, &w.f.config).await);
    let view = client::servers_view(&w.f.config);
    let entry = server_of(&view, &w.s.eid()).unwrap();
    assert!(entry.member && !entry.owner && !entry.through && !entry.revoked, "{entry:?}");
    assert_eq!(entry.offer, "new");
    assert_eq!(entry.via, vec![w.o.eid()]);
    assert_eq!(entry.via_peer.as_deref(), Some(w.o_on_f.as_str()));
    assert_eq!(entry.via_name.as_deref(), Some("Ashton"));
    // The owner's page lists F (vouched), even with nothing held yet.
    let st = server::status(&w.s.config);
    assert!(st.people.iter().any(|p| p.name == "Fay" && p.via_owner), "{:?}", st.people);
    assert_eq!(st.owner.as_ref().map(|o| o.sharing), Some(1));

    // "Turn On": hold my messages there too → registers push.
    client::set_prefs(&w.f.config, &w.s.eid(), Some(true), Some(true), Some("seen")).unwrap();
    assert!(client::push_register(&w.f.ep, &w.s.eid(), &seal::b64(b"sealed-apns-token")).await, "a vouched friend may register for push");

    // F → O while O is offline: held on O's own server, O picks it up.
    introduce(&w.o, &w.f);
    let m = uuid::Uuid::new_v4().to_string();
    let held = client::deposit_chat(&w.f.state, &w.f.config, &w.o_on_f, "chat", &chat_frame(&m, "see you at 8"), Some(&m)).await.unwrap();
    assert_eq!(held.server, w.s.eid());
    let got = tokio::time::timeout(Duration::from_secs(30), client::fetch_from(&w.o.state, &w.o.config, &w.s.eid())).await.unwrap().unwrap();
    assert_eq!(got, 1);
    assert_eq!(crate::chat::messages(&w.o.config, &w.f_on_o)[0].text, "see you at 8");

    // O → F while F is offline: held on the same server (F's inbox now).
    introduce(&w.f, &w.o);
    let m2 = uuid::Uuid::new_v4().to_string();
    let held = client::deposit_chat(&w.o.state, &w.o.config, &w.f_on_o, "chat", &chat_frame(&m2, "ok!"), Some(&m2)).await.unwrap();
    assert_eq!(held.server, w.s.eid());
    let got = tokio::time::timeout(Duration::from_secs(30), client::fetch_from(&w.f.state, &w.f.config, &w.s.eid())).await.unwrap().unwrap();
    assert_eq!(got, 1);
    assert_eq!(crate::chat::messages(&w.f.config, &w.o_on_f).iter().filter(|m| m.text == "ok!").count(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn strangers_and_friends_cannot_vouch_or_use_the_owners_server() {
    let w = owner_world("stranger").await;
    owner_shares(&w).await;
    assert!(client::push_members(&w.o.state, &w.o.config).await);
    // A stranger: no deposit, no hello, no push registration.
    let x = node(&w.base, "x").await;
    crate::friends::upsert_by_endpoint(&x.config, &w.o.eid(), "Ashton");
    introduce(&w.o, &x);
    client::set_server_for_tests(&x.config, UsableServer { eid: w.s.eid(), name: "Linux Box".into(), member: true, use_it: true, ..Default::default() });
    let o_on_x = crate::friends::chat_sender(&x.config, &w.o.eid()).unwrap().id;
    let m = uuid::Uuid::new_v4().to_string();
    let e = client::deposit_chat(&x.state, &x.config, &o_on_x, "chat", &chat_frame(&m, "spam"), Some(&m)).await.unwrap_err();
    assert!(matches!(&e, DepositError::Refused { reason, .. } if reason == "denied"), "{e:?}");
    assert!(matches!(client::server_hello_reply(&x.ep, &w.s.eid()).await, client::HelloReply::Refused));
    assert!(!client::push_register(&x.ep, &w.s.eid(), &seal::b64(b"t")).await);
    // Nobody but the owner's devices may say who the owner's friends are: not a
    // stranger, not a vouched friend, not someone who merely CLAIMS the account.
    let forged = json!({"kind": "mailbox.members", "people": [{"key": x.eid(), "name": "X", "devices": [{"eid": x.eid(), "since": 1}]}],
        "account_pub": crate::link::account_pub(&w.o.config).unwrap()});
    for n in [&x, &w.f] {
        let conn = n.ep.connect(iroh_net::dial_addr(w.s.ep.id()), iroh_net::ALPN).await.unwrap();
        assert_eq!(rpc(&conn, forged.clone()).await["reason"], "denied");
    }
    let c = server::load_config(&w.s.config);
    assert!(!server::rights_for(&w.s.config, &c, &x.eid()).any());
    // The account isn't proven by a claim: a hello with a bad signature changes nothing.
    crate::friends::upsert_by_endpoint(&w.s.config, &x.eid(), "Mallory");
    crate::friends::apply_device_hello(&w.s.config, &x.eid(), &json!({"device_os": "ios",
        "account_pub": crate::link::account_pub(&w.o.config).unwrap(), "account_sig": hex::encode([0u8; 64])}));
    assert!(!server::rights_for(&w.s.config, &c, &x.eid()).owner);
    x.listener.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removed_blocked_and_denied_friends_of_the_owner_lose_access() {
    let w = owner_world("revoke").await;
    owner_shares(&w).await;
    // A second friend G, and a second owner device O2 later removed from the account.
    let g = iroh::SecretKey::generate().public().to_string();
    crate::friends::upsert_by_endpoint(&w.o.config, &g, "Gil");
    assert!(client::push_members(&w.o.state, &w.o.config).await);
    let c = server::load_config(&w.s.config);
    assert!(server::rights_for(&w.s.config, &c, &w.f.eid()).member);
    assert!(server::rights_for(&w.s.config, &c, &g).member);
    client::recheck_introduced(&w.s.eid());
    client::check_introduced(&w.f.state, &w.f.config).await;
    assert!(server_of(&client::servers(&w.f.config), &w.s.eid()).is_some_and(|s| s.usable()));

    // O blocks G: G is out on the server at the next list.
    let g_id = crate::friends::chat_sender(&w.o.config, &g).unwrap().id;
    crate::block::block_friend(&w.o.config, &g_id).unwrap();
    assert!(client::push_members(&w.o.state, &w.o.config).await);
    assert!(!server::rights_for(&w.s.config, &c, &g).any(), "blocked by the owner = no access");

    // O removes F: F can't deposit any more, and its app shows the server as gone.
    let f_rec = crate::friends::get(&w.o.config, &w.f_on_o).unwrap();
    crate::account::record_friend_removed(&w.o.config, &f_rec);
    crate::friends::remove(&w.o.config, &w.f_on_o).unwrap();
    assert!(client::push_members(&w.o.state, &w.o.config).await);
    assert!(!server::rights_for(&w.s.config, &c, &w.f.eid()).any());
    let m = uuid::Uuid::new_v4().to_string();
    let e = client::deposit_chat(&w.f.state, &w.f.config, &w.o_on_f, "chat", &chat_frame(&m, "hello?"), Some(&m)).await.unwrap_err();
    assert!(matches!(&e, DepositError::Refused { reason, .. } if reason == "denied" || reason == "recipient"), "{e:?}");
    client::recheck_introduced(&w.s.eid());
    client::check_introduced(&w.f.state, &w.f.config).await;
    assert!(server_of(&client::servers(&w.f.config), &w.s.eid()).is_some_and(|s| s.revoked), "shows as no longer available");

    // An older list from another owner device that still lists F doesn't bring
    // F back: the removal is newer than the friendship.
    let o2 = node(&w.base, "o2").await;
    crate::link::adopt_key_for_tests(&o2.config, &w.account_key);
    crate::friends::upsert_by_endpoint(&w.s.config, &o2.eid(), "Ashton");
    prove_account(&w.s, &o2.eid(), &w.account_key);
    crate::friends::upsert_by_endpoint(&o2.config, &w.f.eid(), "Fay");
    let mut stale = super::members::build(&o2.config);
    for p in stale["people"].as_array_mut().unwrap() {
        for d in p["devices"].as_array_mut().unwrap() {
            d["since"] = json!(1); // befriended long before O removed them
        }
    }
    let conn = o2.ep.connect(iroh_net::dial_addr(w.s.ep.id()), iroh_net::ALPN).await.unwrap();
    assert_eq!(rpc(&conn, stale).await["ok"], true, "another owner device may share its list");
    assert!(!server::rights_for(&w.s.config, &c, &w.f.eid()).any(), "a removal on one owner device wins over a stale list");

    // O removes O2 from the account: O2 no longer counts as the owner there.
    crate::account::mark_linked(&w.o.config, &o2.eid());
    assert!(server::rights_for(&w.s.config, &c, &o2.eid()).owner);
    std::thread::sleep(Duration::from_millis(5));
    {
        let path = w.o.config.join("account-state.json");
        let mut book: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        book["removed_devices"][o2.eid()] = json!(crate::chat::now_ms() + 1000);
        std::fs::write(&path, serde_json::to_vec(&book).unwrap()).unwrap();
    }
    assert!(client::push_members(&w.o.state, &w.o.config).await);
    assert!(!server::rights_for(&w.s.config, &c, &o2.eid()).owner, "a device removed from the owner's account isn't the owner");
    // The removed device can't lock the real owner device out by reporting it
    // "removed" (even far in the future): O keeps managing the server.
    let conn = o2.ep.connect(iroh_net::dial_addr(w.s.ep.id()), iroh_net::ALPN).await.unwrap();
    let lie = json!({"kind": "mailbox.members", "people": [], "unlinked": {w.o.eid(): u64::MAX}});
    assert_eq!(rpc(&conn, lie).await["ok"], true);
    assert!(server::rights_for(&w.s.config, &c, &w.o.eid()).owner, "a disputed report never locks a device out");
    // …nor can it re-link itself: its own "I joined just now" doesn't count.
    let conn = o2.ep.connect(iroh_net::dial_addr(w.s.ep.id()), iroh_net::ALPN).await.unwrap();
    let relink = json!({"kind": "mailbox.members", "people": [], "linked": {o2.eid(): u64::MAX}, "unlinked": {w.o.eid(): u64::MAX}});
    assert_eq!(rpc(&conn, relink).await["ok"], true);
    assert!(server::rights_for(&w.s.config, &c, &w.o.eid()).owner, "a removed device can't lock the owner out by re-linking itself");
    assert_eq!(super::members::disputed_devices(&w.s.config, &c), 2, "the server page shows the dispute");
    let lists = super::members::lists_for_tests(&w.s.config);
    assert!(lists[&o2.eid()].unlinked[&w.o.eid()] <= crate::chat::now_ms() + super::server::DAY_MS, "times are clamped");

    // The server's own "Remove" (person id "v:…") beats the owner's list.
    std::thread::sleep(Duration::from_millis(20));
    crate::friends::upsert_by_endpoint(&w.o.config, &w.f.eid(), "Fay again");
    assert!(client::push_members(&w.o.state, &w.o.config).await);
    let person = super::members::vouched(&w.s.config, &c, &w.f.eid()).expect("re-added").person;
    let mut c2 = server::load_config(&w.s.config);
    c2.denied.push(person);
    server::save_config(&w.s.config, &c2).unwrap();
    assert!(!server::rights_for(&w.s.config, &c2, &w.f.eid()).any());
    o2.listener.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_heard_about_from_a_friend_is_never_treated_as_ours() {
    let w = owner_world("claim").await;
    owner_shares(&w).await;
    assert!(client::push_members(&w.o.state, &w.o.config).await);
    client::recheck_introduced(&w.s.eid());
    client::check_introduced(&w.f.state, &w.f.config).await;
    // Even if the server claimed F owns it, F's app would not believe it.
    let fake = client::HelloReply::Ok(json!({"ok": true, "name": "Your Box", "rights": {"own": true, "owner": true, "member": true, "through": true}}));
    client::learn_intro(&w.f.config, &w.s.eid(), &fake, &[w.o.eid()]);
    let e = client::servers(&w.f.config).into_iter().find(|s| s.eid == w.s.eid()).unwrap();
    assert!(!e.owner && !e.own && e.offer != "share", "{e:?}");
    assert!(client::set_prefs_full(&w.f.config, &w.s.eid(), None, None, None, Some(true)).is_err(), "only an owner can share");
    assert!(client::my_shared(&w.f.config).is_empty());
    // When O stops sharing it, F's app shows it as gone.
    client::set_prefs_full(&w.o.config, &w.s.eid(), None, None, None, Some(false)).unwrap();
    introduce(&w.o, &w.f);
    client::check_introduced(&w.f.state, &w.f.config).await;
    let e = client::servers(&w.f.config).into_iter().find(|s| s.eid == w.s.eid()).unwrap();
    assert!(e.revoked && e.via.is_empty(), "{e:?}");
}

// ── T11 (2026-10-04 audit) ──────────────────────────────────────────────────

/// A held file whose record can't be read at startup is set aside, never deleted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t11_unreadable_held_file_is_quarantined_not_deleted() {
    let w = world("t11-quarantine").await;
    let src = w.base.join("src/keep.bin");
    write(&src, 5000, 9);
    let xfer = uuid::Uuid::new_v4().to_string();
    client::deposit_files(&w.a.state, &w.a.config, &w.b_for_a, &xfer, &xfer, &deposit_files_of(&[(src, "keep.bin")]), &[], &["keep.bin".into()],
        &|_, _| {}, &|_| {}, &AtomicBool::new(false), None).await.unwrap();
    let root = server::root(&w.s.config, &server::load_config(&w.s.config)).unwrap();
    let item = walk(&root.join("items")).into_iter().find(|p| p.ends_with("item.json")).unwrap();
    std::fs::write(&item, b"{torn").unwrap();
    server::unload(&w.s.config);
    assert_eq!(server_items(&w), 0);
    let kept = walk(&root.join("quarantine"));
    assert!(kept.iter().any(|p| p.ends_with("payload")), "the payload is set aside: {kept:?}");
}

/// An upload nobody resumed for hours releases its space (not after 7 days).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t11_abandoned_upload_frees_its_space_after_idle() {
    let w = world("t11-idle").await;
    let src = w.base.join("src/big.bin");
    write(&src, (seal::SEG as usize) * 4, 3);
    let files = deposit_files_of(&[(src, "big.bin")]);
    let xfer = uuid::Uuid::new_v4().to_string();
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    let progress = move |d: u64, _t: u64| { if d >= seal::SEG { c2.store(true, Ordering::SeqCst); } };
    let _ = client::deposit_files(&w.a.state, &w.a.config, &w.b_for_a, &xfer, &xfer, &files, &[], &["big.bin".into()],
        &progress, &|_| {}, &cancel, None).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(server_items(&w), 1, "a paused upload stays resumable");
    assert_eq!(server::gc(&w.s.config), 0, "a fresh partial is kept");
    let root = server::root(&w.s.config, &server::load_config(&w.s.config)).unwrap();
    let part = walk(&root.join("items")).into_iter().find(|p| p.ends_with("payload.part")).unwrap();
    iroh_net::set_mtime_secs(&part, (crate::chat::now_ms() / 1000).saturating_sub(7 * 3600));
    server::age_all_for_tests(&w.s.config, 7 * 3600 * 1000);
    assert_eq!(server::gc(&w.s.config), 1);
    assert_eq!(server_items(&w), 0);
}

/// Modified times survive the trip; a chat queued after a file is listed first;
/// a device the owner removed can't collect anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t11_mtime_chat_first_and_removed_device() {
    let w = world("t11-misc").await;
    let src = w.base.join("src/photo.jpg");
    write(&src, 3000, 5);
    iroh_net::set_mtime_secs(&src, 1_600_000_000);
    let xfer = uuid::Uuid::new_v4().to_string();
    client::deposit_files(&w.a.state, &w.a.config, &w.b_for_a, &xfer, &xfer, &deposit_files_of(&[(src, "photo.jpg")]), &[], &["photo.jpg".into()],
        &|_, _| {}, &|_| {}, &AtomicBool::new(false), None).await.unwrap();
    let m = uuid::Uuid::new_v4().to_string();
    client::deposit_chat(&w.a.state, &w.a.config, &w.b_for_a, "chat", &chat_frame(&m, "after the file"), Some(&m)).await.unwrap();
    let conn = w.b.ep.connect(iroh_net::dial_addr(w.s.ep.id()), iroh_net::ALPN).await.unwrap();
    let list = rpc(&conn, json!({"kind": "mailbox.fetch", "v": super::VERSION})).await;
    assert_eq!(list["items"][0]["kind"], "chat", "{list}");
    // The owner denies B's device: it can no longer fetch.
    let mut c = server::load_config(&w.s.config);
    c.denied.push(format!("e:{}", w.b.eid()));
    server::save_config(&w.s.config, &c).unwrap();
    let refused = rpc(&conn, json!({"kind": "mailbox.fetch", "v": super::VERSION})).await;
    assert_eq!(refused["reason"], "denied", "{refused}");
    conn.close(0u32.into(), b"done");
    c.denied.clear();
    server::save_config(&w.s.config, &c).unwrap();
    assert_eq!(fetch(&w).await, 2);
    let landed = std::fs::metadata(w.b.config.join("Downloads/photo.jpg")).unwrap();
    assert_eq!(iroh_net::mtime_secs(&landed), 1_600_000_000);
    assert!(walk(&w.b.config.join("mailbox-in")).is_empty(), "no ciphertext left behind");
}

/// Startup sweep: week-old partial ciphertext and day-old landing stages go;
/// anything else stays.
#[test]
fn t11_startup_sweep_removes_only_stale_leftovers() {
    let base = std::env::temp_dir().join(format!("dropbeam-mbx-sweep-{}", uuid::Uuid::new_v4()));
    let (config, dl) = (base.join("cfg"), base.join("dl"));
    std::fs::create_dir_all(config.join("mailbox-in")).unwrap();
    std::fs::create_dir_all(dl.join("Folder")).unwrap();
    let old = (crate::chat::now_ms() / 1000).saturating_sub(8 * 24 * 3600);
    let mk = |p: PathBuf, stale: bool| { std::fs::write(&p, b"x").unwrap(); if stale { iroh_net::set_mtime_secs(&p, old); } p };
    let stale_ct = mk(config.join("mailbox-in/a.ct"), true);
    let fresh_ct = mk(config.join("mailbox-in/b.ct"), false);
    let stale_part = mk(dl.join("Folder/.dropbeam-mbx-1.part"), true);
    let fresh_part = mk(dl.join(".dropbeam-mbx-2.part"), false);
    let user = mk(dl.join("notes.txt"), true);
    client::sweep_leftovers(&config, &dl);
    assert!(!stale_ct.exists() && !stale_part.exists());
    assert!(fresh_ct.exists() && fresh_part.exists() && user.exists());
    let _ = std::fs::remove_dir_all(base);
}
