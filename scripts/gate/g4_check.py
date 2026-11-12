#!/usr/bin/env python3
"""G4 check (PLAN.md section 1): the follower followed a public testnet for 24 hours without manual help.

    python3 scripts/gate/g4_check.py evidence/g4/follower.jsonl [--min-hours 24] [--cadence S] [--allow-mock]
                                     [--summary-out FILE]

PASS needs all of:
  1. every line is JSON with the required fields ts, pid, event, slot, period, tx, beacon_source, uptime_s;
  2. one PID, exactly one `start` line, no `resume` line (no restart, no manual help);
  3. last ts minus first ts >= --min-hours (default 24);
  4. no gap between consecutive lines above 2x cadence (cadence from the `start` line, or --cadence);
  5. every `submit` line is followed by its outcome (same `n`): `applied`, or `noop` (refused before submission, so
     no transaction was sent); any `submit_failed`, or a submit with no outcome, fails;
  6. one beacon_source for the whole run, `logos_module` or `direct_http` (reported, so README and proposal state it);
  7. the chain is `host` (attest_host to a LEZ sequencer), not the test mock, unless --allow-mock;
  8. at least one update applied, applied head slots never decrease, and no stretch without an applied update
     longer than 3x cadence (the follower kept up, not only kept logging).
Prints a JSON summary; exit 0 on PASS, 1 on FAIL, 2 on usage errors.
"""

import argparse
import json
import sys
from datetime import datetime

REQUIRED = ("ts", "pid", "event", "slot", "period", "tx", "beacon_source", "uptime_s")
SOURCES = ("logos_module", "direct_http")


def parse_ts(s):
    return datetime.fromisoformat(s.replace("Z", "+00:00"))


