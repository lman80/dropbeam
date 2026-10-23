//! "Find my safe speed" and "Protect my router" (#20).
//!
//! Some home routers fall over when one device sends as fast as it can: Wi-Fi
//! drops for everyone until the router reboots. Nobody knows their router's
//! limit, so DropBeam finds it: it sends test data (thrown away on the other
//! end) to one of your devices or a friend, a little faster every few seconds,
//! while it keeps asking the router "are you there?" (the default gateway, by
//! ICMP where the system allows it unprivileged, else a TCP knock on a port the
//! router answers). The moment the router answers slowly or stops answering,
//! the test stops and recommends ~80% of the last speed the router handled.
//!
//! "Protect my router" keeps watching the router while files are being sent and
//! slows sending down automatically when it struggles, recovering slowly.

use std::collections::VecDeque;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The local network's router (default gateway).
pub fn gateway() -> Option<Ipv4Addr> {
    netdev::get_default_gateway().ok()?.ipv4.into_iter().find(|ip| !ip.is_unspecified() && !ip.is_loopback())
}

// ── asking the router "are you there?" ──────────────────────────────────────

/// How we time the router's answer.
enum How {
    /// ICMP echo over an unprivileged datagram socket (Linux with ping sockets
    /// allowed, macOS). `seq` numbers each echo.
    #[cfg(unix)]
    Icmp(std::net::UdpSocket, u16),
    /// A TCP connect to a port the router answers (accepted OR refused: both
    /// are the router's own TCP stack replying).
    Tcp(u16),
}

pub struct Prober {
    gw: Ipv4Addr,
    how: How,
}

impl Prober {
    /// A way to time the router at `gw`, or None when nothing gets an answer.
    pub fn open(gw: Ipv4Addr) -> Option<Prober> {
        #[cfg(unix)]
        {
            if let Some(sock) = icmp_socket() {
                let mut p = Prober { gw, how: How::Icmp(sock, 0) };
                if (0..3).any(|_| p.rtt(Duration::from_millis(800)).is_some()) {
                    return Some(p);
                }
            }
        }
        for port in [80u16, 443, 53, 8080, 22] {
            if tcp_rtt(gw, port, Duration::from_millis(800)).is_some() {
                return Some(Prober { gw, how: How::Tcp(port) });
            }
        }
        None
    }

    /// What the probe uses, for the log ("icmp" / "tcp:80").
    pub fn method(&self) -> String {
        match &self.how {
            #[cfg(unix)]
            How::Icmp(..) => "icmp".into(),
            How::Tcp(port) => format!("tcp:{port}"),
        }
    }

    /// One round trip to the router, or None if it didn't answer in `timeout`.
    pub fn rtt(&mut self, timeout: Duration) -> Option<Duration> {
        match &mut self.how {
            #[cfg(unix)]
            How::Icmp(sock, seq) => {
                *seq = seq.wrapping_add(1);
                icmp_rtt(sock, self.gw, *seq, timeout)
            }
            How::Tcp(port) => tcp_rtt(self.gw, *port, timeout),
        }
    }
}

fn tcp_rtt(gw: Ipv4Addr, port: u16, timeout: Duration) -> Option<Duration> {
    let t = Instant::now();
    match TcpStream::connect_timeout(&SocketAddr::from((gw, port)), timeout) {
        Ok(_) => Some(t.elapsed()),
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => Some(t.elapsed()),
        Err(_) => None,
    }
}

#[cfg(unix)]
fn icmp_socket() -> Option<std::net::UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::ICMPV4)).ok()?;
    Some(s.into())
}

/// An ICMP echo request (`id` is replaced by the kernel on Linux ping sockets).
#[cfg_attr(not(unix), allow(dead_code))]
fn echo_request(seq: u16) -> [u8; 16] {
    let mut p = [0u8; 16];
    p[0] = 8; // echo request
    p[4..6].copy_from_slice(&0x4442u16.to_be_bytes()); // "DB"
    p[6..8].copy_from_slice(&seq.to_be_bytes());
    p[8..16].copy_from_slice(b"dropbeam");
    let sum = checksum(&p);
    p[2..4].copy_from_slice(&sum.to_be_bytes());
    p
}

