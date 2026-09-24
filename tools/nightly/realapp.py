#!/usr/bin/env python3
"""Real-app cross-machine transfer tests driven through DropBeam's lab-mode
automation queue. Machines: m1 (this Mac), mac2 (ssh mac2), lin (ssh penis).

Each case: build a uniquely-named fixture on the SOURCE machine, queue the op
in the source app, wait for the app's terminal result, then sha256-verify the
landed files on the DESTINATION machine (in its download folder)."""
import json, os, shlex, subprocess, sys, time, uuid

MACH = {
    "m1":   {"ssh": None,    "cfg": "~/Library/Application Support/com.dropbeam.app"},
    "mac2": {"ssh": "mac2",  "cfg": "~/Library/Application Support/com.dropbeam.app"},
    "lin":  {"ssh": "penis", "cfg": "~/.config/com.dropbeam.app"},
}

def sh(m, cmd, timeout=600):
    """Run a shell command on machine m, return stdout."""
    host = MACH[m]["ssh"]
    full = cmd if host is None else f"ssh -o ServerAliveInterval=20 {host} {shlex.quote(cmd)}"
    r = subprocess.run(["bash", "-lc", full] if host is None else full, shell=host is not None,
                       capture_output=True, text=True, timeout=timeout)
    if r.returncode != 0:
        raise RuntimeError(f"[{m}] {cmd[:120]} -> {r.returncode}: {r.stderr.strip()[:400]}")
    return r.stdout

CFG = lambda m: MACH[m]["cfg"].replace("~", "$HOME")

GEN = r'''
import os, sys, random, hashlib, json
root = os.path.expanduser(sys.argv[1]); kind = sys.argv[2]; tag = sys.argv[3]
os.makedirs(root, exist_ok=True)
def w(p, n, seed):
    p = os.path.join(root, p); os.makedirs(os.path.dirname(p), exist_ok=True)
    r = random.Random(seed)
    with open(p, "wb") as f:
        left = n
        while left:
            c = min(left, 1 << 20); f.write(r.randbytes(c)); left -= c
paths = []
if kind == "photo":
    w(f"IMG_{tag}.jpg", 3_400_000, 1); paths = [f"IMG_{tag}.jpg"]
elif kind == "video":
    w(f"Clip {tag} 🎬.mov", 48_000_000, 2); paths = [f"Clip {tag} 🎬.mov"]
elif kind == "big":
    w(f"Big-{tag}.bin", 400_000_000, 3); paths = [f"Big-{tag}.bin"]
elif kind == "multi":
    for i in range(5): w(f"Doc {i} — {tag} (v2).pdf", 250_000 + i * 77_000, 10 + i)
    paths = [f"Doc {i} — {tag} (v2).pdf" for i in range(5)]
elif kind == "folder":
    d = f"Folder {tag}"
    w(f"{d}/notes.txt", 1200, 20); w(f"{d}/Sub/Café ☕️ {tag}.txt", 3000, 21)
    w(f"{d}/Sub/Deeper ünïcödé/report.bin", 2_500_000, 22); w(f"{d}/zero.bin", 0, 23)
    os.makedirs(os.path.join(root, d, "empty dir"), exist_ok=True)
    w(f"{d}/.hidden-note", 500, 24)
    paths = [d]
elif kind == "many":
    d = f"Many {tag}"
    for i in range(200): w(f"{d}/file {i:03}.dat", 1000 + (i * 997) % 60000, 100 + i)
    paths = [d]
print(json.dumps([os.path.join(root, p) for p in paths]))
'''

HASH = r'''
import os, sys, hashlib, json
out = {}
for p in json.loads(sys.argv[1]):
    p = os.path.expanduser(p)
    if os.path.isdir(p):
        for dp, dns, fns in os.walk(p):
            for fn in fns:
                if fn == ".DS_Store": continue
                fp = os.path.join(dp, fn)
                out[os.path.relpath(fp, os.path.dirname(p))] = hashlib.sha256(open(fp, "rb").read()).hexdigest()
            for dn in dns:
                full = os.path.join(dp, dn)
                if not os.listdir(full): out[os.path.relpath(full, os.path.dirname(p)) + "/"] = "dir"
    elif os.path.isfile(p):
        out[os.path.basename(p)] = hashlib.sha256(open(p, "rb").read()).hexdigest()
    else:
        out[os.path.basename(p)] = "MISSING"
print(json.dumps(out))
'''

def py(m, script, *args, timeout=900):
    host = MACH[m]["ssh"]
    q = " ".join(shlex.quote(a) for a in args)
    if host is None:
        return subprocess.run(["python3", "-c", script, *args], capture_output=True, text=True, timeout=timeout, check=True).stdout
    remote = shlex.quote(f"python3 - {q}")
    return subprocess.run(f"ssh {host} {remote}", shell=True, input=script, capture_output=True, text=True, timeout=timeout, check=True).stdout

def download_dir(m):
    s = json.loads(sh(m, f'cat "{CFG(m)}/settings.json"'))
    d = (s.get("downloadDir") or "").strip()
    return d or sh(m, 'echo $HOME').strip() + "/Downloads"

def friend_id(m, other_eid):
    fr = json.loads(sh(m, f'cat "{CFG(m)}/friends.json"'))
    return next((f for f in fr if f.get("endpointId") == other_eid), None)

EIDS = {
    "m1":   "52923ac9901b9505727f4355b0b9b25afb0d32c189d76538671005e3c60a4e2e",
    "mac2": "5d0f9908705cdc722d0ae488739602a2740c2f57e696fd795b6776c90354951e",
    "lin":  "7dcbe8927d1ff3b3c5bd1740c70935ab96eae31e92ae7eaa03dbb0effd373b3a",
}
def my_eid(m):
    return EIDS[m]

