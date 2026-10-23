//! Audio and video calls between friends and your own devices (docs/CALLS.md).
//!
//! - Media: WebRTC in the call screen (the webview's RTCPeerConnection on the
//!   desktop, Google's WebRTC framework on iOS) — echo cancellation, jitter
//!   buffer, Opus, H.264/VP8, bandwidth estimation. DTLS-SRTP end to end.
//! - Path: the friend's iroh connection, never STUN/TURN on the internet. The
//!   WebRTC engine may only use a TURN server this app runs on 127.0.0.1 for the
//!   call (turn.rs); its packets ride QUIC datagrams to the other side (media.rs).
//! - Signaling: one `call.offer` stream per device of the person called, kept
//!   open for the whole call (`call.ring/answer/busy/decline/missed/cancel/taken/
//!   hangup` and mid-call frames). Identity is `conn.remote_id()`; only friends
//!   and own devices can ring this device, blocked people never can, ringing is
//!   rate-limited. The DTLS fingerprints travel only inside these authenticated
//!   streams, so the relay path can't swap keys.
//! - Recording happens on the device, only after the other person allowed it,
//!   and they see it while it runs (record.rs).

pub mod stun;
pub mod turn;
pub mod media;
pub mod signal;
pub mod sdp;
pub mod record;
pub mod commands;
pub mod wake;

#[cfg(test)]
mod lab_tests;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use crate::iroh_net::{read_frame_cap, write_frame, IrohState};
use signal::{Act, Call, Dir, End, Ev, Phase};

/// Frames on a call stream are small JSON; an SDP is a few KB.
const MAX_FRAME: usize = 96 * 1024;
/// Ringing a device is limited per caller device: a burst, then a slow trickle.
const RING_PER_MINUTE: usize = 5;
const RING_PER_HOUR: usize = 30;
/// Mid-call frames the two call screens exchange through the engine.
const PASS_KINDS: &[&str] = &["call.media", "call.record", "call.sdp"];
/// Shown when this device's call screen can't do WebRTC.
pub const UNSUPPORTED: &str = "Calls need a newer system web view on this computer (WebRTC isn’t available here).";

/// Connections carrying a call: never rotated or closed for housekeeping.
static IN_CALL: Mutex<Option<HashSet<usize>>> = Mutex::new(None);

pub(crate) fn uses_conn(stable_id: usize) -> bool {
    IN_CALL.lock().ok().and_then(|s| s.as_ref().map(|s| s.contains(&stable_id))).unwrap_or(false)
}
fn mark_conn(stable_id: usize, on: bool) {
    if let Ok(mut s) = IN_CALL.lock() {
        let set = s.get_or_insert_with(HashSet::new);
        if on { set.insert(stable_id); } else { set.remove(&stable_id); }
    }
}

pub fn now_ms() -> u64 { crate::chat::now_ms() }

/// Debug builds only: test media and auto-answer for unattended call tests
/// (DROPBEAM_CALL_FAKE_MEDIA=1, DROPBEAM_CALL_AUTOANSWER=1). Never in a release.
pub(crate) fn lab_flags() -> Option<Value> {
    if !cfg!(debug_assertions) { return None; }
    let on = |k: &str| std::env::var(k).is_ok_and(|v| v == "1");
    let (fake, auto) = (on("DROPBEAM_CALL_FAKE_MEDIA"), on("DROPBEAM_CALL_AUTOANSWER"));
    (fake || auto).then(|| json!({ "fakeMedia": fake, "autoAnswer": auto }))
}

struct LegIo {
    tx: mpsc::UnboundedSender<Option<Value>>,
    conn: Connection,
}

/// The one call this device is in.
struct Live {
    call: Call,
    tag: u64,
    user: String,
    pass: String,
    bridge: Option<media::Bridge>,
    name: String,
    /// The caller's offer (callee: handed to the call screen on accept).
    offer: Option<String>,
    legs: HashMap<String, LegIo>,
    /// Answered automatically (the two of them called each other at once).
    auto_accept: bool,
    /// Glare loser: ended without a history line.
    silent: bool,
    unsupported: bool,
    rec: record::Consent,
}