#[cfg_attr(not(unix), allow(dead_code))]
fn checksum(b: &[u8]) -> u16 {
    let mut sum: u32 = b.chunks(2).map(|c| u32::from(u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]))).sum();
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// The sequence number of an echo REPLY in `b` (macOS hands us the IP header too).
#[cfg_attr(not(unix), allow(dead_code))]
fn echo_reply_seq(b: &[u8]) -> Option<u16> {
    let icmp = if b.first().is_some_and(|v| v >> 4 == 4) { b.get(usize::from(b[0] & 0x0f) * 4..)? } else { b };
    (icmp.len() >= 8 && icmp[0] == 0).then(|| u16::from_be_bytes([icmp[6], icmp[7]]))
}

#[cfg(unix)]
fn icmp_rtt(sock: &std::net::UdpSocket, gw: Ipv4Addr, seq: u16, timeout: Duration) -> Option<Duration> {
    let t = Instant::now();
    sock.send_to(&echo_request(seq), SocketAddr::from((gw, 0))).ok()?;
    let mut buf = [0u8; 256];
    loop {
        let left = timeout.checked_sub(t.elapsed()).filter(|d| !d.is_zero())?;
        sock.set_read_timeout(Some(left)).ok()?;
        let n = sock.recv(&mut buf).ok()?;
        if echo_reply_seq(&buf[..n]) == Some(seq) {
            return Some(t.elapsed());
        }
    }
}

// ── is the router struggling? ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Fine,
    /// Slower answers or one missed — don't push harder yet.
    Strained,
    /// Answers far slower than normal, or several missed: back off now.
    Trouble,
}

/// Judges the router's answers against how it answered while idle. Wi-Fi
/// jitters, so one slow answer never counts; trouble is a pattern.
#[derive(Default)]
pub struct Judge {
    idle: Vec<Duration>,
    recent: VecDeque<Option<Duration>>,
}

const WINDOW: usize = 5;

impl Judge {
    /// An answer (or a miss) while nothing is being sent.
    pub fn idle(&mut self, rtt: Option<Duration>) {
        if let Some(r) = rtt {
            self.idle.push(r);
        }
    }
    /// The router's normal answer time (median while idle).
    pub fn normal(&self) -> Option<Duration> {
        let mut v = self.idle.clone();
        v.sort();
        v.get(v.len() / 2).copied()
    }
    /// Answers slower than this count as slow: 6× normal and at least 80 ms
    /// more (Wi-Fi's ordinary jitter stays under it).
    pub fn slow_after(&self) -> Duration {
        let n = self.normal().unwrap_or(Duration::from_millis(5));
        (n * 6).max(n + Duration::from_millis(80))
    }
    /// Forget recent answers (a new speed step starts fresh).
    pub fn reset_recent(&mut self) {
        self.recent.clear();
    }
    /// An answer (or a miss) under load → how the router is doing.
    pub fn push(&mut self, rtt: Option<Duration>) -> Health {
        self.recent.push_back(rtt);
        while self.recent.len() > WINDOW {
            self.recent.pop_front();
        }
        let slow = self.slow_after();
        let missed = self.recent.iter().filter(|r| r.is_none()).count();
        let slowed = self.recent.iter().filter(|r| r.is_some_and(|r| r > slow)).count();
        if missed >= 2 || slowed >= 3 || missed + slowed >= 4 {
            Health::Trouble
        } else if missed + slowed >= 1 {
            Health::Strained
        } else {
            Health::Fine
        }
    }
}

// ── the speed ramp ──────────────────────────────────────────────────────────

/// Test speeds in Mbps, slowest first.
pub const STEPS: [u32; 13] = [25, 50, 75, 100, 150, 200, 300, 400, 600, 800, 1000, 1500, 2500];

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Outcome {
    /// The router struggled at `troubleMbps`; `safeMbps` is the recommendation.
    #[serde(rename_all = "camelCase")]
    Limit { safe_mbps: u32, trouble_mbps: u32 },
    /// Full speed (`topMbps`, what the connection really carried) never bothered
    /// the router: no limit needed.
    #[serde(rename_all = "camelCase")]
    NoLimit { top_mbps: u32 },
}

/// Walks up `STEPS`; told after each step what was really sent and how the
/// router did, it says whether to go on.
#[derive(Default)]
pub struct Ramp {
    step: usize,
    healthy_mbps: Option<f64>,
    best_mbps: f64,
    flat: u8,
}

