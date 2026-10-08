use super::*;

fn dir() -> PathBuf {
    let p = std::env::temp_dir().join(format!("db-recovery-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&p).unwrap();
    p
}
fn eid() -> String {
    iroh::SecretKey::generate().public().to_string()
}
fn text(id: &str, peer: &str, from_me: bool, ts: u64) -> chat::ChatMessage {
    serde_json::from_value(json!({"id": id, "peerId": peer, "fromMe": from_me, "kind": "text",
        "text": format!("m{id}"), "files": [], "bytes": 0, "status": if from_me { Some("delivered") } else { None }, "ts": ts, "seq": ts,
        "reactions": [{"emoji": "👍", "fromMe": !from_me}]})).unwrap()
}
/// `dir` has `who` as an accepted friend proving `account`.
fn befriend(dir: &Path, who: &str, name: &str, account: Option<&str>) -> crate::models::Friend {
    let f = friends::upsert_by_endpoint(dir, who, name);
    friends::set_device_info(dir, who, Some("laptop"), account);
    f
}
/// A hello from a device of `account` (its key) at endpoint `eid`.
fn hello_from(key: &iroh::SecretKey, eid: &str, extra: serde_json::Map<String, Value>) -> Value {
    let mut v = json!({"kind": "friend-hello", "account_pub": hex::encode(key.public().as_bytes()),
        "account_sig": hex::encode(key.sign(eid.as_bytes()).to_bytes()), "device_os": "macos", "device_kind": "laptop"});
    v.as_object_mut().unwrap().extend(extra);
    v
}

// ── words ───────────────────────────────────────────────────────────────────

#[test]
fn bip39_reference_vectors() {
    // Trezor's published vectors (the passphrase-free part of BIP39).
    assert_eq!(encode(&[0u8; 16]).join(" "), "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about");
    assert_eq!(encode(&[0x7f; 16]).join(" "), "legal winner thank year wave sausage worth useful legal winner thank yellow");
    assert_eq!(encode(&[0xff; 16]).join(" "), "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong");
    assert_eq!(encode(&[0u8; 32]).join(" "), format!("{} art", ["abandon"; 23].join(" ")));
    let v = hex::decode("9e885d952ad362caeb4efe34a8e91bd2").unwrap();
    assert_eq!(encode(&v).join(" "), "ozone drill grab fiber curtain grace pudding thank cruise elder eight picnic");
}

#[test]
fn words_round_trip_and_forgive_how_they_were_typed() {
    for len in [16usize, 32] {
        let e: Vec<u8> = (0..len).map(|_| rand::random()).collect();
        let w = encode(&e);
        assert_eq!(w.len(), if len == 16 { 12 } else { 24 });
        assert_eq!(decode(&w.join(" ")).unwrap().as_slice(), e.as_slice());
        // Numbered, upper case, commas, line breaks, the QR form, 4-letter starts.
        let numbered: String = w.iter().enumerate().map(|(i, x)| format!("{}. {},\n", i + 1, x.to_uppercase())).collect();
        assert_eq!(decode(&numbered).unwrap().as_slice(), e.as_slice());
        assert_eq!(decode(&format!("{QR_PREFIX}{}", w.join(" "))).unwrap().as_slice(), e.as_slice());
        let short: Vec<String> = w.iter().map(|x| x.chars().take(4).collect()).collect();
        assert_eq!(decode(&short.join(" ")).unwrap().as_slice(), e.as_slice());
    }
}

#[test]
fn mistakes_are_caught_and_explained() {
    let w = encode(&[0x7f; 16]);
    // A swapped word fails the checksum.
    let mut swapped = w.clone();
    swapped.swap(0, 1);
    assert_eq!(decode(&swapped.join(" ")).unwrap_err(), WRONG_WORDS);
    let c = check(&swapped.join(" "));
    assert!(c.complete && !c.valid && c.problem.as_deref() == Some(WRONG_WORDS));
    // An unknown word is pointed at by its number.
    let mut typo = w.clone();
    typo[4] = "wavv".into();
    let c = check(&typo.join(" "));
    assert_eq!(c.unknown, vec![4]);
    assert!(!c.complete && c.problem.unwrap().contains("Word 5"));
    // Too few words: no complaint while typing, a clear one on restore.
    let c = check("legal winner thank");
    assert_eq!((c.count, c.complete, c.valid, c.problem), (3, false, false, None));
    assert!(decode("legal winner thank").unwrap_err().contains("3 were entered"));
    // Ambiguous starts (fewer than 4 letters) aren't guessed.
    assert_eq!(lookup("aba"), None);
    assert_eq!(lookup("aban"), Some(0));
    assert_eq!(lookup("zoo"), Some(2047));
    let c = check(&w.join(" "));
    assert!(c.valid && c.problem.is_none());
}

// ── the account behind the words ───────────────────────────────────────────

#[test]
fn a_new_account_has_12_words_that_restore_the_same_account() {
    let (a, b) = (dir(), dir());
    let words = code_words(&a).unwrap();
    assert_eq!(words.len(), 12, "a new account is minted from 12 words");
    let account = crate::account::my_pub(&a).unwrap();
    assert_eq!(code_words(&a).unwrap().as_slice(), words.as_slice(), "the same words every time");
    let restored = restore_from_words(&b, &eid(), &words.join(" ")).unwrap();
    assert_eq!(restored, account);
    assert_eq!(crate::account::my_pub(&b).as_deref(), Some(account.as_str()));
    assert_eq!(code_words(&b).unwrap().as_slice(), words.as_slice(), "the restored device shows the same 12 words");
    assert!(status(&b).saved, "restoring proves the user has the code");
    assert!(restore_active(&b));
    for d in [a, b] { let _ = std::fs::remove_dir_all(d); }
}

#[test]
fn an_older_account_has_24_words_and_a_stale_seed_is_never_shown() {
    let (a, b) = (dir(), dir());
    let key = iroh::SecretKey::generate();
    crate::link::adopt_key_for_tests(&a, &key);
    // A seed left over from another account must not produce words for it.
    std::fs::write(a.join("account.seed"), [9u8; 16]).unwrap();
    let words = code_words(&a).unwrap();
    assert_eq!(words.len(), 24);
    restore_from_words(&b, &eid(), &words.join(" ")).unwrap();
    assert_eq!(crate::account::my_pub(&b), Some(hex::encode(key.public().as_bytes())));
    assert!(!b.join("account.seed").exists());
    for d in [a, b] { let _ = std::fs::remove_dir_all(d); }
}

#[test]
fn restore_is_refused_where_it_would_break_something() {
    let (a, b) = (dir(), dir());
    let words = code_words(&a).unwrap().join(" ");
    // Same account already here.
    assert_eq!(restore_from_words(&a, &eid(), &words).unwrap_err(), ALREADY_THIS);
    // A device sharing ANOTHER account with its own devices must leave first.
    let other = code_words(&b).unwrap();
    let account_b = crate::account::my_pub(&b).unwrap();
    friends::upsert_own_device(&b, &eid(), "iPhone", Some("phone"), Some("ios"), None, &account_b, chat::now_ms(), true);
    assert_eq!(restore_from_words(&b, &eid(), &words).unwrap_err(), LINKED_ELSEWHERE);
    assert_eq!(code_words(&b).unwrap().as_slice(), other.as_slice(), "nothing changed");
    // Bad words change nothing either.
    let c = dir();
    assert!(restore_from_words(&c, &eid(), "legal winner").is_err());
    assert!(crate::account::my_pub(&c).is_none());
    for d in [a, b, c] { let _ = std::fs::remove_dir_all(d); }
}

#[test]
fn saving_is_confirmed_only_with_the_right_words_and_only_for_this_account() {
    let a = dir();
    let words = code_words(&a).unwrap();
    assert!(!status(&a).saved);
    assert!(confirm_saved(&a, &[Answer { index: 2, word: "nope".into() }]).is_err());
    assert!(confirm_saved(&a, &[]).is_err());
    confirm_saved(&a, &[Answer { index: 2, word: words[2].to_uppercase() }, Answer { index: 8, word: words[8].clone() }]).unwrap();
    assert!(status(&a).saved);
    // The device moves to another account (linked elsewhere): that code isn't saved.
    crate::link::adopt_key_for_tests(&a, &iroh::SecretKey::generate());
    assert!(!status(&a).saved);
    let _ = std::fs::remove_dir_all(a);
}

#[test]
fn a_restored_device_starts_alone_so_old_devices_need_approval() {
    let (a, b) = (dir(), dir());
    let words = code_words(&a).unwrap().join(" ");
    let account = crate::account::my_pub(&a).unwrap();
    let (old, me) = (eid(), eid());
    restore_from_words(&b, &me, &words).unwrap();
    // The old device turns up proving the key: it isn't one of ours yet.
    let key = crate::link::account_key(&b).unwrap();
    friends::apply_device_hello(&b, &old, &hello_from(&key, &old, Default::default()));
    assert!(!crate::account::is_own_device(&b, &old));
    assert!(crate::account::is_removed_device(&b, &account, &old), "not vouched for");
    for d in [a, b] { let _ = std::fs::remove_dir_all(d); }
}

// ── friends coming back ────────────────────────────────────────────────────

/// P (the user, device `p_eid` in dir `p`) and F (a friend, device `f_eid` in
/// dir `f`) are friends with accounts; their hellos exchanged vouches.
struct Pair2 { p: PathBuf, f: PathBuf, p_eid: String, f_eid: String, p_key: iroh::SecretKey, f_key: iroh::SecretKey, words: String }

fn befriended() -> Pair2 {
    let (p, f) = (dir(), dir());
    let words = code_words(&p).unwrap().join(" ");
    code_words(&f).unwrap();
    let (p_key, f_key) = (crate::link::account_key(&p).unwrap(), crate::link::account_key(&f).unwrap());
    let (p_eid, f_eid) = (eid(), eid());
    let (pa, fa) = (crate::account::my_pub(&p).unwrap(), crate::account::my_pub(&f).unwrap());
    befriend(&p, &f_eid, "Fran", Some(&fa));
    befriend(&f, &p_eid, "Pat", Some(&pa));
    // P greets F: F keeps P's vouch.
    let fields = hello_fields(&p, &f_eid, None);
    assert!(fields.contains_key("vouch"));
    assert!(store_vouch(&f, &f_eid, &Value::Object(fields)));
    Pair2 { p, f, p_eid, f_eid, p_key, f_key, words }
}

#[test]
fn vouches_bring_former_friends_back_and_nobody_else() {
    let t = befriended();
    let r = dir();
    let r_eid = eid();
    restore_from_words(&r, &r_eid, &t.words).unwrap();
    let account = crate::account::my_pub(&r).unwrap();
    // F finds R (rendezvous) and greets it with the vouch it holds.
    let fields = hello_fields(&t.f, &r_eid, Some(&account));
    assert!(fields.contains_key("your_vouch"));
    let hello = hello_from(&t.f_key, &t.f_eid, fields.clone());
    assert_eq!(friends::apply_hello_from(&r, "", &t.f_eid, "Fran", &hello), friends::HelloOutcome::Returned);
    assert!(friends::chat_sender(&r, &t.f_eid).is_some(), "Fran is a friend again");
    // A stranger replaying F's vouch from another device gets a request.
    let stranger = eid();
    let hello = hello_from(&iroh::SecretKey::generate(), &stranger, fields);
    assert_eq!(friends::apply_hello_from(&r, "", &stranger, "Mallory", &hello), friends::HelloOutcome::Requested);
    // F's side: R proves P's account, so it's another device of Pat.
    let hello = hello_from(&t.p_key, &r_eid, Default::default());
    assert_eq!(friends::apply_hello_from(&t.f, "", &r_eid, "Pat", &hello), friends::HelloOutcome::AddedDevice);
    // A vouch for someone else's device isn't stored, and a request gets none.
    assert!(!store_vouch(&t.f, &eid(), &Value::Object(hello_fields(&t.p, &t.f_eid, None))));
    assert!(!hello_fields(&t.p, &stranger, None).contains_key("vouch"));
    for d in [t.p, t.f, r] { let _ = std::fs::remove_dir_all(d); }
}

#[test]
fn a_friend_sends_back_the_conversation_flipped_with_old_devices_and_folders() {
    let t = befriended();
    let r = dir();
    let r_eid = eid();
    let p_on_f = friends::chat_sender(&t.f, &t.p_eid).unwrap();
    for i in 0..10 { chat::append(&t.f, &text(&format!("m{i}"), &p_on_f.id, i % 3 == 0, 1000 + i)); }
    let pairs = json!([{"id": "pair1", "role": "a", "peerName": "Pat", "secret": "s", "folder": "/Users/fran/Shared/Vacation",
        "twoWay": true, "mirror": true, "autoDelete": false, "deleteMode": "trash", "createdAt": 1, "endpointId": t.p_eid}]);
    std::fs::write(crate::pairing::pairs_path(&t.f), serde_json::to_vec(&pairs).unwrap()).unwrap();
    restore_from_words(&r, &r_eid, &t.words).unwrap();
    friends::apply_hello_from(&t.f, "", &r_eid, "Pat", &hello_from(&t.p_key, &r_eid, Default::default()));
    friends::apply_device_hello(&t.f, &r_eid, &hello_from(&t.p_key, &r_eid, Default::default()));
    befriend(&r, &t.f_eid, "Fran", crate::account::my_pub(&t.f).as_deref());
    // R asks F.
    let req = json!({"kind": "restore-sync", "v": 1, "account_pub": crate::account::my_pub(&r), "account_sig": crate::link::sign_endpoint(&r, &r_eid)});
    let reply = serve_restore(&t.f, &r_eid, &req);
    assert_eq!(reply["kind"], "restore-sync-ok");
    assert_eq!(reply["folders"], json!(["Vacation"]), "only the folder's name, never F's path");
    let applied = apply_restore(&r, &r_eid, &t.f_eid, &reply).unwrap();
    assert_eq!(applied, Applied { messages: 10, devices: 1, folders: 1 });
    let fran = friends::chat_sender(&r, &t.f_eid).unwrap();
    let thread = chat::messages(&r, &fran.id);
    assert_eq!(thread.len(), 10);
    for m in &thread {
        let n: u64 = m.id[1..].parse().unwrap();
        assert_eq!(m.from_me, n % 3 != 0, "who sent what flips to this side");
        assert_eq!(m.reactions[0].from_me, !m.from_me);
        assert_eq!(m.status.as_deref(), m.from_me.then_some("delivered"));
    }
    let s = status(&r).restore.unwrap();
    assert_eq!(s.old_devices.len(), 1);
    assert_eq!(s.old_devices[0].endpoint_id, t.p_eid, "Pat's old device is listed from before");
    assert_eq!(s.folders, vec![FolderNote { name: "Vacation".into(), with: "Fran".into() }]);
    assert_eq!(s.friends_synced, 1);
    // Asking twice in a row is turned down (rate limit); the answer applied once.
    assert_eq!(serve_restore(&t.f, &r_eid, &req)["kind"], "restore-sync-no");
    assert!(due_friends(&r, chat::now_ms()).is_empty(), "nobody left to ask");
    for d in [t.p, t.f, r] { let _ = std::fs::remove_dir_all(d); }
}

#[test]
fn only_a_friends_account_gets_the_conversation() {
    let t = befriended();
    let p_on_f = friends::chat_sender(&t.f, &t.p_eid).unwrap();
    chat::append(&t.f, &text("m1", &p_on_f.id, true, 5));
    // Someone with another account (or no proof) gets nothing.
    let stranger = iroh::SecretKey::generate();
    let s_eid = eid();
    let req = json!({"kind": "restore-sync", "account_pub": hex::encode(stranger.public().as_bytes()),
        "account_sig": hex::encode(stranger.sign(s_eid.as_bytes()).to_bytes())});
    assert_eq!(serve_restore(&t.f, &s_eid, &req)["kind"], "restore-sync-no");
    let forged = json!({"kind": "restore-sync", "account_pub": hex::encode(t.p_key.public().as_bytes()), "account_sig": "00"});
    assert_eq!(serve_restore(&t.f, &eid(), &forged)["kind"], "restore-sync-no");
    // An answer from someone who isn't our friend is ignored on the restored side.
    let r = dir();
    restore_from_words(&r, &eid(), &t.words).unwrap();
    assert!(apply_restore(&r, &eid(), &eid(), &json!({"kind": "restore-sync-ok", "messages": []})).is_err());
    for d in [t.p, t.f, r] { let _ = std::fs::remove_dir_all(d); }
}

// ── rendezvous ─────────────────────────────────────────────────────────────

#[test]
fn rendezvous_record_names_devices_only_under_the_right_account() {
    let key = iroh::SecretKey::generate();
    let (a, b) = (eid(), eid());
    let payload = rendezvous_packet(&key, &[a.clone(), b.clone(), "junk".into()]).unwrap().to_relay_payload();
    assert_eq!(rendezvous_eids(&key.public(), &payload, chat::now_ms()), vec![a, b]);
    // Another account's key can't vouch for it; an old record is ignored.
    assert!(rendezvous_eids(&iroh::SecretKey::generate().public(), &payload, chat::now_ms()).is_empty());
    assert!(rendezvous_eids(&key.public(), &payload, chat::now_ms() + RV_MAX_AGE_MS + 3_600_000).is_empty());
    assert!(rendezvous_eids(&key.public(), b"garbage", chat::now_ms()).is_empty());
}

#[test]
fn friends_accounts_are_looked_up_but_not_our_own_or_blocked_ones() {
    let t = befriended();
    let fa = crate::account::my_pub(&t.f).unwrap();
    let targets = lookup_targets(&t.p);
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].0, fa);
    assert!(targets[0].1.contains(&t.f_eid));
    for d in [t.p, t.f] { let _ = std::fs::remove_dir_all(d); }
}

