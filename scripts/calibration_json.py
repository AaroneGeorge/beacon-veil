#!/usr/bin/env python3
"""Merge one scripts/calibrate.sh run into evidence/calibration.json under the key "pipeline_smoke".

Usage: python3 -I scripts/calibration_json.py evidence/calibration/run-<stamp>

Reads <prefix>.meta, <prefix>.time (GNU time -v), <prefix>.log (RISC0_INFO session lines) and the matching fund-<stamp>.time.
All figures are MEASURED on this machine; us_per_cycle divides prove wall time by total cycles of all proven sessions,
which includes deployment block waits (labelled), so it is an upper bound and not the K4 number (W6 owns K4).
"""
import json, os, re, sys

prefix = sys.argv[1]
d = os.path.dirname(prefix)
stamp = os.path.basename(prefix).split("run-", 1)[1]
meta = dict(l.split("=", 1) for l in open(prefix + ".meta").read().splitlines() if "=" in l)

def gtime(path):
    out = {}
    if not os.path.exists(path):
        return out
    for l in open(path):
        l = l.strip()
        if l.startswith("Elapsed (wall clock) time"):
            v = l.rsplit(": ", 1)[1]
            parts = [float(x) for x in v.split(":")]
            s = 0.0
            for p in parts:
                s = s * 60 + p
            out["wall_s"] = round(s, 2)
        elif l.startswith("Maximum resident set size"):
            out["peak_rss_kib"] = int(l.rsplit(": ", 1)[1])
        elif l.startswith("User time"):
            out["user_s"] = float(l.rsplit(": ", 1)[1])
        elif l.startswith("System time"):
            out["sys_s"] = float(l.rsplit(": ", 1)[1])
        elif l.startswith("Exit status"):
            out["exit"] = int(l.rsplit(": ", 1)[1])
    return out

log = re.sub(r"\x1b\[[0-9;]*m", "", open(prefix + ".log", errors="replace").read())
totals = [int(x) for x in re.findall(r"(\d+) total cycles", log)]
users = [int(x) for x in re.findall(r"(\d+) user cycles", log)]
run = gtime(prefix + ".time")
fund = gtime(os.path.join(d, f"fund-{stamp}.time"))
entry = {
    "kind": "pipeline_smoke",
    "label": "measured",
    "what": "LEZ v0.4.0-rc3 examples/program_deployment run_hello_world_private on seq-real (RISC0_DEV_MODE unset): "
            "deploy hello_world (public txs), then one privacy-preserving tx proven locally through r0vm 3.0.5 (succinct)",
    "not_k4": "Not the K4 calibration: hello-world segments are small; W6 measures po2=20 segment cost with cycle_bench",
    "date_utc": meta.get("date_utc"),
    "machine": meta.get("machine"),
    "r0vm": meta.get("r0vm"),
    "command": meta.get("cmd"),
    "exit": int(meta.get("exit", "-1")),
    "run": run,
    "fund_private_account_tx": fund,
    "sessions_total_cycles": totals,
    "sessions_user_cycles": users,
    "sum_total_cycles": sum(totals),
    "us_per_total_cycle_upper_bound": round(run["wall_s"] * 1e6 / sum(totals), 2) if totals and "wall_s" in run else None,
    "note": "run.wall_s includes about 5 deployment transactions waiting for 15 s blocks; the three 65,536-cycle "
            "sessions are the hello program (two executor runs for simulation, then its proof)",
    "raw": {k: os.path.relpath(prefix + k, os.getcwd()) for k in (".meta", ".time", ".log")},
}
# Circuit proof time: from the circuit session's executor summary (last "total cycles" line, printed before proving
# starts) to the end of the run (date_utc + wall_s). The runner prints nothing after submitting.
import datetime as _dt
ts = re.findall(r"(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+)Z +INFO +risc0_zkvm::host::server::session: (\d+) total cycles", log)
segs = [int(x) for x in re.findall(r"number of segments: (\d+)", log)]
if ts and "wall_s" in run and meta.get("date_utc"):
    start = _dt.datetime.fromisoformat(meta["date_utc"].replace("Z", "+00:00"))
    end = start + _dt.timedelta(seconds=run["wall_s"])
    last = _dt.datetime.fromisoformat(ts[-1][0][:26] + "+00:00")
    circ_s = round((end - last).total_seconds(), 1)
    entry_extra = {
        "circuit_session": {"segments": segs[-1] if segs else None, "total_cycles": totals[-1], "user_cycles": users[-1]},
        "circuit_prove_s": circ_s,
        "circuit_us_per_total_cycle": round(circ_s * 1e6 / totals[-1], 1),
        "circuit_s_per_segment": round(circ_s / segs[-1], 1) if segs else None,
        "circuit_prove_note": "succinct receipt incl. lift/join and the inner hello receipt composition; no Groth16 wrap",
    }
else:
    entry_extra = {}
loadf = os.path.join(d, f"load-{stamp}.log")
if os.path.exists(loadf):
    xs = sorted(float(l.split()[1]) for l in open(loadf) if not l.startswith("#") and l.strip())
    if xs:
        entry_extra["load1_during_proof"] = {"min": xs[0], "median": xs[len(xs) // 2], "max": xs[-1], "samples": len(xs),
                                             "raw": os.path.relpath(loadf, os.getcwd()),
                                             "note": "other sprint tracks were building; sampler started during the run (first sample in the raw file)"}
entry.update(entry_extra)
out = "evidence/calibration.json"
data = json.load(open(out)) if os.path.exists(out) else {}
data["pipeline_smoke"] = entry
tmp = out + ".tmp"
json.dump(data, open(tmp, "w"), indent=2)
os.replace(tmp, out)
print(json.dumps(entry, indent=2))