impl Ramp {
    pub fn rate_mbps(&self) -> u32 {
        STEPS[self.step.min(STEPS.len() - 1)]
    }
    /// The step finished having carried `achieved` Mbps with the router in
    /// `worst` shape. None = test the next speed; Some = done.
    pub fn finish_step(&mut self, achieved: f64, worst: Health) -> Option<Outcome> {
        if worst == Health::Trouble {
            // 80% of the last speed the router handled (never below 5 Mbps). If
            // even the first step was too much, 80% of what that step carried.
            let base = self.healthy_mbps.unwrap_or(achieved.min(f64::from(self.rate_mbps())));
            return Some(Outcome::Limit { safe_mbps: ((base * 0.8).floor() as u32).max(5), trouble_mbps: achieved.round().max(1.0) as u32 });
        }
        if worst == Health::Fine {
            self.healthy_mbps = Some(self.healthy_mbps.unwrap_or(0.0).max(achieved));
        }
        // The connection itself is the ceiling: it stopped going faster.
        if achieved < self.best_mbps * 1.1 && achieved < f64::from(self.rate_mbps()) * 0.85 {
            self.flat += 1;
        } else {
            self.flat = 0;
        }
        self.best_mbps = self.best_mbps.max(achieved);
        self.step += 1;
        if self.flat >= 2 || self.step >= STEPS.len() {
            return Some(Outcome::NoLimit { top_mbps: self.best_mbps.round() as u32 });
        }
        None
    }
}

// ── "Protect my router" while real files are sent ───────────────────────────

/// Bytes handed to the network by every send (counted by the pacer).
pub static SENT: AtomicU64 = AtomicU64::new(0);
/// The cap "Protect my router" is holding sending to, bytes/s (0 = none).
pub static GUARD_BPS: AtomicU64 = AtomicU64::new(0);
static PROTECT: AtomicBool = AtomicBool::new(false);
static WATCHING: AtomicBool = AtomicBool::new(false);
/// Set while "Find my safe speed" runs (only one at a time; it can be canceled).
pub static TESTING: AtomicBool = AtomicBool::new(false);
pub static CANCEL: AtomicBool = AtomicBool::new(false);

pub fn set_protect(on: bool) {
    PROTECT.store(on, Ordering::Relaxed);
    if !on {
        GUARD_BPS.store(0, Ordering::Relaxed);
    }
}
pub fn protecting() -> bool {
    PROTECT.load(Ordering::Relaxed)
}

/// The guard's next cap from the router's health and what was being sent:
/// trouble → 70% of the current rate (at least 2 Mbps); a calm router lets the
/// cap rise 10% at a time, and once it is well above what's sent, it lifts.
pub fn next_guard(cap_bps: u64, sending_bps: u64, health: Health, calm_for: Duration) -> u64 {
    const FLOOR: u64 = 2_000_000 / 8;
    match health {
        Health::Trouble => {
            let from = if cap_bps == 0 { sending_bps } else { cap_bps.min(sending_bps.max(FLOOR)) };
            (from * 7 / 10).max(FLOOR)
        }
        Health::Strained => cap_bps,
        Health::Fine if cap_bps == 0 => 0,
        Health::Fine if calm_for >= Duration::from_secs(10) => {
            let up = cap_bps + cap_bps / 10;
            if sending_bps > 0 && up > sending_bps * 2 { 0 } else { up }
        }
        Health::Fine => cap_bps,
    }
}