#[derive(Default)]
pub struct Calls {
    live: Mutex<Option<Live>>,
    last: Mutex<Option<Value>>,
    rate: Mutex<HashMap<String, VecDeque<Instant>>>,
    /// We dropped our call to this person because theirs to us won the glare:
    /// their call, arriving in a moment, is answered automatically.
    glare: Mutex<Option<(String, Instant)>>,
    /// The call screen can do WebRTC here (reported by the UI; false until then).
    capable: std::sync::atomic::AtomicBool,
    app: OnceLock<AppHandle>,
    config: OnceLock<PathBuf>,
    #[cfg(test)]
    pub(crate) events: Mutex<Vec<(String, Value)>>,
    /// The lab call node (labkit::call_node) gets every event here.
    sink: Mutex<Option<mpsc::UnboundedSender<(String, Value)>>>,
}

impl Calls {
    fn bind(&self, state: &IrohState) {
        if let Some(app) = state.app.get() { let _ = self.app.set(app.clone()); }
        if let Ok(c) = crate::iroh_net::location_config(state) { let _ = self.config.set(c); }
    }

    fn emit(&self, event: &str, payload: Value) {
        #[cfg(test)]
        self.events.lock().unwrap().push((event.to_string(), payload.clone()));
        if let Some(tx) = self.sink.lock().unwrap().as_ref() { let _ = tx.send((event.to_string(), payload.clone())); }
        if let Some(app) = self.app.get() { let _ = app.emit(event, payload); }
    }

    pub(crate) fn set_sink(&self, tx: mpsc::UnboundedSender<(String, Value)>) { *self.sink.lock().unwrap() = Some(tx); }

