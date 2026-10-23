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
    rec_id: String,
    call_id: String,
    dir: PathBuf,
    files: HashMap<String, (String, std::fs::File)>,
    started_ms: u64,
    name: String,
    video: bool,
    outgoing: bool,
}

static RECS: Mutex<Option<HashMap<String, Recording>>> = Mutex::new(None);
/// Recordings finished in the last while (rec id → folder, name): the iPhone
/// encodes each track to a file and hands it over when it stops — which can be
/// just after the call (and so the recording) ended.
static RECENT: Mutex<Option<HashMap<String, (PathBuf, String, u64)>>> = Mutex::new(None);
const RECENT_MS: u64 = 10 * 60 * 1000;

fn remember_recent(rec_id: &str, dir: &Path, name: &str) {
    let mut g = RECENT.lock().unwrap_or_else(|p| p.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    let now = super::now_ms();
    m.retain(|_, (_, _, at)| now.saturating_sub(*at) < RECENT_MS);
    m.insert(rec_id.to_string(), (dir.to_path_buf(), name.to_string(), now));
}

fn track_file(track: &str, name: &str, ext: &str) -> String {
    let label = match track { "me" => "Me".to_string(), "them" => safe_name(name), _ => "Call".to_string() };
    format!("{label}.{ext}")
}

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
    with(|r| r.insert(rec_id.clone(), Recording { rec_id: rec_id.clone(), call_id: id.into(), dir: dir.clone(), files: HashMap::new(), started_ms: super::now_ms(), name, video, outgoing }));
    super::record_notice(calls, id, "started");
    Ok(json!({ "recId": rec_id, "dir": dir }))
}