/// Start the guard's watcher (once). It sleeps while nothing is being sent.
#[cfg_attr(not(desktop), allow(dead_code))]
pub fn start_guard(app: tauri::AppHandle) {
    if WATCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new().name("router-guard".into()).spawn(move || {
        use tauri::Emitter;
        let mut prober: Option<(Ipv4Addr, Prober)> = None;
        let mut judge = Judge::default();
        let (mut last_sent, mut last_at) = (SENT.load(Ordering::Relaxed), Instant::now());
        let mut calm_since = Instant::now();
        let mut idle_ticks = 0u32;
        loop {
            std::thread::sleep(Duration::from_millis(500));
            if !protecting() || TESTING.load(Ordering::Relaxed) {
                continue;
            }
            let sent = SENT.load(Ordering::Relaxed);
            let rate = (sent.saturating_sub(last_sent) as f64 / last_at.elapsed().as_secs_f64().max(0.1)) as u64;
            (last_sent, last_at) = (sent, Instant::now());
            let busy = rate > 1_000_000 / 8;
            // Learn (or relearn) the router while idle: that's its "normal".
            if !busy {
                idle_ticks += 1;
                // Look for the router now and then (it changes when the Wi-Fi does).
                if idle_ticks % 20 == 1 && prober.as_ref().is_none_or(|(gw, _)| Some(*gw) != gateway()) {
                    prober = gateway().and_then(|gw| Prober::open(gw).map(|p| (gw, p)));
                    judge = Judge::default();
                }
                if let Some((_, p)) = prober.as_mut() {
                    if judge.idle.len() < 9 {
                        judge.idle(p.rtt(Duration::from_millis(800)));
                    }
                }
                if GUARD_BPS.load(Ordering::Relaxed) != 0 && idle_ticks > 60 {
                    GUARD_BPS.store(0, Ordering::Relaxed); // a quiet minute: start fresh
                }
                continue;
            }
            idle_ticks = 0;
            let Some((_, p)) = prober.as_mut() else { continue };
            if judge.normal().is_none() {
                continue;
            }
            let health = judge.push(p.rtt(Duration::from_millis(1000)));
            if health != Health::Fine {
                calm_since = Instant::now();
            }
            let cap = GUARD_BPS.load(Ordering::Relaxed);
            let next = next_guard(cap, rate, health, calm_since.elapsed());
            if next != cap {
                GUARD_BPS.store(next, Ordering::Relaxed);
                if next != 0 && health == Health::Trouble {
                    calm_since = Instant::now();
                    log::info!("router guard: router struggling ({}), sending capped at {} Mbps", p.method(), next * 8 / 1_000_000);
                    let _ = app.emit("safe-speed://guard", serde_json::json!({ "capMbps": next * 8 / 1_000_000 }));
                } else if next == 0 {
                    log::info!("router guard: router calm again, cap lifted");
                    let _ = app.emit("safe-speed://guard", serde_json::json!({ "capMbps": 0 }));
                }
            }
        }
    }).ok();
}

// ── the test itself ─────────────────────────────────────────────────────────

/// How long each speed is held (the router needs a moment to show strain).
const STEP_TIME: Duration = Duration::from_secs(3);
/// What a test may send at most (a receiver refuses more).
const TEST_MAX_BYTES: u64 = 16 << 30;
const TEST_MAX_TIME: Duration = Duration::from_secs(120);

/// The result the Settings screen shows.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    #[serde(flatten)]
    pub outcome: Outcome,
    /// Who the test data went to.
    pub target: String,
    /// Over the local network (else the internet).
    pub local: bool,
    /// The router's normal answer time, ms.
    pub router_ms: f64,
}

pub(crate) fn locality_is_local(conn: &iroh::endpoint::Connection) -> bool {
    matches!(crate::iroh_net::locality_of(conn), crate::models::Locality::Local)
}

static TESTS_RUNNING: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::LazyLock::new(Default::default);
/// A peer's one running speed test (freed on drop).
struct TestSlot(String);
impl TestSlot {
    fn take(who: &str) -> Option<TestSlot> {
        TESTS_RUNNING.lock().unwrap_or_else(|p| p.into_inner()).insert(who.to_owned()).then(|| TestSlot(who.to_owned()))
    }
}
impl Drop for TestSlot {
    fn drop(&mut self) { TESTS_RUNNING.lock().unwrap_or_else(|p| p.into_inner()).remove(&self.0); }
}

