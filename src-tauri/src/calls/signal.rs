//! The call state machine, free of I/O so every rule is unit-tested: ringing
//! several devices of one person (first to answer wins, the others stop),
//! busy / declined / missed / no answer, timeouts and hang-ups. The async glue in
//! `mod.rs` turns `Act`s into frames on each device's call stream.
//!
//! One call at a time per device. Glare (two people calling each other at the
//! same moment) is settled in `glare_winner` before a second call is created.

use serde::Serialize;
use serde_json::{json, Value};

/// How long an unanswered call rings.
pub const RING_MS: u64 = 45_000;
/// The caller gives up a little later than the callee stops ringing, so the
/// callee's "missed" normally arrives first and the caller reads "No answer".
pub const CALLER_RING_MS: u64 = 50_000;
/// From "answered" until media must flow.
pub const CONNECT_MS: u64 = 30_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir { Out, In }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase { Ringing, Connecting, Active, Ended }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum End {
    /// We hung up after it was answered.
    Hangup,
    /// They hung up.
    RemoteHangup,
    /// The callee said no.
    Declined,
    /// Every device was in another call.
    Busy,
    /// Rang out (caller side).
    NoAnswer,
    /// Rang out or the caller gave up first (callee side).
    Missed,
    /// The caller stopped before anyone answered.
    Canceled,
    /// Another of the callee's own devices took it.
    AnsweredElsewhere,
    /// No device could be reached or none can take calls.
    Unavailable,
    /// Answered, but media never started, or the connection died.
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LegState { Dialing, Ringing, Answered, Busy, Declined, Missed, Unavailable, Failed, Gone }

impl LegState {
    fn done(self) -> bool { !matches!(self, LegState::Dialing | LegState::Ringing | LegState::Answered) }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Leg { pub endpoint: String, pub state: LegState }

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Call {
    pub id: String,
    pub dir: Dir,
    /// The friend record that owns this person's chat thread.
    pub person: String,
    pub video: bool,
    pub phase: Phase,
    pub started_ms: u64,
    pub answered_ms: Option<u64>,
    pub media_ms: Option<u64>,
    pub ended_ms: Option<u64>,
    pub end: Option<End>,
    /// Caller: one per device of the callee. Callee: the caller's device.
    pub legs: Vec<Leg>,
    /// The device carrying the call once answered.
    pub active: Option<String>,
}

/// Something the glue must do.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// Write a frame on that device's call stream.
    Send(String, Value),
    /// Finish that device's stream (it's out of the call).
    Close(String),
    /// Media goes over that device's connection from now on.
    Media(String),
    /// The call ended (record it, stop media, close every stream).
    Ended(End),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ev {
    // From this device's person.
    Accept,
    Decline,
    Hangup,
    /// The call screen reports media is flowing.
    MediaUp,
    // From a device's stream.
    Ringing(String),
    Answered(String),
    Busy(String),
    Declined(String),
    Missed(String),
    Unavailable(String),
    /// The stream ended without a reason (dial failed, connection lost).
    Lost(String),
    RemoteHangup(String),
    Canceled(String),
    Taken(String),
    /// A clock tick.
    Tick(u64),
}

impl Call {
    pub fn outgoing(id: &str, person: &str, video: bool, devices: &[String], now: u64) -> Call {
        Call { id: id.into(), dir: Dir::Out, person: person.into(), video, phase: Phase::Ringing, started_ms: now,
            answered_ms: None, media_ms: None, ended_ms: None, end: None,
            legs: devices.iter().map(|d| Leg { endpoint: d.clone(), state: LegState::Dialing }).collect(), active: None }
    }

    pub fn incoming(id: &str, person: &str, video: bool, from: &str, now: u64) -> (Call, Vec<Act>) {
        let c = Call { id: id.into(), dir: Dir::In, person: person.into(), video, phase: Phase::Ringing, started_ms: now,
            answered_ms: None, media_ms: None, ended_ms: None, end: None,
            legs: vec![Leg { endpoint: from.into(), state: LegState::Ringing }], active: None };
        (c, vec![Act::Send(from.into(), json!({"kind": "call.ring"}))])
    }

    pub fn ended(&self) -> bool { self.phase == Phase::Ended }

    fn leg(&mut self, ep: &str) -> Option<&mut Leg> { self.legs.iter_mut().find(|l| l.endpoint == ep) }

    fn finish(&mut self, end: End, now: u64, acts: &mut Vec<Act>) {
        if self.ended() { return; }
        self.phase = Phase::Ended;
        self.end = Some(end);
        self.ended_ms = Some(now);
        for l in &mut self.legs {
            acts.push(Act::Close(l.endpoint.clone()));
            if !l.state.done() { l.state = LegState::Gone; }
        }
        acts.push(Act::Ended(end));
    }

    /// Caller: tell every device still ringing that it's over.
    fn cancel_others(&mut self, keep: Option<&str>, kind: &str, acts: &mut Vec<Act>) {
        for l in &mut self.legs {
            if Some(l.endpoint.as_str()) != keep && !l.state.done() {
                acts.push(Act::Send(l.endpoint.clone(), json!({"kind": kind})));
                acts.push(Act::Close(l.endpoint.clone()));
                l.state = LegState::Gone;
            }
        }
    }

    /// Caller: once no device can still answer, the call is over — busy if any
    /// device was busy, missed if any rang, else unreachable.
    fn settle(&mut self, now: u64, acts: &mut Vec<Act>) {
        if self.phase != Phase::Ringing || self.legs.iter().any(|l| !l.state.done()) { return; }
        let end = if self.legs.iter().any(|l| l.state == LegState::Busy) { End::Busy }
            else if self.legs.iter().any(|l| l.state == LegState::Missed) { End::NoAnswer }
            else { End::Unavailable };
        self.finish(end, now, acts);
    }

    pub fn on(&mut self, ev: Ev, now: u64) -> Vec<Act> {
        let mut acts = Vec::new();
        if self.ended() { return acts; }
        let active = self.active.clone();
        let is_active = |ep: &str| active.as_deref() == Some(ep);
        match (self.dir, ev) {
            (_, Ev::Tick(t)) => {
                match self.phase {
                    Phase::Ringing if self.dir == Dir::Out && t >= self.started_ms + CALLER_RING_MS => {
                        self.cancel_others(None, "call.cancel", &mut acts);
                        self.finish(End::NoAnswer, t, &mut acts);
                    }
                    Phase::Ringing if self.dir == Dir::In && t >= self.started_ms + RING_MS => {
                        let ep = self.legs[0].endpoint.clone();
                        acts.push(Act::Send(ep, json!({"kind": "call.missed"})));
                        self.finish(End::Missed, t, &mut acts);
                    }
                    Phase::Connecting if t >= self.answered_ms.unwrap_or(t) + CONNECT_MS => {
                        if let Some(ep) = active { acts.push(Act::Send(ep, json!({"kind": "call.hangup", "reason": "failed"}))); }
                        self.finish(End::Failed, t, &mut acts);
                    }
                    _ => {}
                }
            }
            (_, Ev::MediaUp) => {
                if self.phase == Phase::Connecting {
                    self.phase = Phase::Active;
                    self.media_ms = Some(now);
                }
            }
            (_, Ev::Hangup) => match (self.dir, self.phase) {
                (Dir::Out, Phase::Ringing) => {
                    self.cancel_others(None, "call.cancel", &mut acts);
                    self.finish(End::Canceled, now, &mut acts);
                }
                (Dir::In, Phase::Ringing) => {
                    let ep = self.legs[0].endpoint.clone();
                    acts.push(Act::Send(ep, json!({"kind": "call.decline"})));
                    self.finish(End::Declined, now, &mut acts);
                }
                _ => {
                    if let Some(ep) = active { acts.push(Act::Send(ep, json!({"kind": "call.hangup"}))); }
                    self.finish(End::Hangup, now, &mut acts);
                }
            },
            (_, Ev::RemoteHangup(ep)) if is_active(&ep) => self.finish(End::RemoteHangup, now, &mut acts),
            (_, Ev::Lost(ep)) if is_active(&ep) => self.finish(End::Failed, now, &mut acts),

            // ── caller ──
            (Dir::Out, Ev::Ringing(ep)) => {
                if let Some(l) = self.leg(&ep).filter(|l| l.state == LegState::Dialing) { l.state = LegState::Ringing; }
            }
            (Dir::Out, Ev::Answered(ep)) => {
                let ringing = self.legs.iter().any(|l| l.endpoint == ep && !l.state.done());
                if self.phase == Phase::Ringing && ringing {
                    if let Some(l) = self.leg(&ep) { l.state = LegState::Answered; }
                    self.active = Some(ep.clone());
                    self.phase = Phase::Connecting;
                    self.answered_ms = Some(now);
                    self.cancel_others(Some(&ep), "call.taken", &mut acts);
                    acts.push(Act::Media(ep));
                } else if !is_active(&ep) {
                    // A second device answered a moment too late.
                    acts.push(Act::Send(ep.clone(), json!({"kind": "call.taken"})));
                    acts.push(Act::Close(ep.clone()));
                    if let Some(l) = self.leg(&ep) { l.state = LegState::Gone; }
                }
            }
            (Dir::Out, Ev::Declined(ep)) if self.phase == Phase::Ringing && self.legs.iter().any(|l| l.endpoint == ep) => {
                // A decline on any of their devices is the person saying no.
                if let Some(l) = self.leg(&ep) { l.state = LegState::Declined; }
                self.cancel_others(Some(&ep), "call.cancel", &mut acts);
                self.finish(End::Declined, now, &mut acts);
            }
            (Dir::Out, ev @ (Ev::Busy(_) | Ev::Missed(_) | Ev::Unavailable(_) | Ev::Lost(_) | Ev::Declined(_) | Ev::RemoteHangup(_))) => {
                let (ep, st) = match ev {
                    Ev::Busy(e) => (e, LegState::Busy),
                    Ev::Missed(e) => (e, LegState::Missed),
                    Ev::Unavailable(e) => (e, LegState::Unavailable),
                    Ev::Declined(e) => (e, LegState::Declined),
                    Ev::RemoteHangup(e) | Ev::Lost(e) => (e, LegState::Failed),
                    _ => unreachable!(),
                };
                if let Some(l) = self.leg(&ep).filter(|l| !l.state.done()) {
                    l.state = st;
                    acts.push(Act::Close(ep));
                }
                self.settle(now, &mut acts);
            }

            // ── callee ──
            (Dir::In, Ev::Accept) if self.phase == Phase::Ringing => {
                let ep = self.legs[0].endpoint.clone();
                self.legs[0].state = LegState::Answered;
                self.active = Some(ep.clone());
                self.phase = Phase::Connecting;
                self.answered_ms = Some(now);
                acts.push(Act::Media(ep));
            }
            (Dir::In, Ev::Decline) if self.phase == Phase::Ringing => {
                let ep = self.legs[0].endpoint.clone();
                acts.push(Act::Send(ep, json!({"kind": "call.decline"})));
                self.finish(End::Declined, now, &mut acts);
            }
            (Dir::In, Ev::Canceled(ep) | Ev::Lost(ep)) if self.legs[0].endpoint == ep => {
                let end = if self.phase == Phase::Ringing { End::Missed } else { End::Failed };
                self.finish(end, now, &mut acts);
            }
            (Dir::In, Ev::Taken(ep)) if self.legs[0].endpoint == ep => self.finish(End::AnsweredElsewhere, now, &mut acts),
            _ => {}
        }
        acts
    }

    /// Talk time in ms, if it was ever answered.
    pub fn duration_ms(&self) -> Option<u64> {
        let from = self.media_ms.or(self.answered_ms)?;
        Some(self.ended_ms.unwrap_or(from).saturating_sub(from))
    }
}

/// Two people called each other at the same moment. Both sides compare the
/// two call ids the same way: the smaller id survives; the other call is
/// dropped by its caller and the surviving one is answered automatically
/// (each of them was trying to reach the other).
pub fn glare_winner<'a>(mine: &'a str, theirs: &'a str) -> &'a str {
    if mine < theirs { mine } else { theirs }
}

/// Words for a call's history line in the chat thread.
pub fn history_text(c: &Call) -> String {
    let what = if c.video { "Video call" } else { "Audio call" };
    let lower = if c.video { "video call" } else { "audio call" };
    let dur = |ms: u64| { let s = ms / 1000; if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) } };
    match (c.dir, c.end) {
        (_, Some(End::Hangup | End::RemoteHangup)) | (_, Some(End::Failed)) if c.media_ms.is_some() => format!("{what} · {}", dur(c.duration_ms().unwrap_or(0))),
        (Dir::In, Some(End::Missed)) => format!("Missed {lower}"),
        (Dir::In, Some(End::Declined)) => format!("Declined {lower}"),
        (Dir::Out, Some(End::Declined)) => format!("{what} · Declined"),
        (Dir::Out, Some(End::Busy)) => format!("{what} · Busy"),
        (Dir::Out, Some(End::NoAnswer)) => format!("{what} · No answer"),
        (Dir::Out, Some(End::Canceled)) => format!("{what} · Canceled"),
        (_, Some(End::Unavailable)) => format!("{what} · Couldn’t connect"),
        _ => format!("{what} · Couldn’t connect"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devs(n: usize) -> Vec<String> { (0..n).map(|i| format!("dev{i}")).collect() }
    fn sends(acts: &[Act]) -> Vec<(String, String)> {
        acts.iter().filter_map(|a| match a { Act::Send(ep, v) => Some((ep.clone(), v["kind"].as_str().unwrap().to_string())), _ => None }).collect()
    }

    #[test]
    fn first_device_to_answer_wins_and_the_others_stop() {
        let mut c = Call::outgoing("c1", "p", true, &devs(3), 0);
        c.on(Ev::Ringing("dev0".into()), 1);
        c.on(Ev::Ringing("dev1".into()), 1);
        c.on(Ev::Ringing("dev2".into()), 1);
        let acts = c.on(Ev::Answered("dev1".into()), 5);
        assert_eq!(c.phase, Phase::Connecting);
        assert_eq!(c.active.as_deref(), Some("dev1"));
        assert_eq!(sends(&acts), vec![("dev0".into(), "call.taken".into()), ("dev2".into(), "call.taken".into())]);
        assert!(acts.contains(&Act::Media("dev1".into())));
        // dev2 answered too, a moment later: told it was taken, call unaffected.
        let acts = c.on(Ev::Answered("dev2".into()), 6);
        assert_eq!(sends(&acts), vec![("dev2".into(), "call.taken".into())]);
        assert_eq!(c.active.as_deref(), Some("dev1"));
        c.on(Ev::MediaUp, 7);
        assert_eq!(c.phase, Phase::Active);
        let acts = c.on(Ev::Hangup, 67_007);
        assert_eq!(sends(&acts), vec![("dev1".into(), "call.hangup".into())]);
        assert_eq!(c.end, Some(End::Hangup));
        assert_eq!(c.duration_ms(), Some(67_000));
        assert_eq!(history_text(&c), "Video call · 1:07");
    }

    #[test]
    fn busy_unavailable_and_no_answer() {
        let mut c = Call::outgoing("c", "p", false, &devs(2), 0);
        c.on(Ev::Busy("dev0".into()), 1);
        assert_eq!(c.phase, Phase::Ringing);
        let acts = c.on(Ev::Lost("dev1".into()), 2);
        assert!(acts.contains(&Act::Ended(End::Busy)));
        assert_eq!(history_text(&c), "Audio call · Busy");

        let mut c = Call::outgoing("c", "p", false, &devs(2), 0);
        c.on(Ev::Unavailable("dev0".into()), 1);
        c.on(Ev::Lost("dev1".into()), 2);
        assert_eq!(c.end, Some(End::Unavailable));

        // One device misses it, the other was never reachable → No answer.
        let mut c = Call::outgoing("c", "p", false, &devs(2), 0);
        c.on(Ev::Ringing("dev0".into()), 1);
        c.on(Ev::Lost("dev1".into()), 1);
        c.on(Ev::Missed("dev0".into()), RING_MS);
        assert_eq!(c.end, Some(End::NoAnswer));

        // Nobody answers before the caller's own timer.
        let mut c = Call::outgoing("c", "p", false, &devs(1), 0);
        c.on(Ev::Ringing("dev0".into()), 1);
        assert!(c.on(Ev::Tick(CALLER_RING_MS - 1), CALLER_RING_MS - 1).is_empty());
        let acts = c.on(Ev::Tick(CALLER_RING_MS), CALLER_RING_MS);
        assert_eq!(sends(&acts), vec![("dev0".into(), "call.cancel".into())]);
        assert_eq!(c.end, Some(End::NoAnswer));
        assert_eq!(history_text(&c), "Audio call · No answer");
    }

    #[test]
    fn a_decline_on_any_device_ends_the_call_for_all() {
        let mut c = Call::outgoing("c", "p", true, &devs(3), 0);
        for d in devs(3) { c.on(Ev::Ringing(d), 1); }
        let acts = c.on(Ev::Declined("dev2".into()), 3);
        assert_eq!(sends(&acts), vec![("dev0".into(), "call.cancel".into()), ("dev1".into(), "call.cancel".into())]);
        assert_eq!(c.end, Some(End::Declined));
        assert_eq!(history_text(&c), "Video call · Declined");
        // Nothing happens after the end.
        assert!(c.on(Ev::Answered("dev0".into()), 4).is_empty());
    }

    #[test]
    fn caller_cancel_before_answer() {
        let mut c = Call::outgoing("c", "p", false, &devs(2), 0);
        c.on(Ev::Ringing("dev0".into()), 1);
        let acts = c.on(Ev::Hangup, 2);
        assert_eq!(sends(&acts), vec![("dev0".into(), "call.cancel".into()), ("dev1".into(), "call.cancel".into())]);
        assert_eq!(c.end, Some(End::Canceled));
    }

    #[test]
    fn callee_rings_then_missed_declined_taken_or_answered() {
        let (mut c, acts) = Call::incoming("c", "p", false, "caller", 0);
        assert_eq!(sends(&acts), vec![("caller".into(), "call.ring".into())]);
        let acts = c.on(Ev::Tick(RING_MS), RING_MS);
        assert_eq!(sends(&acts), vec![("caller".into(), "call.missed".into())]);
        assert_eq!(c.end, Some(End::Missed));
        assert_eq!(history_text(&c), "Missed audio call");

        let (mut c, _) = Call::incoming("c", "p", true, "caller", 0);
        c.on(Ev::Canceled("caller".into()), 5);
        assert_eq!(c.end, Some(End::Missed));
        assert_eq!(history_text(&c), "Missed video call");

        let (mut c, _) = Call::incoming("c", "p", true, "caller", 0);
        let acts = c.on(Ev::Decline, 5);
        assert_eq!(sends(&acts), vec![("caller".into(), "call.decline".into())]);
        assert_eq!(history_text(&c), "Declined video call");

        let (mut c, _) = Call::incoming("c", "p", true, "caller", 0);
        // A frame from some other device is not the caller's.
        assert!(c.on(Ev::Taken("other".into()), 5).is_empty());
        c.on(Ev::Taken("caller".into()), 5);
        assert_eq!(c.end, Some(End::AnsweredElsewhere));

        let (mut c, _) = Call::incoming("c", "p", false, "caller", 0);
        let acts = c.on(Ev::Accept, 10);
        assert_eq!(acts, vec![Act::Media("caller".into())]);
        c.on(Ev::MediaUp, 500);
        c.on(Ev::RemoteHangup("caller".into()), 3_600_500 + 61_000);
        assert_eq!(c.end, Some(End::RemoteHangup));
        assert_eq!(history_text(&c), "Audio call · 1:01:01");
    }

    #[test]
    fn media_must_start_soon_after_answer() {
        let mut c = Call::outgoing("c", "p", false, &devs(1), 0);
        c.on(Ev::Answered("dev0".into()), 100);
        let acts = c.on(Ev::Tick(100 + CONNECT_MS), 100 + CONNECT_MS);
        assert_eq!(sends(&acts), vec![("dev0".into(), "call.hangup".into())]);
        assert_eq!(c.end, Some(End::Failed));
        assert_eq!(history_text(&c), "Audio call · Couldn’t connect");
        // A lost connection mid-call ends it.
        let (mut c, _) = Call::incoming("c", "p", false, "caller", 0);
        c.on(Ev::Accept, 1);
        c.on(Ev::MediaUp, 2);
        c.on(Ev::Lost("caller".into()), 10_002);
        assert_eq!(c.end, Some(End::Failed));
        assert_eq!(history_text(&c), "Audio call · 0:10");
    }

    #[test]
    fn glare_is_settled_the_same_way_on_both_sides() {
        assert_eq!(glare_winner("a1", "b2"), "a1");
        assert_eq!(glare_winner("b2", "a1"), "a1");
    }
}