/// Append recorded bytes to one track: "me", "them" or "mixed".
pub fn chunk(rec_id: &str, track: &str, ext: &str, bytes: &[u8]) -> Result<()> {
    anyhow::ensure!(matches!(track, "me" | "them" | "mixed"), "Unknown track.");
    anyhow::ensure!(matches!(ext, "webm" | "mp4" | "m4a" | "ogg"), "Unknown format.");
    let late = with(|r| !r.contains_key(rec_id));
    if late {
        // The call ended and closed the recording a moment before the screen's
        // last chunks arrived: append them to the same files (for 10 minutes).
        let (dir, name, _) = RECENT.lock().unwrap_or_else(|p| p.into_inner()).as_ref()
            .and_then(|m| m.get(rec_id)).cloned().context("This recording has stopped.")?;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(track_file(track, &name, ext)))?;
        f.write_all(bytes)?;
        return Ok(());
    }
    with(|r| {
        let rec = r.get_mut(rec_id).context("This recording has stopped.")?;
        if !rec.files.contains_key(track) {
            let file = track_file(track, &rec.name, ext);
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
    let Some(rec) = with(|r| r.remove(rec_id)) else {
        // Already closed with the call: its folder is still the answer.
        let g = RECENT.lock().unwrap_or_else(|p| p.into_inner());
        let (dir, _, _) = g.as_ref().and_then(|m| m.get(rec_id)).cloned().context("This recording has stopped.")?;
        return Ok(json!({ "dir": dir }));
    };
    remember_recent(rec_id, &rec.dir, &rec.name);
    finish(calls, rec)
}

/// The iPhone's finished track file (AAC, written on the device): moved into
/// the recording's folder and listed in call.json — also shortly after the
/// recording was closed with the call. Only files the app wrote to its own temp
/// folder (`dropbeam-call-…`) are taken.
pub fn import_file(rec_id: &str, track: &str, ext: &str, src: &Path) -> Result<Value> {
    anyhow::ensure!(matches!(track, "me" | "them" | "mixed"), "Unknown track.");
    anyhow::ensure!(matches!(ext, "m4a" | "webm" | "mp4" | "ogg" | "wav"), "Unknown format.");
    let tmp = std::fs::canonicalize(std::env::temp_dir()).unwrap_or_else(|_| std::env::temp_dir());
    let src = std::fs::canonicalize(src).context("The recorded file is missing.")?;
    let ok_name = src.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("dropbeam-call-"));
    anyhow::ensure!(src.starts_with(&tmp) && ok_name && std::fs::symlink_metadata(&src)?.is_file(), "Not a recorded call file.");
    // Open recording: the engine still owns call.json; register the file there.
    let open = with(|r| r.get(rec_id).map(|x| (x.dir.clone(), x.name.clone())));
    let (dir, name, open) = match open {
        Some((d, n)) => (d, n, true),
        None => {
            let g = RECENT.lock().unwrap_or_else(|p| p.into_inner());
            let (d, n, _) = g.as_ref().and_then(|m| m.get(rec_id)).cloned().context("This recording has stopped.")?;
            (d, n, false)
        }
    };
    let file = track_file(track, &name, ext);
    let dest = dir.join(&file);
    anyhow::ensure!(!dest.exists(), "That track is already saved.");
    if std::fs::rename(&src, &dest).is_err() {
        std::fs::copy(&src, &dest)?;
        let _ = std::fs::remove_file(&src);
    }
    if open {
        with(|r| {
            if let Some(rec) = r.get_mut(rec_id) {
                if let Ok(f) = std::fs::OpenOptions::new().append(true).open(&dest) { rec.files.insert(track.to_string(), (file.clone(), f)); }
            }
        });
    } else {
        // Closed already: add the track to call.json.
        let meta_path = dir.join("call.json");
        if let Ok(mut meta) = std::fs::read(&meta_path).map_err(anyhow::Error::from).and_then(|b| Ok(serde_json::from_slice::<Value>(&b)?)) {
            let who = match track { "me" => "me".to_string(), "them" => name.clone(), _ => "both".to_string() };
            meta["tracks"][track] = json!({ "file": file, "who": who });
            crate::settings::write_atomic(&meta_path, &serde_json::to_vec_pretty(&meta)?)?;
        }
    }
    Ok(json!({ "dir": dir, "file": dest }))
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

fn rec_id_of(r: &Recording) -> String { r.rec_id.clone() }

/// The call ended: close whatever is still recording for it.
pub fn close_for_call(id: &str) {
    let open: Vec<Recording> = with(|r| {
        let keys: Vec<String> = r.iter().filter(|(_, x)| x.call_id == id).map(|(k, _)| k.clone()).collect();
        keys.into_iter().filter_map(|k| r.remove(&k)).collect()
    });
    for rec in open {
        remember_recent(&rec_id_of(&rec), &rec.dir, &rec.name);
        let _ = finish(None, rec);
    }
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
    fn a_finished_track_file_joins_the_recording_even_after_it_closed() {
        let tmp = std::env::temp_dir();
        let dir = tmp.join(format!("dropbeam-rec-test-{}", rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let rec_id = format!("r{}", rand::random::<u32>());
        with(|r| r.insert(rec_id.clone(), Recording { rec_id: rec_id.clone(), call_id: "c".into(), dir: dir.clone(), files: HashMap::new(),
            started_ms: 1, name: "Bea".into(), video: false, outgoing: true }));
        // Open: imported, then listed when the recording closes.
        let me = tmp.join(format!("dropbeam-call-{}-me.m4a", rand::random::<u32>()));
        std::fs::write(&me, b"aac").unwrap();
        import_file(&rec_id, "me", "m4a", &me).unwrap();
        assert!(!me.exists());
        close(None, &rec_id).unwrap();
        let meta: Value = serde_json::from_slice(&std::fs::read(dir.join("call.json")).unwrap()).unwrap();
        assert_eq!(meta["tracks"]["me"]["file"], "Me.m4a");
        // Closed: still accepted for a while, and call.json learns it.
        let them = tmp.join(format!("dropbeam-call-{}-them.m4a", rand::random::<u32>()));
        std::fs::write(&them, b"aac").unwrap();
        import_file(&rec_id, "them", "m4a", &them).unwrap();
        let meta: Value = serde_json::from_slice(&std::fs::read(dir.join("call.json")).unwrap()).unwrap();
        assert_eq!(meta["tracks"]["them"]["file"], "Bea.m4a");
        assert_eq!(std::fs::read(dir.join("Bea.m4a")).unwrap(), b"aac");
        // Only the app's own temp recordings are taken.
        let other = tmp.join(format!("notes-{}.m4a", rand::random::<u32>()));
        std::fs::write(&other, b"x").unwrap();
        assert!(import_file(&rec_id, "mixed", "m4a", &other).is_err());
        assert!(import_file(&rec_id, "mixed", "m4a", Path::new("/etc/hosts")).is_err());
        assert!(import_file("nope", "mixed", "m4a", &other).is_err());
        let _ = std::fs::remove_file(&other);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_are_safe_for_folders() {
        assert_eq!(safe_name("../../etc"), "_.._etc");
        assert_eq!(safe_name("Ann/Bob:\u{0}"), "Ann_Bob__");
        assert_eq!(safe_name("..."), "Friend");
        assert_eq!(safe_name("Zoë O'Neil"), "Zoë O'Neil");
    }
}