/// Serve a friend's (or own device's) speed test: read and throw away what
/// they send, then say how much arrived. Strangers are refused.
pub(crate) async fn serve_test(config: &std::path::Path, who: &str, send: &mut iroh::endpoint::SendStream, recv: &mut iroh::endpoint::RecvStream) -> anyhow::Result<()> {
    use crate::iroh_net::write_frame;
    if crate::friends::chat_sender(config, who).is_none() {
        write_frame(send, &serde_json::json!({ "kind": "speed-test", "ok": false, "error": "Not a friend" })).await?;
        let _ = send.finish();
        return Ok(());
    }
    // One test at a time per peer: each may take a minute and up to TEST_MAX_BYTES.
    let Some(_running) = TestSlot::take(who) else {
        write_frame(send, &serde_json::json!({ "kind": "speed-test", "ok": false, "error": "A speed test from this device is already running" })).await?;
        let _ = send.finish();
        return Ok(());
    };
    write_frame(send, &serde_json::json!({ "kind": "speed-test", "ok": true })).await?;
    let start = Instant::now();
    let mut got = 0u64;
    let mut buf = vec![0u8; 256 * 1024];
    while start.elapsed() < TEST_MAX_TIME && got < TEST_MAX_BYTES {
        match tokio::time::timeout(Duration::from_secs(10), recv.read(&mut buf)).await {
            Ok(Ok(Some(n))) => got += n as u64,
            _ => break,
        }
    }
    let _ = recv.stop(0u32.into());
    write_frame(send, &serde_json::json!({ "kind": "speed-test", "ok": true, "bytes": got })).await?;
    let _ = send.finish();
    Ok(())
}

/// Progress for the Settings screen.
fn progress(app: &tauri::AppHandle, v: serde_json::Value) {
    use tauri::Emitter;
    let _ = app.emit("safe-speed://progress", v);
}

/// Run "Find my safe speed" against the first device in `targets` (endpoint
/// id, name) that takes a test, preferring one on the local network.
pub(crate) async fn run(app: tauri::AppHandle, state: std::sync::Arc<crate::iroh_net::IrohState>, targets: Vec<(String, String)>) -> Result<Report, String> {
    if TESTING.swap(true, Ordering::SeqCst) {
        return Err("A speed test is already running.".into());
    }
    CANCEL.store(false, Ordering::SeqCst);
    let out = run_inner(&app, &state, targets).await;
    TESTING.store(false, Ordering::SeqCst);
    out
}

