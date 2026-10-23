//! Calls end to end over loopback iroh: real endpoints, the real accept loop and
//! call streams, TURN clients standing in for the WebRTC engines, media as QUIC
//! datagrams between the bridges.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::presets;
use iroh::{Endpoint, RelayMode, SecretKey};
use serde_json::{json, Value};

use super::stun::{self, attr, method, Builder, Class};
use super::turn::{self, REALM};
use crate::iroh_net::{accept_loop, IrohState, ALPN};

const SDP: &str = "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\nc=IN IP4 0.0.0.0\r\na=fingerprint:sha-256 AB:CD\r\na=rtpmap:111 opus/48000/2\r\n";

struct Node { state: Arc<IrohState>, config: std::path::PathBuf, ep: Endpoint, _dir: tempdir::Dir }

mod tempdir {
    pub struct Dir(pub std::path::PathBuf);
    impl Drop for Dir { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
}

impl Node {
    async fn new() -> Node {
        let dir = std::env::temp_dir().join(format!("dropbeam-calls-{}", rand::random::<u64>()));
        let config = dir.join("config");
        std::fs::create_dir_all(&config).unwrap();
        let ep = Endpoint::builder(presets::Minimal).secret_key(SecretKey::generate()).alpns(vec![ALPN.to_vec()])
            .relay_mode(RelayMode::Disabled).bind_addr("127.0.0.1:0").unwrap().bind().await.unwrap();
        let state = Arc::new(IrohState::default());
        state.location_config.set(config.clone()).unwrap();
        state.endpoint.set(ep.clone()).ok().unwrap();
        state.calls.set_capable(true);
        tokio::spawn(accept_loop(ep.clone(), state.clone()));
        Node { state, config, ep, _dir: tempdir::Dir(dir) }
    }
    fn id(&self) -> String { self.ep.id().to_string() }
    /// Make `other` a friend of this node; returns the friend id.
    fn befriend(&self, other: &Node, name: &str) -> String {
        crate::iroh_net::remember_addrs_for_tests(&other.ep.addr());
        crate::friends::upsert_by_endpoint(&self.config, &other.id(), name).id
    }
    fn events(&self, name: &str) -> Vec<Value> {
        self.state.calls.events.lock().unwrap().iter().filter(|(n, _)| n == name).map(|(_, v)| v.clone()).collect()
    }
    async fn wait_event(&self, name: &str, pred: impl Fn(&Value) -> bool) -> Value {
        for _ in 0..200 {
            if let Some(v) = self.events(name).into_iter().find(|v| pred(v)) { return v; }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("no {name} event; got {:?}", self.state.calls.events.lock().unwrap());
    }
    async fn wait_end(&self) -> Value {
        self.wait_event("call://state", |v| v["phase"] == "ended").await
    }
    fn history(&self) -> Vec<Value> { super::history(&self.config, None, 50) }
}

/// A stand-in for the WebRTC engine: allocate on the bridge, permit the peer,
/// send and receive through it.
struct Engine { sock: tokio::net::UdpSocket, server: SocketAddr, key: [u8; 16], user: String, nonce: Vec<u8>, relayed: Option<SocketAddr> }

impl Engine {
    async fn new(turn: &Value) -> Engine {
        let url = turn["urls"][0].as_str().unwrap();
        let port: u16 = url.rsplit(':').next().unwrap().split('?').next().unwrap().parse().unwrap();
        let user = turn["username"].as_str().unwrap().to_string();
        let key = stun::long_term_key(&user, REALM, turn["credential"].as_str().unwrap());
        let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        Engine { sock, server: SocketAddr::from(([127, 0, 0, 1], port)), key, user, nonce: vec![], relayed: None }
    }
    async fn rt(&self, pkt: &[u8]) -> Vec<u8> {
        self.sock.send_to(pkt, self.server).await.unwrap();
        let mut b = vec![0u8; 2048];
        let (n, _) = tokio::time::timeout(Duration::from_secs(2), self.sock.recv_from(&mut b)).await.unwrap().unwrap();
        b.truncate(n);
        b
    }
    fn req(&self, m: u16, f: impl FnOnce(Builder) -> Builder) -> Vec<u8> {
        f(Builder::new(m, Class::Request, rand::random())).add(attr::USERNAME, self.user.as_bytes())
            .add(attr::REALM, REALM.as_bytes()).add(attr::NONCE, &self.nonce).integrity(&self.key).build()
    }
    async fn allocate(&mut self) -> SocketAddr {
        let r = self.rt(&Builder::new(method::ALLOCATE, Class::Request, rand::random()).add(attr::REQUESTED_TRANSPORT, &[17, 0, 0, 0]).build()).await;
        self.nonce = stun::parse(&r).unwrap().get(attr::NONCE).unwrap().to_vec();
        let r = self.rt(&self.req(method::ALLOCATE, |b| b.add(attr::REQUESTED_TRANSPORT, &[17, 0, 0, 0]))).await;
        let a = stun::parse(&r).unwrap().xor_addr(attr::XOR_RELAYED_ADDRESS).unwrap();
        self.relayed = Some(a);
        a
    }
    async fn permit(&self, peer: SocketAddr) {
        let r = self.rt(&self.req(method::CREATE_PERMISSION, |b| b.xor_addr(attr::XOR_PEER_ADDRESS, peer))).await;
        assert_eq!(stun::parse(&r).unwrap().class, Class::Success);
    }
    async fn send(&self, peer: SocketAddr, data: &[u8]) {
        let p = Builder::new(method::SEND, Class::Indication, rand::random()).xor_addr(attr::XOR_PEER_ADDRESS, peer).add(attr::DATA, data).build();
        self.sock.send_to(&p, self.server).await.unwrap();
    }
    async fn recv(&self) -> (SocketAddr, Vec<u8>) {
        let mut b = vec![0u8; 4096];
        let (n, _) = tokio::time::timeout(Duration::from_secs(5), self.sock.recv_from(&mut b)).await.expect("media arrives").unwrap();
        let m = stun::parse(&b[..n]).unwrap();
        (m.xor_addr(attr::XOR_PEER_ADDRESS).unwrap(), m.get(attr::DATA).unwrap().to_vec())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_friend_call_rings_connects_carries_media_and_is_logged() {
    let (a, b) = (Node::new().await, Node::new().await);
    let b_in_a = a.befriend(&b, "Bea");
    b.befriend(&a, "Al");
    let started = super::start(&a.state, &b_in_a, true).await.unwrap();
    let id = started["callId"].as_str().unwrap().to_string();
    super::send_offer(&a.state, &id, SDP).await.unwrap();

    let incoming = b.wait_event("call://incoming", |_| true).await;
    assert_eq!(incoming["name"], "Al");
    assert_eq!(incoming["video"], true);
    a.wait_event("call://state", |v| v["legs"][0]["state"] == "ringing").await;

    let acc = super::accept(&b.state, &id).unwrap();
    assert!(acc["sdp"].as_str().unwrap().contains("useinbandfec=1;usedtx=0"), "the caller's Opus line was tuned");
    super::answer(&b.state, &id, SDP).unwrap();
    let ans = a.wait_event("call://signal", |v| v["kind"] == "call.answer").await;
    assert!(ans["sdp"].as_str().unwrap().starts_with("v=0"));

    // Media: each "engine" allocates on its own bridge; packets cross as datagrams.
    let mut ea = Engine::new(&started["turn"]).await;
    let mut eb = Engine::new(&acc["turn"]).await;
    let ra = ea.allocate().await;
    let rb = eb.allocate().await;
    ea.permit(rb).await;
    eb.permit(ra).await;
    let big: Vec<u8> = (0..3000u32).map(|i| (i * 7) as u8).collect();
    ea.send(rb, b"rtp from a").await;
    assert_eq!(eb.recv().await, (ra, b"rtp from a".to_vec()));
    eb.send(ra, &big).await; // larger than one datagram: fragmented and rejoined
    assert_eq!(ea.recv().await, (rb, big));

    super::media_up(&a.state, &id);
    super::media_up(&b.state, &id);
    let stats = super::stats(&a.state, &id).unwrap();
    assert!(stats["bridge"]["packetsOut"].as_u64().unwrap() >= 1);
    assert!(stats["path"].is_object());

    // Mid-call frames pass through, bounded to known fields.
    super::signal_out(&a.state, &id, &json!({"kind": "call.media", "muted": true, "camera": false, "evil": "x"})).unwrap();
    let m = b.wait_event("call://signal", |v| v["kind"] == "call.media").await;
    assert_eq!(m["body"], json!({"muted": true, "camera": false}));
    assert!(super::signal_out(&a.state, &id, &json!({"kind": "call.offer"})).is_err());

    super::hangup(&b.state, &id);
    let end_a = a.wait_end().await;
    assert_eq!(end_a["end"], "remote_hangup");
    let end_b = b.wait_end().await;
    assert_eq!(end_b["end"], "hangup");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let ha = a.history();
    assert_eq!(ha.len(), 1);
    assert!(ha[0]["text"].as_str().unwrap().starts_with("Video call · 0:0"), "{ha:?}");
    assert_eq!(ha[0]["outgoing"], true);
    assert_eq!(b.history()[0]["outgoing"], false);
    // The connection is free for housekeeping again.
    assert!(a.state.calls.live.lock().unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn strangers_and_blocked_people_cannot_ring() {
    let (a, b) = (Node::new().await, Node::new().await);
    // B doesn't know A: unavailable, nothing rings.
    let b_in_a = a.befriend(&b, "Bea");
    let id = super::start(&a.state, &b_in_a, false).await.unwrap()["callId"].as_str().unwrap().to_string();
    super::send_offer(&a.state, &id, SDP).await.unwrap();
    assert_eq!(a.wait_end().await["end"], "unavailable");
    assert!(b.events("call://incoming").is_empty());

    // B knows A but blocked them: answered exactly like a stranger.
    let a_in_b = b.befriend(&a, "Al");
    crate::block::block_friend(&b.config, &a_in_b).unwrap();
    a.state.calls.events.lock().unwrap().clear();
    let id = super::start(&a.state, &b_in_a, false).await.unwrap()["callId"].as_str().unwrap().to_string();
    super::send_offer(&a.state, &id, SDP).await.unwrap();
    assert_eq!(a.wait_end().await["end"], "unavailable");
    assert!(b.events("call://incoming").is_empty());
    assert!(b.history().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_that_cannot_do_calls_says_unavailable() {
    let (a, b) = (Node::new().await, Node::new().await);
    let b_in_a = a.befriend(&b, "Bea");
    b.befriend(&a, "Al");
    b.state.calls.set_capable(false);
    let id = super::start(&a.state, &b_in_a, false).await.unwrap()["callId"].as_str().unwrap().to_string();
    super::send_offer(&a.state, &id, SDP).await.unwrap();
    let end = a.wait_end().await;
    assert_eq!(end["end"], "unavailable");
    assert_eq!(end["unsupported"], true);
    assert_eq!(a.history()[0]["text"], "Audio call · Couldn’t connect");
}

#[tokio::test(flavor = "multi_thread")]
async fn busy_decline_and_missed() {
    let (a, b, c) = (Node::new().await, Node::new().await, Node::new().await);
    let b_in_a = a.befriend(&b, "Bea");
    b.befriend(&a, "Al");
    let b_in_c = c.befriend(&b, "Bea");
    b.befriend(&c, "Cy");
    // A calls B; B declines.
    let id = super::start(&a.state, &b_in_a, false).await.unwrap()["callId"].as_str().unwrap().to_string();
    super::send_offer(&a.state, &id, SDP).await.unwrap();
    b.wait_event("call://incoming", |_| true).await;
    // Meanwhile C calls B: busy, and B gets a missed-call line for Cy.
    let idc = super::start(&c.state, &b_in_c, false).await.unwrap()["callId"].as_str().unwrap().to_string();
    super::send_offer(&c.state, &idc, SDP).await.unwrap();
    assert_eq!(c.wait_end().await["end"], "busy");
    super::decline(&b.state, &id);
    assert_eq!(a.wait_end().await["end"], "declined");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let texts: Vec<String> = b.history().iter().map(|h| h["text"].as_str().unwrap().to_string()).collect();
    assert!(texts.contains(&"Declined audio call".to_string()), "{texts:?}");
    assert!(texts.contains(&"Missed audio call".to_string()), "{texts:?}");
    assert_eq!(a.history()[0]["text"], "Audio call · Declined");

    // A calls again and hangs up while it rings: B logs a missed call.
    a.state.calls.events.lock().unwrap().clear();
    b.state.calls.events.lock().unwrap().clear();
    let id = super::start(&a.state, &b_in_a, false).await.unwrap()["callId"].as_str().unwrap().to_string();
    super::send_offer(&a.state, &id, SDP).await.unwrap();
    b.wait_event("call://incoming", |_| true).await;
    super::hangup(&a.state, &id);
    assert_eq!(b.wait_end().await["end"], "missed");
}

#[tokio::test(flavor = "multi_thread")]
async fn ringing_is_rate_limited() {
    let (a, b) = (Node::new().await, Node::new().await);
    let b_in_a = a.befriend(&b, "Bea");
    b.befriend(&a, "Al");
    let mut rang = 0;
    for _ in 0..(super::RING_PER_MINUTE + 2) {
        a.state.calls.events.lock().unwrap().clear();
        b.state.calls.events.lock().unwrap().clear();
        let id = super::start(&a.state, &b_in_a, false).await.unwrap()["callId"].as_str().unwrap().to_string();
        super::send_offer(&a.state, &id, SDP).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        if !b.events("call://incoming").is_empty() { rang += 1; }
        super::hangup(&a.state, &id);
        a.wait_end().await;
    }
    assert_eq!(rang, super::RING_PER_MINUTE);
}

#[tokio::test(flavor = "multi_thread")]
async fn several_devices_ring_and_the_first_to_answer_wins() {
    let (a, b1, b2) = (Node::new().await, Node::new().await, Node::new().await);
    // B has two devices with one account; A knows both.
    let b1_in_a = a.befriend(&b1, "Bea");
    a.befriend(&b2, "Bea's iPhone");
    crate::friends::set_device_info(&a.config, &b1.id(), None, Some("acct-bea"));
    crate::friends::set_device_info(&a.config, &b2.id(), None, Some("acct-bea"));
    b1.befriend(&a, "Al");
    b2.befriend(&a, "Al");
    let id = super::start(&a.state, &b1_in_a, true).await.unwrap()["callId"].as_str().unwrap().to_string();
    super::send_offer(&a.state, &id, SDP).await.unwrap();
    b1.wait_event("call://incoming", |_| true).await;
    b2.wait_event("call://incoming", |_| true).await;
    super::accept(&b2.state, &id).unwrap();
    super::answer(&b2.state, &id, SDP).unwrap();
    a.wait_event("call://signal", |v| v["kind"] == "call.answer").await;
    // b1 stops ringing: answered elsewhere, and it logs nothing.
    assert_eq!(b1.wait_end().await["end"], "answered_elsewhere");
    // b1 answering late is refused politely.
    assert!(super::accept(&b1.state, &id).is_err());
    let snap = a.wait_event("call://state", |v| v["phase"] == "connecting").await;
    assert_eq!(snap["active"], b2.id());
    super::hangup(&a.state, &id);
    assert_eq!(b2.wait_end().await["end"], "remote_hangup");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(b1.history().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn calling_each_other_at_once_connects_one_call() {
    let (a, b) = (Node::new().await, Node::new().await);
    let b_in_a = a.befriend(&b, "Bea");
    let a_in_b = b.befriend(&a, "Al");
    let ida = super::start(&a.state, &b_in_a, false).await.unwrap()["callId"].as_str().unwrap().to_string();
    let idb = super::start(&b.state, &a_in_b, false).await.unwrap()["callId"].as_str().unwrap().to_string();
    let (r1, r2) = tokio::join!(super::send_offer(&a.state, &ida, SDP), super::send_offer(&b.state, &idb, SDP));
    r1.unwrap(); r2.unwrap();
    let winner = std::cmp::min(ida.clone(), idb.clone());
    // The side whose call lost answers the winner automatically.
    let callee = if winner == ida { &b } else { &a };
    let inc = callee.wait_event("call://incoming", |v| v["id"] == winner.as_str()).await;
    assert_eq!(inc["autoAccept"], true);
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Only the winning call is live on both sides.
    for n in [&a, &b] {
        let live = n.state.calls.live.lock().unwrap();
        assert_eq!(live.as_ref().map(|l| l.call.id.clone()), Some(winner.clone()));
    }
    let _ = turn::virtual_ip(true);
}
