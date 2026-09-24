#!/bin/bash
# usage: run.sh <label> <addrfile> <mode> <suite> [--only case]
L=~/DropBeam-ios/src-tauri/target/release/dropbeam-lab
label=$1; addr=$(cat $2); mode=$3; suite=$4; shift 4
out=~/DropBeam-wt/nightly/$label.jsonl
$L send --to $addr --mode $mode --suite $suite "$@" > $out 2>&1
python3 - "$out" "$label" <<'PY'
import json,sys
for l in open(sys.argv[1]):
    try: d=json.loads(l)
    except: print('   ', l.strip()[:200]); continue
    if d.get('event')=='sent':
        ps,pe=d.get('pathStart') or {},d.get('pathEnd') or {}
        print(f"{sys.argv[2]:18} {d['case']:22} {d['verdict']:5} {d['bytes']/1e6:9.2f}MB {d['mbps']:7.2f}MB/s {d['ms']/1000:7.1f}s par={d['parallelEngaged']!s:5} {ps.get('path')}/{ps.get('rttMs')}ms -> {pe.get('path')}/{pe.get('rttMs')}ms {' '.join(map(str,d.get('problems') or []))[:120]}")
    elif d.get('event') in ('done','error'): print(f"{sys.argv[2]:18} {d}")
PY