    pub fn set_capable(&self, yes: bool) {
        self.capable.store(yes, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn capable(&self) -> bool { self.capable.load(std::sync::atomic::Ordering::SeqCst) }

    fn snapshot(l: &Live) -> Value {
        let mut v = serde_json::to_value(&l.call).unwrap_or(Value::Null);
        v["name"] = json!(l.name);
        v["autoAccept"] = json!(l.auto_accept);
        // Ended only because the two of them called each other at once.
        v["glare"] = json!(l.silent);
        // Some device of theirs answered that it can't take calls.
        v["unsupported"] = json!(l.unsupported);
        v["recording"] = serde_json::to_value(&l.rec).unwrap_or(Value::Null);
        v
    }

    pub fn status(&self) -> Value {
        let live = self.live.lock().unwrap();
        json!({ "call": live.as_ref().map(Self::snapshot), "last": self.last.lock().unwrap().clone(), "capable": self.capable() })
    }

    fn rate_ok(&self, who: &str) -> bool {
        let mut rate = self.rate.lock().unwrap();
        let q = rate.entry(who.to_string()).or_default();
        let now = Instant::now();
        while q.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(3600)) { q.pop_front(); }
        let last_minute = q.iter().filter(|t| now.duration_since(**t) < Duration::from_secs(60)).count();
        if last_minute >= RING_PER_MINUTE || q.len() >= RING_PER_HOUR { return false; }
        q.push_back(now);
        true
    }

    /// Feed one event to the call `id` and carry out what it says.
    fn apply(self: &Arc<Self>, id: &str, ev: Ev) {
        let mut ended = None;
        {
            let mut guard = self.live.lock().unwrap();
            let Some(l) = guard.as_mut().filter(|l| l.call.id == id) else { return };
            let tick = matches!(ev, Ev::Tick(_));
            let before = serde_json::to_value(&l.call).ok();
            let acts = l.call.on(ev, now_ms());
            // Ticks change nothing most seconds; tell the screen only about changes.
            if acts.is_empty() && (tick || before == serde_json::to_value(&l.call).ok()) { return; }
            for a in acts {
                match a {
                    Act::Send(ep, frame) => { if let Some(leg) = l.legs.get(&ep) { let _ = leg.tx.send(Some(frame)); } }
                    Act::Close(ep) => {
                        if let Some(leg) = l.legs.remove(&ep) { let _ = leg.tx.send(None); mark_conn(leg.conn.stable_id(), false); }
                    }
                    Act::Media(ep) => {
                        if let (Some(leg), Some(b)) = (l.legs.get(&ep), l.bridge.as_ref()) {
                            mark_conn(leg.conn.stable_id(), true);
                            b.attach(leg.conn.clone());
                        }
                    }
                    Act::Ended(_) => {}
                }
            }
            let snap = Self::snapshot(l);
            if l.call.ended() {
                let done = guard.take().unwrap();
                for leg in done.legs.values() { let _ = leg.tx.send(None); mark_conn(leg.conn.stable_id(), false); }
                *self.last.lock().unwrap() = Some(snap.clone());
                ended = Some(done);
            }
            self.emit("call://state", snap);
        }
        if let Some(done) = ended {
            record::close_for_call(&done.call.id);
            if !done.silent { self.record_history(&done); }
        }
    }

    fn record_history(&self, l: &Live) {
        let Some(config) = self.config.get() else { return };
        let c = &l.call;
        // A call another of my devices took isn't mine to log; that device does.
        if c.end == Some(End::AnsweredElsewhere) { return; }
        let msg = crate::chat::ChatMessage {
            file_xfer_id: None, id: format!("call-{}", c.id), peer_id: c.person.clone(), from_me: c.dir == Dir::Out,
            kind: "call".into(), text: signal::history_text(c), files: vec![], bytes: 0, path: None, status: None,
            ts: c.started_ms, seq: crate::chat::next_seq(config, &c.person), reply_to: None, reply_preview: None,
            reactions: vec![], edited: false, deleted: false, gif: None, rev: 0, held_on: None, server_note: None,
            via: None, deliveries: vec![], link_preview: None, text_rev: 0, reaction_revs: vec![], from_agent: None,
            call: Some(crate::chat::CallInfo { video: c.video, outcome: serde_json::to_value(c.end).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default(),
                duration_ms: c.duration_ms().filter(|_| c.media_ms.is_some()).unwrap_or(0) }),
        };
        if crate::chat::append(config, &msg) {
            self.emit("chat://message", serde_json::to_value(&msg).unwrap_or(Value::Null));
            if c.dir == Dir::In && c.end == Some(End::Missed) {
                if let Some(app) = self.app.get() {
                    let what = if c.video { "Missed video call" } else { "Missed audio call" };
                    #[cfg(desktop)]
                    crate::chat_notify::show(app, &l.name, what, &c.person);
                }
            }
        }
    }
}

fn calls(state: &IrohState) -> Arc<Calls> {
    state.calls.bind(state);
    state.calls.clone()
}

fn turn_info(l: &Live) -> Value {
    let port = l.bridge.as_ref().map(|b| b.port).unwrap_or(0);
    json!({ "urls": [format!("turn:127.0.0.1:{port}?transport=udp")], "username": l.user, "credential": l.pass })
}

fn valid_id(s: &str) -> bool { !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') }

/// Where a person can be rung: every device of theirs (or the one own device).
fn devices_for(config: &std::path::Path, friend_id: &str) -> Result<(crate::models::Friend, Vec<String>)> {
    let owner = crate::friends::thread_owner(config, friend_id).context("Friend not found.")?;
    let eps: Vec<String> = crate::friends::person_endpoints(config, &owner.id).into_iter()
        .filter(|e| !crate::block::is_blocked(config, e)).collect();
    anyhow::ensure!(!eps.is_empty(), "This friend needs to open DropBeam once before you can call them.");
    Ok((owner, eps))
}

// ───────────────────────── caller ─────────────────────────

/// Start a call: set up the bridge; the call screen then makes its offer
/// (`send_offer`), which rings every device of the person.
pub async fn start(state: &IrohState, friend_id: &str, video: bool) -> Result<Value> {
    let calls = calls(state);
    let config = calls.config.get().cloned().context("DropBeam is still starting")?;
    if crate::mailbox::is_headless() { anyhow::bail!("Calls need the DropBeam app."); }
    let (owner, eps) = devices_for(&config, friend_id)?;
    let id = uuid::Uuid::new_v4().to_string();
    let (user, pass) = (hex::encode(rand::random::<[u8; 12]>()), hex::encode(rand::random::<[u8; 24]>()));
    let tag = rand::random::<u64>();
    let bridge = media::Bridge::start(&user, &pass, true, tag).await.context("start the call bridge")?;
    let mut guard = calls.live.lock().unwrap();
    anyhow::ensure!(guard.is_none(), "You're already in a call.");
    let l = Live { call: Call::outgoing(&id, &owner.id, video, &eps, now_ms()), tag, user, pass, bridge: Some(bridge),
        name: owner.name.clone(), offer: None, legs: HashMap::new(), auto_accept: false, silent: false, unsupported: false, rec: Default::default() };
    let out = json!({ "callId": id, "turn": turn_info(&l), "call": Calls::snapshot(&l) });
    calls.emit("call://state", Calls::snapshot(&l));
    *guard = Some(l);
    drop(guard);
    spawn_ticker(calls.clone(), id);
    Ok(out)
}

/// The caller's offer is ready: ring every device.
pub async fn send_offer(state: &Arc<IrohState>, id: &str, offer: &str) -> Result<()> {
    let calls = calls(state);
    let offer = sdp::outgoing(offer)?;
    let (eps, tag, video) = {
        let mut guard = calls.live.lock().unwrap();
        let l = guard.as_mut().filter(|l| l.call.id == id && l.call.dir == Dir::Out).context("This call has ended.")?;
        anyhow::ensure!(l.offer.is_none(), "Already ringing.");
        l.offer = Some(offer.clone());
        (l.call.legs.iter().map(|g| g.endpoint.clone()).collect::<Vec<_>>(), l.tag, l.call.video)
    };
    for ep in eps {
        let (state, calls, id, offer) = (state.clone(), calls.clone(), id.to_string(), offer.clone());
        tauri::async_runtime::spawn(async move {
            let frame = json!({ "kind": "call.offer", "v": 1, "callId": id, "tag": format!("{tag:016x}"), "video": video, "sdp": offer });
            let mut opened = open_leg(&state, &ep, &frame).await;
            // Asleep (an iPhone in the background)? Wake it with a push and keep
            // redialing while the call still rings.
            if opened.is_err() && wake::wake(&state, &ep, &id, video).await {
                for _ in 0..12 {
                    let ringing = calls.live.lock().unwrap().as_ref().is_some_and(|l| l.call.id == id && l.call.phase == Phase::Ringing);
                    if !ringing { break; }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    opened = open_leg(&state, &ep, &frame).await;
                    if opened.is_ok() { break; }
                }
            }
            match opened {
                Ok((conn, send, recv)) => run_leg(calls, id, ep, conn, send, recv).await,
                Err(e) => {
                    log::info!("call: couldn't ring {}: {e:#}", crate::telemetry::redact_paths_only(&ep[..ep.len().min(10)]));
                    calls.apply(&id, Ev::Lost(ep));
                }
            }
        });
    }
    Ok(())
}

async fn open_leg(state: &IrohState, ep: &str, frame: &Value) -> Result<(Connection, SendStream, RecvStream)> {
    let conn = crate::iroh_net::friend_connection(state, ep).await?;
    let (mut send, recv) = conn.open_bi().await.context("open call stream")?;
    write_frame(&mut send, frame).await?;
    Ok((conn, send, recv))
}

/// Pump one device's call stream until it ends: frames we queue go out,
/// frames that arrive become events. Owned streams (caller side).
async fn run_leg(calls: Arc<Calls>, id: String, ep: String, conn: Connection, mut send: SendStream, mut recv: RecvStream) {
    let (tx, rx) = mpsc::unbounded_channel();
    let joined = {
        let mut guard = calls.live.lock().unwrap();
        match guard.as_mut().filter(|l| l.call.id == id && !l.call.ended()) {
            Some(l) => { l.legs.insert(ep.clone(), LegIo { tx, conn: conn.clone() }); true }
            None => false,
        }
    };
    if !joined {
        // Over before this device answered the dial: tell it so.
        let _ = write_frame(&mut send, &json!({"kind": "call.cancel"})).await;
        let _ = send.finish();
        return;
    }
    pump(&calls, &id, &ep, &mut send, &mut recv, rx).await;
    calls.apply(&id, Ev::Lost(ep));
}

async fn pump(calls: &Arc<Calls>, id: &str, ep: &str, send: &mut SendStream, recv: &mut RecvStream, mut rx: mpsc::UnboundedReceiver<Option<Value>>) {
    let writer = async {
        while let Some(Some(frame)) = rx.recv().await {
            if write_frame(send, &frame).await.is_err() { break; }
        }
        let _ = send.finish();
        // Let the last frame reach the other side before the stream drops.
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    let reader = async {
        while let Ok(frame) = read_frame_cap(recv, MAX_FRAME).await {
            on_frame(calls, id, ep, frame);
        }
    };
    tokio::select! { _ = writer => {}, _ = reader => {} }
}

/// One frame from a device in the call.
fn on_frame(calls: &Arc<Calls>, id: &str, ep: &str, frame: Value) {
    let kind = frame["kind"].as_str().unwrap_or("");
    let ev = |e: fn(String) -> Ev| Some(e(ep.to_string()));
    let ev = match kind {
        "call.ring" => ev(Ev::Ringing),
        "call.busy" => ev(Ev::Busy),
        "call.decline" => ev(Ev::Declined),
        "call.missed" => ev(Ev::Missed),
        "call.unavailable" => {
            if frame["reason"] == "unsupported" {
                if let Some(l) = calls.live.lock().unwrap().as_mut().filter(|l| l.call.id == id) { l.unsupported = true; }
            }
            ev(Ev::Unavailable)
        }
        "call.glare" => {
            // They're calling us at this very moment and their call wins.
            let mut g = calls.live.lock().unwrap();
            if let Some(l) = g.as_mut().filter(|l| l.call.id == id) {
                l.silent = true;
                *calls.glare.lock().unwrap() = Some((l.call.person.clone(), Instant::now()));
            }
            drop(g);
            ev(Ev::Unavailable)
        }
        "call.hangup" => ev(Ev::RemoteHangup),
        "call.cancel" => ev(Ev::Canceled),
        "call.taken" => ev(Ev::Taken),
        "call.answer" => {
            let Some(answer) = frame["sdp"].as_str().and_then(|s| sdp::incoming(s).ok()) else { return };
            calls.apply(id, Ev::Answered(ep.to_string()));
            let won = calls.live.lock().unwrap().as_ref().is_some_and(|l| l.call.id == id && l.call.active.as_deref() == Some(ep));
            if won { calls.emit("call://signal", json!({"callId": id, "kind": "call.answer", "sdp": answer})); }
            None
        }
        k if PASS_KINDS.contains(&k) => {
            let active = calls.live.lock().unwrap().as_ref().is_some_and(|l| l.call.id == id && l.call.active.as_deref() == Some(ep));
            if active {
                if let Some(clean) = clean_pass(&frame) {
                    if k == "call.record" { note_record(calls, id, &clean, false); }
                    calls.emit("call://signal", json!({"callId": id, "kind": k, "body": clean}));
                }
            }
            None
        }
        _ => None,
    };
    if let Some(ev) = ev { calls.apply(id, ev); }
}

/// Only known fields of a mid-call frame, bounded, reach the call screen.
fn clean_pass(frame: &Value) -> Option<Value> {
    match frame["kind"].as_str()? {
        "call.media" => Some(json!({
            "muted": frame["muted"].as_bool().unwrap_or(false),
            "camera": frame["camera"].as_bool().unwrap_or(false),
        })),
        "call.record" => {
            let action = frame["action"].as_str()?;
            matches!(action, "ask" | "allow" | "deny" | "started" | "stopped" | "share").then(|| json!({ "action": action }))
        }
        "call.sdp" => {
            let t = frame["type"].as_str().filter(|t| matches!(*t, "offer" | "answer"))?;
            Some(json!({ "type": t, "sdp": sdp::incoming(frame["sdp"].as_str()?).ok()? }))
        }
        _ => None,
    }
}

fn note_record(calls: &Arc<Calls>, id: &str, body: &Value, from_me: bool) {
    let mut guard = calls.live.lock().unwrap();
    if let Some(l) = guard.as_mut().filter(|l| l.call.id == id) {
        l.rec.note(body["action"].as_str().unwrap_or(""), from_me);
        let snap = Calls::snapshot(l);
        drop(guard);
        calls.emit("call://state", snap);
    }
}

fn spawn_ticker(calls: Arc<Calls>, id: String) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let alive = calls.live.lock().unwrap().as_ref().is_some_and(|l| l.call.id == id);
            if !alive { break; }
            calls.apply(&id, Ev::Tick(now_ms()));
        }
    });
}

