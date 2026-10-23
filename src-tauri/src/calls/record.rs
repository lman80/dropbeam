//! Recording a call — on this device only, never through any server, and only
//! with the other person's yes.
//!
//! Consent runs through the call stream: the recorder's screen sends
//! `call.record {action:"ask"}`, the other side answers `allow` or `deny`.
//! `open` refuses until an `allow` arrived for THIS call, and the engine itself
//! sends `started`/`stopped`, so the other side's "Recording" badge can't be
//! skipped by a modified call screen on this side.
//!
//! Files land in `<Downloads>/DropBeam Calls/<date time> <name>/`: one file per
//! voice (`Me.webm`, `<Name>.webm`), the mixed call (`Call.webm`, with video when
//! there was any) and `call.json` saying who is in which file.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

use super::Calls;

#[derive(Default, Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Consent {
    /// I asked to record; waiting or answered.
    pub asked_by_me: bool,
    /// They said yes to my recording.
    pub they_allow: bool,
    /// They asked to record me.
    pub asked_by_them: bool,
    /// I said yes to theirs.
    pub i_allow: bool,
    pub me_recording: bool,
    pub them_recording: bool,
    /// After the call each may send the other their own clean voice track —
    /// only when both said so.
    pub i_share: bool,
    pub they_share: bool,
}

impl Consent {
    pub fn note(&mut self, action: &str, from_me: bool) {
        match (action, from_me) {
            ("ask", true) => { self.asked_by_me = true; self.they_allow = false; }
            ("ask", false) => { self.asked_by_them = true; self.i_allow = false; }
            ("allow", true) if self.asked_by_them => self.i_allow = true,
            ("allow", false) if self.asked_by_me => self.they_allow = true,
            ("deny", true) => { self.i_allow = false; self.asked_by_them = false; }
            ("deny", false) => { self.they_allow = false; self.asked_by_me = false; }
            ("started", true) => self.me_recording = true,
            ("started", false) => self.them_recording = true,
            ("stopped", true) => self.me_recording = false,
            ("stopped", false) => self.them_recording = false,
            ("share", true) => self.i_share = true,
            ("share", false) => self.they_share = true,
            _ => {}
        }
    }
}

struct Recording {
    call_id: String,
    dir: PathBuf,
    files: HashMap<String, (String, std::fs::File)>,
    started_ms: u64,
    name: String,
    video: bool,
    outgoing: bool,
}

static RECS: Mutex<Option<HashMap<String, Recording>>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut HashMap<String, Recording>) -> R) -> R {
    let mut g = RECS.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

/// A file-system-safe folder/file name.
pub fn safe_name(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_alphanumeric() || " -_.'()".contains(c) { c } else { '_' }).take(60).collect();
    let t = t.trim().trim_matches('.').to_string();
    if t.is_empty() { "Friend".into() } else { t }
}

pub fn calls_dir(download: &Path) -> PathBuf { download.join("DropBeam Calls") }

/// Start recording call `id`: only after the other person allowed it.
pub fn open(calls: &Arc<Calls>, id: &str, download: &Path) -> Result<Value> {
    let (name, video, outgoing) = {
        let g = calls.live.lock().unwrap();
        let l = g.as_ref().filter(|l| l.call.id == id).context("This call has ended.")?;
        anyhow::ensure!(l.call.phase == super::Phase::Active || l.call.phase == super::Phase::Connecting, "The call isn't connected.");
        anyhow::ensure!(l.rec.they_allow, "They haven't allowed recording.");
        (l.name.clone(), l.call.video, l.call.dir == super::Dir::Out)
    };
    if let Some(existing) = with(|r| r.iter().find(|(_, x)| x.call_id == id).map(|(k, x)| (k.clone(), x.dir.clone()))) {
        return Ok(json!({ "recId": existing.0, "dir": existing.1 }));
    }
    let stamp = chrono::Local::now().format("%Y-%m-%d %H.%M").to_string();
    let base = calls_dir(download);
    let mut dir = base.join(format!("{stamp} {}", safe_name(&name)));
    let mut n = 2;
    while dir.exists() { dir = base.join(format!("{stamp} {} ({n})", safe_name(&name))); n += 1; }
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let rec_id = uuid::Uuid::new_v4().to_string();
    with(|r| r.insert(rec_id.clone(), Recording { call_id: id.into(), dir: dir.clone(), files: HashMap::new(), started_ms: super::now_ms(), name, video, outgoing }));
    super::record_notice(calls, id, "started");
    Ok(json!({ "recId": rec_id, "dir": dir }))
}