async fn run_inner(app: &tauri::AppHandle, state: &crate::iroh_net::IrohState, targets: Vec<(String, String)>) -> Result<Report, String> {
    use crate::iroh_net::{read_frame_cap, write_frame};
    progress(app, serde_json::json!({ "phase": "router" }));
    let gw = gateway().ok_or("DropBeam couldn’t find your router. Are you connected to Wi‑Fi or a network cable?")?;
    let (prober, mut judge) = tokio::task::spawn_blocking(move || {
        let mut p = Prober::open(gw)?;
        let mut j = Judge::default();
        for _ in 0..7 {
            j.idle(p.rtt(Duration::from_millis(800)));
            std::thread::sleep(Duration::from_millis(150));
        }
        Some((p, j))
    }).await.ok().flatten().ok_or("Your router didn’t answer DropBeam’s check, so the test can’t watch it. You can still pick a limit yourself.")?;
    let normal = judge.normal().unwrap_or_default();
    log::info!("safe speed: router {gw} answers in {normal:?} ({})", prober.method());
    // Find someone to send test data to: local-network devices first.
    progress(app, serde_json::json!({ "phase": "device" }));
    let mut picked = None;
    let mut needs_update = false;
    let mut conns = Vec::new();
    for (eid, name) in targets {
        if CANCEL.load(Ordering::Relaxed) { return Err("Canceled.".into()); }
        if let Ok(Ok(conn)) = tokio::time::timeout(Duration::from_secs(6), crate::iroh_net::friend_connection(state, &eid)).await {
            conns.push((locality_is_local(&conn), conn, name));
        }
    }
    conns.sort_by_key(|(local, ..)| !*local);
    for (local, conn, name) in conns {
        let Ok((mut send, mut recv)) = conn.open_bi().await else { continue };
        if write_frame(&mut send, &serde_json::json!({ "kind": "speed-test", "v": 1 })).await.is_err() { continue; }
        match tokio::time::timeout(Duration::from_secs(8), read_frame_cap(&mut recv, 4096)).await {
            Ok(Ok(r)) if r["kind"] == "speed-test" && r["ok"] == true => { picked = Some((local, send, recv, name)); break; }
            Ok(Ok(r)) if r["kind"] != "speed-test" => needs_update = true,
            _ => {}
        }
    }
    let Some((local, mut send, mut recv, target)) = picked else {
        return Err(if needs_update { "Your devices need a DropBeam update to run the test. Update DropBeam on another device, then try again." }
            else { "The test needs another device or a friend online with DropBeam open. Open DropBeam on one, then try again." }.into());
    };
    // Ramp: each step sends at its speed while the router is asked every ~250 ms.
    let prober = std::sync::Arc::new(std::sync::Mutex::new(prober));
    let mut ramp = Ramp::default();
    let chunk = vec![0u8; 64 * 1024];
    let started = Instant::now();
    let outcome = loop {
        if CANCEL.load(Ordering::Relaxed) {
            let _ = send.reset(0u32.into());
            return Err("Canceled.".into());
        }
        let rate = ramp.rate_mbps();
        let bps = f64::from(rate) * 1_000_000.0 / 8.0;
        judge.reset_recent();
        let watching = std::sync::Arc::new(AtomicBool::new(true));
        let (w, p) = (watching.clone(), prober.clone());
        let watcher = tokio::task::spawn_blocking(move || {
            let mut answers = Vec::new();
            while w.load(Ordering::Relaxed) {
                let t = Instant::now();
                answers.push(p.lock().unwrap_or_else(|e| e.into_inner()).rtt(Duration::from_millis(1000)));
                std::thread::sleep(Duration::from_millis(250).saturating_sub(t.elapsed()));
            }
            answers
        });
        let t0 = Instant::now();
        let mut sent = 0u64;
        let mut failed = None;
        while t0.elapsed() < STEP_TIME {
            // Paced: never ahead of `rate` (a small burst at most).
            let ahead = sent as f64 / bps - t0.elapsed().as_secs_f64();
            if ahead > 0.02 { tokio::time::sleep(Duration::from_secs_f64(ahead)).await; }
            match tokio::time::timeout(Duration::from_secs(5), send.write_all(&chunk)).await {
                Ok(Ok(())) => sent += chunk.len() as u64,
                Ok(Err(e)) => { failed = Some(e.to_string()); break; }
                Err(_) => break, // nothing moving for 5 s: the router (or link) choked
            }
        }
        let secs = t0.elapsed().as_secs_f64().max(0.001);
        watching.store(false, Ordering::Relaxed);
        let answers = watcher.await.unwrap_or_default();
        let mut worst = Health::Fine;
        let mut slowest = Duration::ZERO;
        for a in &answers {
            let h = judge.push(*a);
            slowest = slowest.max(a.unwrap_or(Duration::from_secs(1)));
            if h == Health::Trouble || (h == Health::Strained && worst == Health::Fine) { worst = h; }
        }
        if let Some(e) = &failed {
            log::info!("safe speed: the test connection failed at {rate} Mbps: {e}");
            worst = Health::Trouble;
        }
        let achieved = sent as f64 * 8.0 / secs / 1_000_000.0;
        log::info!("safe speed: step {rate} Mbps → sent {achieved:.0} Mbps, router {worst:?} (slowest {slowest:?})");
        progress(app, serde_json::json!({ "phase": "testing", "rateMbps": rate, "achievedMbps": achieved.round(), "health": worst, "routerMs": slowest.as_secs_f64() * 1000.0, "target": target }));
        if let Some(done) = ramp.finish_step(achieved, worst) { break done; }
        if started.elapsed() > TEST_MAX_TIME - STEP_TIME * 2 {
            break Outcome::NoLimit { top_mbps: achieved.round() as u32 };
        }
    };
    let _ = send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(5), read_frame_cap(&mut recv, 4096)).await;
    Ok(Report { outcome, target, local, router_ms: normal.as_secs_f64() * 1000.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One speed test at a time per peer; another peer isn't held up.
    #[test]
    fn one_speed_test_per_peer() {
        let who = format!("peer-{}", uuid::Uuid::new_v4());
        let first = TestSlot::take(&who).unwrap();
        assert!(TestSlot::take(&who).is_none());
        assert!(TestSlot::take(&format!("{who}-b")).is_some());
        drop(first);
        assert!(TestSlot::take(&who).is_some());
    }

    fn ms(n: u64) -> Option<Duration> {
        Some(Duration::from_millis(n))
    }

    #[test]
    fn echo_packets_are_well_formed_and_replies_are_matched() {
        let p = echo_request(7);
        assert_eq!(p[0], 8);
        assert_eq!(checksum(&p), 0, "a correct checksum sums to zero");
        // A Linux ping-socket reply (ICMP only) and a macOS one (with a 20-byte IP header).
        let mut reply = p;
        reply[0] = 0;
        assert_eq!(echo_reply_seq(&reply), Some(7));
        let mut with_ip = vec![0x45u8];
        with_ip.extend([0u8; 19]);
        with_ip.extend(reply);
        assert_eq!(echo_reply_seq(&with_ip), Some(7));
        assert_eq!(echo_reply_seq(&p), None, "our own request is not a reply");
        assert_eq!(echo_reply_seq(&[0, 0]), None);
    }

    #[test]
    fn one_slow_wifi_answer_is_not_trouble_but_a_pattern_is() {
        let mut j = Judge::default();
        for r in [3, 4, 2, 5, 3] {
            j.idle(ms(r));
        }
        assert_eq!(j.normal(), ms(3));
        assert_eq!(j.slow_after(), Duration::from_millis(83));
        assert_eq!(j.push(ms(4)), Health::Fine);
        assert_eq!(j.push(ms(60)), Health::Fine, "Wi-Fi jitter");
        assert_eq!(j.push(ms(200)), Health::Strained);
        assert_eq!(j.push(ms(250)), Health::Strained);
        assert_eq!(j.push(ms(300)), Health::Trouble, "three slow answers in five");
        j.reset_recent();
        assert_eq!(j.push(None), Health::Strained);
        assert_eq!(j.push(None), Health::Trouble, "two missed answers");
    }

    #[test]
    fn the_ramp_recommends_80_percent_of_the_last_speed_the_router_handled() {
        let mut r = Ramp::default();
        assert_eq!(r.rate_mbps(), 25);
        assert_eq!(r.finish_step(24.0, Health::Fine), None);
        assert_eq!(r.finish_step(49.0, Health::Fine), None);
        assert_eq!(r.finish_step(74.0, Health::Strained), None);
        assert_eq!(r.rate_mbps(), 100);
        assert_eq!(r.finish_step(98.0, Health::Fine), None);
        assert_eq!(r.finish_step(140.0, Health::Trouble), Some(Outcome::Limit { safe_mbps: 78, trouble_mbps: 140 }));
        // Trouble at the very first step: 80% of what it carried, at least 5.
        let mut r = Ramp::default();
        assert_eq!(r.finish_step(4.0, Health::Trouble), Some(Outcome::Limit { safe_mbps: 5, trouble_mbps: 4 }));
    }

    #[test]
    fn a_router_that_copes_with_full_speed_needs_no_limit() {
        let mut r = Ramp::default();
        // The connection tops out around 90 Mbps; the router never minds.
        for achieved in [25.0, 50.0, 74.0, 90.0, 91.0] {
            assert_eq!(r.finish_step(achieved, Health::Fine), None);
        }
        assert_eq!(r.finish_step(90.5, Health::Fine), Some(Outcome::NoLimit { top_mbps: 91 }));
    }

    #[test]
    fn the_guard_backs_off_fast_and_recovers_slowly() {
        let mb = |m: u64| m * 1_000_000 / 8;
        // Trouble at 400 Mbps with no cap: hold to 70%.
        assert_eq!(next_guard(0, mb(400), Health::Trouble, Duration::ZERO), mb(280));
        // More trouble: 70% again, never under 2 Mbps.
        assert_eq!(next_guard(mb(280), mb(270), Health::Trouble, Duration::ZERO), mb(189));
        assert_eq!(next_guard(mb(2), mb(1), Health::Trouble, Duration::ZERO), mb(2));
        // Strained or freshly calm: hold.
        assert_eq!(next_guard(mb(280), mb(270), Health::Strained, Duration::from_secs(60)), mb(280));
        assert_eq!(next_guard(mb(280), mb(270), Health::Fine, Duration::from_secs(3)), mb(280));
        // Calm 10 s: +10%; well above what's sent: lifted.
        assert_eq!(next_guard(mb(100), mb(100), Health::Fine, Duration::from_secs(10)), mb(110));
        assert_eq!(next_guard(mb(300), mb(100), Health::Fine, Duration::from_secs(10)), 0);
        assert_eq!(next_guard(0, mb(500), Health::Fine, Duration::from_secs(99)), 0, "no cap stays none");
    }
}