// ───────────────────────── callee ─────────────────────────

/// Answer a `call.offer` stream: check who it is, ring, and keep the stream
/// as the call's line until it ends.
pub(crate) async fn serve_offer(conn: &Connection, send: &mut SendStream, recv: &mut RecvStream, state: &IrohState, req: &Value) -> Result<()> {
    let calls = calls(state);
    let who = conn.remote_id().to_string();
    let refuse = |kind: &'static str| json!({ "kind": kind });
    let Some(config) = calls.config.get().cloned() else { write_frame(send, &refuse("call.unavailable")).await?; return Ok(()) };
    // Strangers, blocked people (answered exactly like strangers), a box with
    // no screen, or a device whose call screen can't do WebRTC: unavailable.
    let friend = crate::friends::chat_sender(&config, &who);
    let id = req["callId"].as_str().unwrap_or("").to_string();
    let tag = req["tag"].as_str().and_then(|t| u64::from_str_radix(t, 16).ok());
    let offer = req["sdp"].as_str().and_then(|s| sdp::incoming(s).ok());
    let (Some(friend), Some(tag), Some(offer), true) = (friend, tag, offer, valid_id(&id)) else {
        write_frame(send, &refuse("call.unavailable")).await?;
        let _ = send.finish();
        return Ok(());
    };
    // A friend may learn that this device can't take calls (no screen, or a
    // web view without WebRTC), so their screen can say so plainly.
    if crate::mailbox::is_headless() || !calls.capable() {
        write_frame(send, &json!({"kind": "call.unavailable", "reason": "unsupported"})).await?;
        let _ = send.finish();
        return Ok(());
    }
    if !calls.rate_ok(&who) {
        log::info!("call: refusing a ring — too many from this device");
        write_frame(send, &refuse("call.unavailable")).await?;
        let _ = send.finish();
        return Ok(());
    }
    let video = req["video"].as_bool().unwrap_or(false);
    // Busy, or both calling each other at once?
    enum Seen { Free, Glare(String), Lost, Busy }
    let seen = {
        let guard = calls.live.lock().unwrap();
        match guard.as_ref() {
            None => Seen::Free,
            Some(l) if l.call.dir == Dir::Out && l.call.person == friend.id && l.call.phase == Phase::Ringing =>
                if signal::glare_winner(&l.call.id, &id) == id { Seen::Glare(l.call.id.clone()) } else { Seen::Lost },
            Some(_) => Seen::Busy,
        }
    };
    let lab_answer = lab_flags().is_some_and(|l| l["autoAnswer"] == true);
    let (auto_accept, glare_cancel) = match seen {
        Seen::Free => {
            let recent = calls.glare.lock().unwrap().take();
            (lab_answer || recent.is_some_and(|(p, t)| p == friend.id && t.elapsed() < Duration::from_secs(10)), None)
        }
        Seen::Glare(mine) => (true, Some(mine)),
        Seen::Lost => {
            // Our own call to them wins; they drop theirs when ours arrives.
            write_frame(send, &refuse("call.glare")).await?;
            let _ = send.finish();
            return Ok(());
        }
        Seen::Busy => {
            write_frame(send, &refuse("call.busy")).await?;
            let _ = send.finish();
            let (mut missed, _) = Call::incoming(&id, &friend.id, video, &who, now_ms());
            missed.on(Ev::Canceled(who.clone()), now_ms());
            calls.record_history(&Live { call: missed, tag, user: String::new(), pass: String::new(), bridge: None,
                name: friend.name.clone(), offer: None, legs: HashMap::new(), auto_accept: false, silent: false, unsupported: false, rec: Default::default() });
            return Ok(());
        }
    };
    if let Some(mine) = glare_cancel {
        // Drop my own outgoing call without a history line; theirs survives.
        if let Some(l) = calls.live.lock().unwrap().as_mut().filter(|l| l.call.id == mine) { l.silent = true; }
        calls.apply(&mine, Ev::Hangup);
    }
    let (user, pass) = (hex::encode(rand::random::<[u8; 12]>()), hex::encode(rand::random::<[u8; 24]>()));
    let bridge = media::Bridge::start(&user, &pass, false, tag).await?;
    let (tx, rx) = mpsc::unbounded_channel();
    let (call, acts) = Call::incoming(&id, &friend.id, video, &who, now_ms());
    let taken = {
        let mut guard = calls.live.lock().unwrap();
        if guard.is_some() { true } else {
        let mut legs = HashMap::new();
        legs.insert(who.clone(), LegIo { tx: tx.clone(), conn: conn.clone() });
        let l = Live { call, tag, user, pass, bridge: Some(bridge), name: friend.name.clone(), offer: Some(offer), legs,
            auto_accept, silent: false, unsupported: false, rec: Default::default() };
        for a in acts { if let Act::Send(_, f) = a { let _ = tx.send(Some(f)); } }
        let snap = Calls::snapshot(&l);
        *guard = Some(l);
        drop(guard);
        calls.emit("call://incoming", snap.clone());
        calls.emit("call://state", snap);
        false
        }
    };
    if taken {
        write_frame(send, &refuse("call.busy")).await?;
        return Ok(());
    }
    ring_here(&calls, &friend.name, &friend.id, video, !auto_accept);
    spawn_ticker(calls.clone(), id.clone());
    pump(&calls, &id, &who, send, recv, rx).await;
    calls.apply(&id, Ev::Lost(who));
    Ok(())
}