def check(lines, min_hours, cadence_override, allow_mock):
    fails = []
    info = {}
    recs = []
    for i, raw in enumerate(lines, 1):
        raw = raw.strip()
        if not raw:
            continue
        try:
            r = json.loads(raw)
        except json.JSONDecodeError as e:
            fails.append(f"line {i}: not JSON ({e})")
            continue
        missing = [k for k in REQUIRED if k not in r]
        if missing:
            fails.append(f"line {i}: missing fields {missing}")
            continue
        try:
            r["_t"] = parse_ts(r["ts"])
        except ValueError:
            fails.append(f"line {i}: bad ts {r['ts']!r}")
            continue
        r["_line"] = i
        recs.append(r)
    if not recs:
        return False, ["no log lines"], info

    # 2. one process, no restart.
    pids = sorted({r["pid"] for r in recs})
    info["pids"] = pids
    if len(pids) != 1:
        fails.append(f"{len(pids)} PIDs: {pids}")
    starts = [r for r in recs if r["event"] == "start"]
    resumes = [r for r in recs if r["event"] == "resume"]
    info["start_lines"] = len(starts)
    if len(starts) != 1:
        fails.append(f"{len(starts)} start lines (want exactly 1)")
    if resumes:
        fails.append(f"{len(resumes)} resume lines: the run was restarted")
    start_extra = (starts[0].get("extra") or {}) if starts else {}
    info["start"] = start_extra

    # 3. duration.
    first, last = recs[0]["_t"], recs[-1]["_t"]
    hours = (last - first).total_seconds() / 3600
    info["first_ts"], info["last_ts"], info["hours"] = recs[0]["ts"], recs[-1]["ts"], round(hours, 3)
    if hours < min_hours:
        fails.append(f"duration {hours:.2f} h < {min_hours} h")

    # 4. gaps.
    cadence = cadence_override or start_extra.get("cadence_s")
    if not cadence:
        fails.append("no cadence: neither --cadence nor cadence_s in the start line")
        cadence = 0
    info["cadence_s"] = cadence
    max_gap, at = 0.0, None
    for a, b in zip(recs, recs[1:]):
        g = (b["_t"] - a["_t"]).total_seconds()
        if g < 0:
            fails.append(f"line {b['_line']}: time goes backwards")
        if g > max_gap:
            max_gap, at = g, b["_line"]
    info["max_gap_s"] = round(max_gap, 1)
    info["max_gap_before_line"] = at
    if cadence and max_gap > 2 * cadence:
        fails.append(f"gap of {max_gap:.0f} s before line {at} > 2 x cadence ({2 * cadence} s)")

    # 5. submissions and outcomes.
    outcomes = {}
    for r in recs:
        if r["event"] in ("applied", "noop", "submit_failed") and r.get("n") is not None:
            outcomes.setdefault(r["n"], r)
    submits = [r for r in recs if r["event"] == "submit"]
    applied = [r for r in recs if r["event"] == "applied"]
    noops, failed, unresolved = [], [], []
    for s in submits:
        o = outcomes.get(s.get("n"))
        if o is None:
            unresolved.append(s["_line"])
        elif o["event"] == "noop":
            noops.append(s["_line"])
        elif o["event"] == "submit_failed":
            failed.append((s["_line"], o.get("code"), (o.get("detail") or "")[:200]))
    info["submits"], info["applied"], info["noops"] = len(submits), len(applied), len(noops)
    info["handoffs"] = sum(1 for r in applied if (r.get("extra") or {}).get("handoff"))
    info["period_updates"] = sum(1 for r in applied if r.get("kind") == "period")
    if failed:
        fails.append(f"{len(failed)} submitted updates not applied: {failed[:5]}")
    if unresolved:
        fails.append(f"{len(unresolved)} submits without an outcome line (lines {unresolved[:5]})")
    if not applied:
        fails.append("no update applied")
    if any(not r.get("tx") for r in applied):
        fails.append("an applied line has no tx")

    # 8. head monotone.
    slots = [r["slot"] for r in applied if r.get("slot") is not None]
    if any(b < a for a, b in zip(slots, slots[1:])):
        fails.append("applied head slot decreased")
    if slots:
        info["head_first"], info["head_last"] = slots[0], slots[-1]
    if applied and cadence:
        marks = [recs[0]["_t"]] + [r["_t"] for r in applied] + [recs[-1]["_t"]]
        gaps = [(b - a).total_seconds() for a, b in zip(marks, marks[1:])]
        info["max_applied_gap_s"] = round(max(gaps), 1)
        if max(gaps) > 3 * cadence:
            fails.append(f"{max(gaps):.0f} s without an applied update > 3 x cadence ({3 * cadence} s)")

    # 6. source.
    srcs = sorted({str(r["beacon_source"]) for r in recs})
    info["beacon_source"] = srcs[0] if len(srcs) == 1 else srcs
    if len(srcs) != 1 or srcs[0] not in SOURCES:
        fails.append(f"beacon_source must be one of {SOURCES} for the whole run, got {srcs}")

    # 7. chain.
    chain = start_extra.get("chain")
    info["chain"] = chain
    info["risc0_dev_mode"] = start_extra.get("risc0_dev_mode")
    if chain != "host" and not allow_mock:
        fails.append(f"chain is {chain!r}, not 'host' (a mock run is not G4 evidence)")

    errs = {}
    for r in recs:
        if r["event"].endswith("_error") or r["event"] in ("no_instance",):
            errs[r["event"]] = errs.get(r["event"], 0) + 1
    info["error_lines"] = errs
    info["lines"] = len(recs)
    return not fails, fails, info


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("log")
    ap.add_argument("--min-hours", type=float, default=24.0)
    ap.add_argument("--cadence", type=float, default=None)
    ap.add_argument("--allow-mock", action="store_true")
    ap.add_argument("--summary-out")
    a = ap.parse_args()
    try:
        with open(a.log, encoding="utf-8") as f:
            lines = f.readlines()
    except OSError as e:
        print(f"g4_check: {e}", file=sys.stderr)
        return 2
    ok, fails, info = check(lines, a.min_hours, a.cadence, a.allow_mock)
    out = {"gate": "G4", "result": "PASS" if ok else "FAIL", "log": a.log, "failures": fails, **info}
    s = json.dumps(out, indent=2, default=str)
    print(s)
    if a.summary_out:
        with open(a.summary_out, "w", encoding="utf-8") as f:
            f.write(s + "\n")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