def queue(m, cmds):
    data = json.dumps(cmds)
    host = MACH[m]["ssh"]
    tmp = f'{CFG(m)}/automation-queue.json.tmp'
    if host is None:
        p = os.path.expanduser(MACH[m]["cfg"])
        open(p + "/automation-queue.json.tmp", "w").write(data)
        os.rename(p + "/automation-queue.json.tmp", p + "/automation-queue.json")
    else:
        remote = f'cat > "{tmp}" && mv "{tmp}" "{CFG(m)}/automation-queue.json"'
        subprocess.run(f"ssh {host} {shlex.quote(remote)}", shell=True, input=data, text=True, check=True)

def results(m):
    try:
        txt = sh(m, f'cat "{CFG(m)}/automation-results.jsonl" 2>/dev/null || true')
    except RuntimeError:
        return []
    return [json.loads(l) for l in txt.splitlines() if l.strip()]

def wait_for(m, pred, timeout):
    t0 = time.time()
    while time.time() - t0 < timeout:
        for r in results(m):
            if pred(r): return r
        time.sleep(3)
    return None

def run_case(src, dst, kind, op, log):
    tag = uuid.uuid4().hex[:6]
    root = f"~/dbtest-src/{tag}"
    paths = json.loads(py(src, GEN, root, kind, tag))
    want = json.loads(py(src, HASH, json.dumps(paths)))
    size = sum(1 for _ in want)
    dst_eid = my_eid(dst)
    t0 = time.time()
    if op == "send":
        queue(src, [{"op": "send", "to": dst_eid, "paths": paths}])
        started = wait_for(src, lambda r: r.get("cmd", {}).get("paths") == paths, 60)
        if not started or started.get("event") != "started":
            return log(src, dst, kind, op, "FAIL", 0, f"not started: {started}")
        tid = started["id"]
        done = wait_for(src, lambda r: r.get("event") == "done" and r.get("id") == tid, 3600)
    else:  # quicksend → receive
        queue(src, [{"op": "quicksend", "paths": paths}])
        started = wait_for(src, lambda r: r.get("cmd", {}).get("paths") == paths, 60)
        if not started or not started.get("code"):
            return log(src, dst, kind, op, "FAIL", 0, f"no code: {started}")
        queue(dst, [{"op": "receive", "code": started["code"]}])
        rs = wait_for(dst, lambda r: r.get("cmd", {}).get("code") == started["code"], 60)
        if not rs or rs.get("event") != "started":
            return log(src, dst, kind, op, "FAIL", 0, f"receive not started: {rs}")
        done = wait_for(dst, lambda r: r.get("event") == "done" and r.get("id") == rs["id"], 3600)
    secs = time.time() - t0
    if not done:
        return log(src, dst, kind, op, "FAIL", secs, "no terminal state within timeout")
    if done.get("state") != "completed":
        return log(src, dst, kind, op, "FAIL", secs, f"{done.get('state')}: {done.get('error')}")
    ddir = download_dir(dst)
    landed = [os.path.join(ddir, os.path.basename(os.path.expanduser(p))) for p in paths]
    got = json.loads(py(dst, HASH, json.dumps(landed)))
    # Friend sends and Quick Sends skip dotfiles by design (receiver sanitize_rel);
    # report that as a note, not a failure.
    skipped_dot = [k for k in want if any(part.startswith('.') for part in k.split('/')) and k not in got]
    want = {k: v for k, v in want.items() if k not in skipped_dot}
    bad = {k: (v, got.get(k)) for k, v in want.items() if got.get(k) != v}
    extra = [k for k in got if k not in want]
    nbytes = done.get("bytes") or 0
    mbps = (nbytes / 1e6) / max(done.get("ms", secs * 1000) / 1000, 0.001)
    verdict = "PASS" if not bad and not extra else "FAIL"
    detail = f"{nbytes/1e6:.1f}MB {mbps:.2f}MB/s {done.get('locality')}" + (f" bad={list(bad)[:3]} extra={extra[:3]}" if verdict == "FAIL" else "") + (f" (dotfiles skipped by design: {len(skipped_dot)})" if skipped_dot else "")
    # Clean up: source fixture + landed copy.
    try:
        sh(src, f"rm -rf {root}")
        sh(dst, " ; ".join(f"rm -rf {shlex.quote(p)}" for p in landed))
    except Exception:
        pass
    return log(src, dst, kind, op, verdict, secs, detail)

def main():
    pairs = [p.split(">") for p in sys.argv[1].split(",")]
    kinds = sys.argv[2].split(",") if len(sys.argv) > 2 else ["photo", "multi", "folder", "many", "video", "big"]
    ops = sys.argv[3].split(",") if len(sys.argv) > 3 else ["send", "quicksend"]
    out = open(os.path.expanduser("~/DropBeam-wt/nightly/realapp.log"), "a")
    def log(src, dst, kind, op, verdict, secs, detail):
        line = f"{time.strftime('%H:%M:%S')} {src:>4}->{dst:<4} {op:9} {kind:7} {verdict:4} {secs:7.1f}s {detail}"
        print(line, flush=True); out.write(line + "\n"); out.flush()
        return verdict
    for src, dst in pairs:
        for op in ops:
            for kind in kinds:
                try:
                    run_case(src, dst, kind, op, log)
                except Exception as e:
                    log(src, dst, kind, op, "ERR", 0, str(e)[:300])

if __name__ == "__main__":
    main()