/// Bring the window up and post a notification, so a call rings even when
/// DropBeam sits in the menu bar / tray.
fn ring_here(calls: &Calls, name: &str, person: &str, video: bool, notify: bool) {
    let Some(app) = calls.app.get() else { return };
    #[cfg(desktop)]
    crate::show_main_window(app);
    if !notify { return; }
    let what = if video { "Video call" } else { "Audio call" };
    #[cfg(desktop)]
    crate::chat_notify::show(app, &format!("{name} is calling"), what, person);
}

/// The person tapped Answer: the call screen gets the offer and TURN details.
pub fn accept(state: &IrohState, id: &str) -> Result<Value> {
    let calls = calls(state);
    let guard = calls.live.lock().unwrap();
    let l = guard.as_ref().filter(|l| l.call.id == id && l.call.dir == Dir::In && l.call.phase == Phase::Ringing).context("This call has ended.")?;
    Ok(json!({ "callId": id, "turn": turn_info(l), "sdp": l.offer, "video": l.call.video, "name": l.name }))
}

/// The call screen's answer is ready: send it and start carrying media.
pub fn answer(state: &IrohState, id: &str, answer: &str) -> Result<()> {
    let calls = calls(state);
    let answer = sdp::outgoing(answer)?;
    {
        let guard = calls.live.lock().unwrap();
        let l = guard.as_ref().filter(|l| l.call.id == id && l.call.dir == Dir::In && l.call.phase == Phase::Ringing).context("This call has ended.")?;
        let leg = l.legs.values().next().context("The caller hung up.")?;
        let _ = leg.tx.send(Some(json!({"kind": "call.answer", "sdp": answer})));
    }
    calls.apply(id, Ev::Accept);
    Ok(())
}