/// Append recorded bytes to one track: "me", "them" or "mixed".
pub fn chunk(rec_id: &str, track: &str, ext: &str, bytes: &[u8]) -> Result<()> {
    anyhow::ensure!(matches!(track, "me" | "them" | "mixed"), "Unknown track.");
    anyhow::ensure!(matches!(ext, "webm" | "mp4" | "m4a" | "ogg"), "Unknown format.");
    with(|r| {
        let rec = r.get_mut(rec_id).context("This recording has stopped.")?;
        if !rec.files.contains_key(track) {
            let label = match track { "me" => "Me".to_string(), "them" => safe_name(&rec.name), _ => "Call".to_string() };
            let file = format!("{label}.{ext}");
            let f = std::fs::OpenOptions::new().create_new(true).write(true).open(rec.dir.join(&file))?;
            rec.files.insert(track.to_string(), (file, f));
        }
        let (_, f) = rec.files.get_mut(track).unwrap();
        f.write_all(bytes)?;
        Ok(())
    })
}

/// Finish: flush the files and write call.json. Returns the folder.
pub fn close(calls: Option<&Arc<Calls>>, rec_id: &str) -> Result<Value> {
    let rec = with(|r| r.remove(rec_id)).context("This recording has stopped.")?;
    finish(calls, rec)
}

fn finish(calls: Option<&Arc<Calls>>, rec: Recording) -> Result<Value> {
    let mut tracks = serde_json::Map::new();
    for (track, (file, f)) in &rec.files {
        let _ = f.sync_all();
        let who = match track.as_str() { "me" => "me".to_string(), "them" => rec.name.clone(), _ => "both".to_string() };
        tracks.insert(track.clone(), json!({ "file": file, "who": who }));
    }
    let meta = json!({
        "app": "DropBeam", "v": 1, "callId": rec.call_id, "with": rec.name, "outgoing": rec.outgoing, "video": rec.video,
        "startedAt": rec.started_ms, "endedAt": super::now_ms(), "tracks": tracks,
        "note": "Recorded on this device with both people's agreement. Nothing was uploaded anywhere.",
    });
    crate::settings::write_atomic(&rec.dir.join("call.json"), &serde_json::to_vec_pretty(&meta)?)?;
    if let Some(c) = calls { super::record_notice(c, &rec.call_id, "stopped"); }
    Ok(json!({ "dir": rec.dir, "meta": meta }))
}

/// The call ended: close whatever is still recording for it.
pub fn close_for_call(id: &str) {
    let open: Vec<Recording> = with(|r| {
        let keys: Vec<String> = r.iter().filter(|(_, x)| x.call_id == id).map(|(k, _)| k.clone()).collect();
        keys.into_iter().filter_map(|k| r.remove(&k)).collect()
    });
    for rec in open { let _ = finish(None, rec); }
}

/// Saved recordings, newest first.
pub fn list(download: &Path) -> Vec<Value> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(calls_dir(download)) else { return out };
    for e in rd.flatten() {
        let meta = e.path().join("call.json");
        if let Ok(v) = std::fs::read(&meta).map_err(anyhow::Error::from).and_then(|b| Ok(serde_json::from_slice::<Value>(&b)?)) {
            out.push(json!({ "dir": e.path(), "meta": v }));
        }
    }
    out.sort_by(|a, b| b["meta"]["startedAt"].as_u64().cmp(&a["meta"]["startedAt"].as_u64()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consent_needs_an_ask_before_an_allow() {
        let mut c = Consent::default();
        c.note("allow", false); // unsolicited yes counts for nothing
        assert!(!c.they_allow);
        c.note("ask", true);
        c.note("allow", false);
        assert!(c.they_allow);
        c.note("deny", false); // they can take it back
        assert!(!c.they_allow && !c.asked_by_me);
        c.note("ask", false);
        c.note("allow", true);
        assert!(c.i_allow);
        c.note("started", false);
        assert!(c.them_recording);
        c.note("stopped", false);
        assert!(!c.them_recording);
    }

    #[test]
    fn names_are_safe_for_folders() {
        assert_eq!(safe_name("../../etc"), "_.._etc");
        assert_eq!(safe_name("Ann/Bob:\u{0}"), "Ann_Bob__");
        assert_eq!(safe_name("..."), "Friend");
        assert_eq!(safe_name("Zoë O'Neil"), "Zoë O'Neil");
    }
}