// ── over a real connection ─────────────────────────────────────────────────

/// The restored device asks a friend over a loopback iroh connection and gets
/// the conversation back.
#[tokio::test]
async fn restore_sync_over_loopback() {
    use iroh::endpoint::presets;
    let t = befriended();
    let server = iroh::Endpoint::builder(presets::N0).alpns(vec![iroh_net::ALPN.to_vec()]).bind().await.unwrap();
    let client = iroh::Endpoint::bind(presets::N0).await.unwrap();
    // The friend's device is the server; the restored device the client.
    let (f_eid, r_eid) = (server.id().to_string(), client.id().to_string());
    let p_account = crate::account::my_pub(&t.p).unwrap();
    let f_account = crate::account::my_pub(&t.f).unwrap();
    let p_on_f = friends::chat_sender(&t.f, &t.p_eid).unwrap();
    for i in 0..40 { chat::append(&t.f, &text(&format!("x{i}"), &p_on_f.id, i % 2 == 0, 10 + i)); }
    let r = dir();
    restore_from_words(&r, &r_eid, &t.words).unwrap();
    // F counts R as another device of Pat; R has F as a friend (in real life
    // through F's vouch — F's endpoint here is a fresh one, so added directly).
    friends::apply_hello_from(&t.f, "", &r_eid, "Pat", &hello_from(&t.p_key, &r_eid, Default::default()));
    friends::apply_device_hello(&t.f, &r_eid, &hello_from(&t.p_key, &r_eid, Default::default()));
    befriend(&r, &f_eid, "Fran", Some(&f_account));
    let _ = p_account;
    let addr = server.addr();
    let f_dir = t.f.clone();
    let served = tokio::spawn(async move {
        let conn = server.accept().await.unwrap().await.unwrap();
        let who = conn.remote_id().to_string();
        let (mut send, mut recv) = conn.accept_bi().await.unwrap();
        let req = iroh_net::read_frame(&mut recv).await.unwrap();
        assert_eq!(req["kind"], "restore-sync");
        let reply = serve_restore(&f_dir, &who, &req);
        iroh_net::write_frame(&mut send, &reply).await.unwrap();
        send.finish().unwrap();
        let _ = send.stopped().await;
    });
    let conn = client.connect(addr, iroh_net::ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    let reply = request_restore(&r, &r_eid, &mut send, &mut recv).await.unwrap();
    served.await.unwrap();
    let applied = apply_restore(&r, &r_eid, &f_eid, &reply).unwrap();
    assert_eq!(applied.messages, 40);
    let fran = friends::chat_sender(&r, &f_eid).unwrap();
    assert_eq!(chat::messages(&r, &fran.id).iter().filter(|m| m.from_me).count(), 20);
    for d in [t.p, t.f, r] { let _ = std::fs::remove_dir_all(d); }
}