pub fn decline(state: &IrohState, id: &str) { let c = calls(state); c.apply(id, Ev::Decline); }
pub fn hangup(state: &IrohState, id: &str) { let c = calls(state); c.apply(id, Ev::Hangup); }
pub fn media_up(state: &IrohState, id: &str) { let c = calls(state); c.apply(id, Ev::MediaUp); }

/// A mid-call frame from the call screen to the other side.
pub fn signal_out(state: &IrohState, id: &str, frame: &Value) -> Result<()> {
    let calls = calls(state);
    let kind = frame["kind"].as_str().unwrap_or("");
    anyhow::ensure!(PASS_KINDS.contains(&kind), "Unknown call message.");
    let mut clean = clean_pass(frame).context("Bad call message.")?;
    if kind == "call.sdp" { clean["sdp"] = json!(sdp::outgoing(clean["sdp"].as_str().unwrap_or(""))?); }
    if kind == "call.record" {
        // "started" is sent by the engine itself when a recording opens.
        anyhow::ensure!(matches!(clean["action"].as_str(), Some("ask" | "allow" | "deny" | "share")), "Not allowed.");
    }
    {
        let guard = calls.live.lock().unwrap();
        let l = guard.as_ref().filter(|l| l.call.id == id).context("This call has ended.")?;
        let leg = l.call.active.as_ref().and_then(|a| l.legs.get(a)).context("The call isn't connected.")?;
        let mut out = clean.clone();
        out["kind"] = json!(kind);
        let _ = leg.tx.send(Some(out));
    }
    if kind == "call.record" { note_record(&calls, id, &clean, true); }
    Ok(())
}

/// Send `started`/`stopped` for a recording (engine-owned, so the other side
/// always sees the indicator).
pub(crate) fn record_notice(calls: &Arc<Calls>, id: &str, action: &str) {
    {
        let guard = calls.live.lock().unwrap();
        let Some(l) = guard.as_ref().filter(|l| l.call.id == id) else { return };
        if let Some(leg) = l.call.active.as_ref().and_then(|a| l.legs.get(a)) {
            let _ = leg.tx.send(Some(json!({"kind": "call.record", "action": action})));
        }
    }
    note_record(calls, id, &json!({"action": action}), true);
}

/// Live numbers for the quality indicator: which path iroh is on, its RTT, and
/// what the bridge carried.
pub fn stats(state: &IrohState, id: &str) -> Result<Value> {
    let calls = calls(state);
    let guard = calls.live.lock().unwrap();
    let l = guard.as_ref().filter(|l| l.call.id == id).context("This call has ended.")?;
    let conn = l.call.active.as_ref().and_then(|a| l.legs.get(a)).map(|g| g.conn.clone());
    let detail = conn.as_ref().map(|c| serde_json::to_value(crate::iroh_net::conn_detail(c)).unwrap_or(Value::Null));
    Ok(json!({ "path": detail, "bridge": l.bridge.as_ref().map(|b| b.snapshot()) }))
}

pub(crate) fn calls_of(state: &IrohState) -> Arc<Calls> { calls(state) }

// ───────────────────────── history ─────────────────────────

/// Call history lines across every thread, newest first.
pub fn history(config: &std::path::Path, friend_id: Option<&str>, limit: usize) -> Vec<Value> {
    let mut out: Vec<(u64, Value)> = Vec::new();
    let threads: Vec<String> = match friend_id {
        Some(f) => crate::friends::thread_owner(config, f).map(|o| vec![o.id]).unwrap_or_default(),
        None => crate::friends::load(config).into_iter().map(|f| f.id).collect(),
    };
    for t in threads {
        let name = crate::friends::get(config, &t).map(|f| f.name).unwrap_or_default();
        for m in crate::chat::messages(config, &t).into_iter().filter(|m| m.kind == "call") {
            out.push((m.ts, json!({ "friendId": t, "name": name, "at": m.ts, "outgoing": m.from_me, "text": m.text,
                "video": m.call.as_ref().map(|c| c.video), "outcome": m.call.as_ref().map(|c| c.outcome.clone()),
                "durationMs": m.call.as_ref().map(|c| c.duration_ms) })));
        }
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.into_iter().take(limit).map(|x| x.1).collect()
}
