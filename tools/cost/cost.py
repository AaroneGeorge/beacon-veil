#!/usr/bin/env python3
"""Cost table, proof-job records and the K4 projection (W6, PLAN.md section 4). Stdlib only; run with `python3 -I`.

    cost.py build   --raw DIR --out evidence/cost      # cost.json + cost.md from the raw executor outputs
    cost.py prove-record --dir JOBDIR --name N --exit RC --dev-mode BOOL [--receipt F] [--receipt-regex RE] [--cycles N]
    cost.py prove-summary --prove-dir evidence/prove --out evidence/cost   # prove.json + prove.md from job records
    cost.py project --calib JOBDIR [--calib JOBDIR...] [--ppe JOBDIR] --exec evidence/cost/cost.json --out DIR

Labels: every figure is `measured` (read from a raw file written by a command on this machine), `derived` (arithmetic
on measured figures and protocol constants; the formula is printed next to it) or `estimate` (an assumption is
involved; the assumption is printed). Protocol constants are LEZ v0.4.0-rc3 source, cited by file.
"""

import argparse
import datetime as dt
import json
import math
import os
import re
import statistics
import subprocess
import sys

# ---- LEZ v0.4.0-rc3 constants (lez/programs/fee/core/src/market.rs; marked "FIXME: Provisional" upstream) ----
LEZ = {
    "MAX_GAS_EXEC": 10_000_000,       # market.rs: 2 * TARGET_GAS_EXEC; per transaction and per block (action phase)
    "TARGET_GAS_EXEC": 5_000_000,
    "MAX_GAS_STOR": 1_000_000,        # market.rs: 2 * TARGET_GAS_STOR, per block
    "TARGET_GAS_STOR": 500_000,
    "PRIVATE_VERIFY_GAS": 409_764,    # market.rs: execution gas of every private transaction (receipt verification)
    "PROOF_BYTES": 223_551,           # market.rs
    "PRIVATE_PAD_BYTES": 512,         # market.rs
    "PRIVATE_GAS_STOR": 224_063,      # market.rs: PROOF_BYTES + PRIVATE_PAD_BYTES
    "MAX_PRIVATE_ACTIONS": 8,
    "BLOCK_S_STANDALONE": 15,         # lez/sequencer/service/networks/local.json block_create_timeout
    "BLOCK_S_TESTNET": 10,            # networks/testnet.json
    "BLOCK_S_DEVNET": 30,             # networks/devnet.json
}
LEZ_SRC = ("LEZ v0.4.0-rc3 lez/programs/fee/core/src/market.rs (constants marked FIXME: Provisional), "
           "lez/programs/fee/core/src/assess.rs (private fee = (PRIVATE_VERIFY_GAS + effect cycles) exec + "
           "PRIVATE_GAS_STOR stor; public = cycles exec + wire bytes stor), lez/sequencer/service/networks/*.json")
SECONDS_PER_SLOT = 12
SLOTS_PER_EPOCH = 32
EPOCHS_PER_PERIOD = 256
DAY = 86_400
CADENCES = {  # updates per day
    "every finality update (1 per epoch, 6.4 min)": DAY / (SECONDS_PER_SLOT * SLOTS_PER_EPOCH),
    "every 16 epochs (1.7 h, the G4 run cadence)": DAY / (SECONDS_PER_SLOT * SLOTS_PER_EPOCH * 16),
    "one per sync-committee period (27.3 h, keep-alive minimum)": DAY / (SECONDS_PER_SLOT * SLOTS_PER_EPOCH * EPOCHS_PER_PERIOD),
}


def sh(cmd):
    try:
        return subprocess.run(cmd, shell=True, capture_output=True, text=True, timeout=20).stdout.strip()
    except Exception:  # noqa: BLE001
        return ""


def machine_line():
    model = sh("lscpu | sed -n 's/^Model name: *//p'")
    ram = sh("free -g | awk '/Mem:/{print $2}'")
    return f"{model}, {os.cpu_count()} threads, {ram} GiB RAM, kernel {sh('uname -r')}, {sh('r0vm --version') or 'r0vm ?'}"


def load(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def dump(obj, path):
    with open(path, "w", encoding="utf-8") as f:
        json.dump(obj, f, indent=2, sort_keys=False)
        f.write("\n")


def fmt(n):
    if n is None:
        return "n/a"
    if isinstance(n, float):
        return f"{n:,.2f}"
    return f"{n:,}"


# ------------------------------------------------------------------------------------------------------------------
# build: cost.json + cost.md
# ------------------------------------------------------------------------------------------------------------------

def rows_from_exec_lc(doc, raw_name):
    out = []
    for r in doc["rows"]:
        public = r["program"] == "attest"
        out.append({
            "id": r["id"], "program": r["program"], "message": r["message"], "step": r["step"],
            "user_cycles": r["user_cycles"], "total_cycles_padded": r["total_cycles_padded"], "segments": r["segments"],
            "message_bytes": r.get("message_bytes"),
            # Public handlers are metered: execution gas = user cycles (LEZ lee/state_machine/src/program/mod.rs).
            "gas_exec": r["user_cycles"] if public else None,
            "label": "measured", "raw": raw_name,
            "extra": {k: v for k, v in r.items() if k not in (
                "id", "program", "message", "step", "user_cycles", "total_cycles_padded", "segments", "message_bytes",
                "executor_ms")},
        })
    return out


def rows_from_attest_exec(doc, raw_name):
    out = []
    for r in doc["rows"]:
        if not r.get("accepted", True):
            continue
        out.append({
            "id": f"{r['program']}.{r['case']}", "program": r["program"], "message": r["message"], "step": r["case"],
            "user_cycles": r["user_cycles"], "total_cycles_padded": r["total_cycles"], "segments": r["segments"],
            "message_bytes": r.get("message_bytes"), "gas_exec": r["user_cycles"], "label": "measured",
            "raw": raw_name, "extra": {"claim_bytes": r.get("claim_bytes")} if r.get("claim_bytes") else {},
        })
    return out


def rows_generic(doc, raw_name):
    """Rows from any other harness that writes {"rows": [{id, program, message, step, user_cycles, ...}]}."""
    out = []
    for r in doc.get("rows", []):
        if "user_cycles" not in r or "id" not in r:
            continue
        out.append({
            "id": r["id"], "program": r.get("program", "?"), "message": r.get("message", "?"),
            "step": r.get("step", ""), "user_cycles": r["user_cycles"],
            "total_cycles_padded": r.get("total_cycles_padded", r.get("total_cycles")),
            "segments": r.get("segments"), "message_bytes": r.get("message_bytes"),
            "gas_exec": r.get("gas_exec"), "label": r.get("label", "measured"), "raw": raw_name, "extra": {},
        })
    return out


def find(rows, prefix):
    return [r for r in rows if r["id"].startswith(prefix)]


def derive(rows, sizes, b0_phases=None):
    """Derived tables. Each entry: value, label, formula/assumption."""
    d = {"constants": {"values": LEZ, "source": LEZ_SRC}}
    verify = {r["step"]: r for r in find(rows, "lc_verify.v1.verify.")}
    apply_ = {r["step"]: r for r in find(rows, "attest.apply.")}
    wrec = {r["step"]: r for r in find(rows, "attest.write_record.") if r["step"] != "bootstrap_15605760"}

    # 1. Gas per update on Path A against the cap.
    path_a = []
    for step, v in verify.items():
        a = apply_.get(step)
        if not a:
            continue
        w = wrec.get(step)
        effects = a["user_cycles"] + (w["user_cycles"] if w else 0)
        gas = LEZ["PRIVATE_VERIFY_GAS"] + effects
        path_a.append({
            "step": step, "handoff": v["extra"].get("handoff"), "verify_cycles_private": v["user_cycles"],
            "apply_cycles": a["user_cycles"], "write_record_cycles": w["user_cycles"] if w else 0,
            "gas_exec": gas, "gas_exec_pct_of_cap": round(100 * gas / LEZ["MAX_GAS_EXEC"], 2),
            "gas_stor": LEZ["PRIVATE_GAS_STOR"], "gas_stor_pct_of_block": round(100 * LEZ["PRIVATE_GAS_STOR"] / LEZ["MAX_GAS_STOR"], 2),
            "label": "derived",
        })
    d["path_a_gas_per_update"] = {
        "rows": path_a,
        "formula": "gas_exec = PRIVATE_VERIFY_GAS + cycles(Apply) + cycles(WriteRecord); gas_stor = PRIVATE_GAS_STOR "
                   "(assess.rs private_fee_required). The private Verify cycles are paid in proving time, not gas.",
        "label": "derived (measured cycles + LEZ constants)",
    }

    # 2. B0: the same Verify on the public path.
    b0 = []
    for step, v in verify.items():
        b0.append({"step": step, "cycles": v["user_cycles"], "x_cap": round(v["user_cycles"] / LEZ["MAX_GAS_EXEC"], 2),
                   "min_public_txs_if_split_evenly": math.ceil(v["user_cycles"] / LEZ["MAX_GAS_EXEC"]),
                   "fits_one_tx": v["user_cycles"] <= LEZ["MAX_GAS_EXEC"]})
    d["b0_public_verify_vs_cap"] = {
        "rows": b0, "label": "measured cycles; split count is a lower bound (estimate)",
        "note": "Same guest and input as the private leg, so the executor's cycle count is what the sequencer would meter; "
                "it is above MAX_GAS_EXEC, so the sequencer refuses it as one transaction (OutOfGas). The split count "
                "assumes perfectly even phases and no split overhead.",
    }

    # 3. Chain capacity per day.
    cap = []
    if path_a:
        worst_a = max(path_a, key=lambda r: r["gas_exec"])
        worst_v = max(verify.values(), key=lambda r: r["user_cycles"])
        for block_name, bs in (("standalone 15 s", LEZ["BLOCK_S_STANDALONE"]), ("testnet 10 s", LEZ["BLOCK_S_TESTNET"])):
            blocks = DAY / bs
            per_block_a = min(LEZ["MAX_GAS_STOR"] // LEZ["PRIVATE_GAS_STOR"], LEZ["MAX_GAS_EXEC"] // worst_a["gas_exec"])
            for cad, n in CADENCES.items():
                cap.append({
                    "block_time": block_name, "cadence": cad, "updates_per_day": round(n, 2),
                    "path_a_exec_gas_per_day": round(n * worst_a["gas_exec"]),
                    "path_a_pct_daily_exec_cap": round(100 * n * worst_a["gas_exec"] / (blocks * LEZ["MAX_GAS_EXEC"]), 4),
                    "path_a_pct_daily_stor_cap": round(100 * n * LEZ["PRIVATE_GAS_STOR"] / (blocks * LEZ["MAX_GAS_STOR"]), 4),
                    "path_a_private_slots_per_day": round(n, 2),
                    "path_b_exec_gas_per_day": round(n * worst_v["user_cycles"]),
                    "path_b_pct_daily_exec_cap": round(100 * n * worst_v["user_cycles"] / (blocks * LEZ["MAX_GAS_EXEC"]), 4),
                    "path_b_pct_daily_exec_target": round(100 * n * worst_v["user_cycles"] / (blocks * LEZ["TARGET_GAS_EXEC"]), 4),
                })
            cap.append({"block_time": block_name, "cadence": "chain maximum",
                        "path_a_updates_per_block_max": per_block_a,
                        "path_a_updates_per_day_max": round(blocks * per_block_a),
                        "path_b_blocks_per_update_min": math.ceil(worst_v["user_cycles"] / LEZ["MAX_GAS_EXEC"]),
                        "path_b_updates_per_day_max": round(blocks / math.ceil(worst_v["user_cycles"] / LEZ["MAX_GAS_EXEC"]))})
    d["chain_capacity_per_day"] = {
        "rows": cap, "label": "derived (worst measured update; Path B as if splittable with no overhead: estimate)",
        "formula": "blocks/day = 86400 / block time; Path A: private tx = PRIVATE_GAS_STOR storage gas against "
                   "MAX_GAS_STOR per block (floor(1e6/224063) = 4 per block) and PRIVATE_VERIFY_GAS + effect cycles "
                   "execution gas; Path B: Verify cycles of execution gas per update.",
    }

    # 4. Amortised cost per attestation (public placement: one public tx carrying Attest + CheckHeader + consumer).
    att = []
    check = find(rows, "attest.check_header")
    check_c = check[0]["user_cycles"] if check else None
    cons = {r["message"]: r for r in rows if r["program"] == "consumer_erc20"}
    worst_upd = max((r["gas_exec"] for r in path_a), default=None)
    for r in rows:
        if r["program"] != "attest" or r["message"] != "Attest":
            continue
        consumer = 0
        if "log" in r["step"] and cons:
            consumer = sum(c["user_cycles"] for c in cons.values())
        gas = r["user_cycles"] + (check_c or 0) + consumer
        entry = {"case": r["step"], "attest_cycles": r["user_cycles"], "check_header_cycles": check_c,
                 "consumer_cycles": consumer or None, "public_gas_exec": gas, "label": "derived"}
        if worst_upd:
            for k in (1, 10, 100, 1000):
                entry[f"with_update_amortised_over_{k}"] = round(gas + worst_upd / k)
        att.append(entry)
    d["amortised_cost_per_attestation"] = {
        "rows": att, "label": "derived (measured cycles); amortisation is arithmetic",
        "formula": "public placement: cycles(Attest) + cycles(CheckHeader) + consumer cycles (Statement + MarkConsumed "
                   "for the ERC-20 consumer), plus Path A update gas / k attestations per header. Storage gas (wire "
                   "bytes of the public tx) is not included here. Private placement: PRIVATE_VERIFY_GAS + public "
                   "effects + PRIVATE_GAS_STOR, the Attest cycles move to the user's prover.",
    }

    # 5. State growth.
    rec_bytes = (sizes or {}).get("header_record_bytes")
    inst_bytes = (sizes or {}).get("instance_state_bytes_mainnet")
    overhead = 72  # estimate: 32-byte account id key + 32-byte program key + 8-byte length; LEZ layout not measured
    growth = []
    if rec_bytes:
        for cad, n in CADENCES.items():
            for years in (1, 5):
                recs = n * 365 * years
                growth.append({"cadence": cad, "years": years, "records": round(recs),
                               "payload_bytes": round(recs * rec_bytes),
                               "with_account_overhead_bytes_est": round(recs * (rec_bytes + overhead))})
    d["state_growth"] = {
        "rows": growth, "header_record_bytes": rec_bytes, "instance_state_bytes": inst_bytes,
        "label": "derived (measured borsh sizes); per-account overhead is an estimate",
        "assumption": f"one write-once header record per update that moves the finalized slot; account overhead "
                      f"{overhead} bytes per account (estimate, LEZ storage layout not measured). Consumer nullifiers "
                      "add one account per accepted statement and are not included.",
    }

    # 6. Precompile delta (estimate).
    phases = b0_phases
    worst_v = max(verify.values(), key=lambda r: r["user_cycles"]) if verify else None
    d["precompile_delta"] = {
        "label": "estimate",
        "rows": [],
        "assumption": "A BLS12-381 precompile would replace the in-guest BLS work (key aggregation with non-signer "
                      "decompression, hash-to-curve, pairing) by a fixed charge P. Phase cycles come from W1's "
                      "cycle-trace build when its raw file is present (raw/b0-phases.json); P is unknown: shown at 0 "
                      "(lower bound) and 1,000,000 gas (illustrative).",
    }
    if phases:
        for st in phases.get("steps", []):
            v = verify.get(st.get("step"))
            if not v:
                continue
            bls = st["bls_sum"]
            for p in (0, 1_000_000):
                rest = v["user_cycles"] - bls + p
                d["precompile_delta"]["rows"].append({
                    "step": st["step"], "verify_cycles_now": v["user_cycles"], "bls_cycles_removed": bls,
                    "pairing_and_subgroup": st.get("g2_subgroup_check", 0) + st.get("miller_loop_2", 0)
                    + st.get("final_exponentiation", 0),
                    "precompile_charge": p, "update_gas_after": rest, "x_cap_after": round(rest / LEZ["MAX_GAS_EXEC"], 2),
                    "fits_one_tx": rest <= LEZ["MAX_GAS_EXEC"]})
    return d


def md_table(rows, cols):
    if not rows:
        return "_no rows yet_\n"
    head = "| " + " | ".join(c for c, _ in cols) + " |\n|" + "|".join("---" for _ in cols) + "|\n"
    body = ""
    for r in rows:
        body += "| " + " | ".join(fmt(r.get(k)) if not isinstance(r.get(k), str) else r.get(k) for _, k in cols) + " |\n"
    return head + body


def cmd_build(a):
    raw = a.raw
    rows, sources = [], []
    sizes = None
    meta = {}
    for name in sorted(os.listdir(raw)):
        p = os.path.join(raw, name)
        if not name.endswith(".json"):
            continue
        doc = load(p)
        rel = os.path.relpath(p, os.path.dirname(a.out.rstrip("/")) if a.out else ".")
        if name.startswith("exec-lc"):
            rows += rows_from_exec_lc(doc, name)
            meta["guests"] = doc.get("guests")
        elif name.startswith("attest-exec"):
            rows += rows_from_attest_exec(doc, name)
            meta["attest_exec_guests"] = {"attest_elf_bytes": doc.get("attest_elf_bytes"),
                                          "consumer_elf_bytes": doc.get("consumer_elf_bytes")}
        elif name.startswith("sizes"):
            sizes = doc
        elif name.startswith("b0-phases"):
            meta["b0_phases"] = doc
        elif name.startswith("run-meta"):
            meta["run"] = doc
            continue
        else:
            rows += rows_generic(doc, name)
        sources.append(rel)
    rows.sort(key=lambda r: r["id"])
    derived = derive(rows, sizes, meta.get("b0_phases"))
    if "b0_phases" in meta:
        derived["b0_phases"] = meta["b0_phases"]
    doc = {
        "what": "beacon-veil cost table (make cost-exec). Cycles are deterministic for the recorded guest image ids.",
        "generated_utc": dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "machine": machine_line(),
        "run": meta.get("run"),
        "guests": meta.get("guests"),
        "raw_files": sources,
        "sizes": sizes,
        "rows": rows,
        "derived": derived,
    }
    os.makedirs(a.out, exist_ok=True)
    dump(doc, os.path.join(a.out, "cost.json"))
    with open(os.path.join(a.out, "cost.md"), "w", encoding="utf-8") as f:
        f.write(render_md(doc))
    print(f"cost: {len(rows)} rows -> {a.out}/cost.json, cost.md")
    return 0


def render_md(doc):
    d = doc["derived"]
    run = doc.get("run") or {}
    out = ["# Cost table (generated by `make cost-exec`; do not edit by hand)\n",
           f"Generated {doc['generated_utc']}. Machine: {doc['machine']}.  ",
           f"Command: `{run.get('command', 'make cost-exec')}`. Guest build: {run.get('guest_build', '?')}. "
           f"Repo commit: {run.get('repo_commit', '?')}.  ",
           "Labels: **measured** = read from a raw file in `evidence/cost/raw/`; **derived** = arithmetic on measured "
           "figures and LEZ rc3 constants (formula given); **estimate** = an assumption is involved (stated).\n"]
    if doc.get("guests"):
        out.append("Guest images: " + "; ".join(
            f"`{k}` {v.get('image_id', '?')[:16]}... ({fmt(v.get('elf_bytes'))} ELF bytes)" for k, v in doc["guests"].items()) + "\n")
    out.append("## 1. Measured cycles (RISC Zero 3.0.5 executor, real mainnet fixtures)\n")
    out.append(md_table(doc["rows"], [("id", "id"), ("message", "message"), ("user cycles", "user_cycles"),
                                       ("padded", "total_cycles_padded"), ("segments", "segments"),
                                       ("gas (public)", "gas_exec"), ("msg bytes", "message_bytes"),
                                       ("label", "label"), ("raw", "raw")]))
    pa = d["path_a_gas_per_update"]
    out.append("\n## 2. Path A: gas per update against the cap (derived)\n")
    out.append(f"{pa['formula']}\n\n")
    out.append(md_table(pa["rows"], [("step", "step"), ("Verify cycles (proven)", "verify_cycles_private"),
                                     ("Apply", "apply_cycles"), ("WriteRecord", "write_record_cycles"),
                                     ("exec gas", "gas_exec"), ("% of 10M cap", "gas_exec_pct_of_cap"),
                                     ("stor gas", "gas_stor"), ("% of block stor", "gas_stor_pct_of_block")]))
    b0 = d["b0_public_verify_vs_cap"]
    out.append("\n## 3. B0: the same Verify on the public path\n")
    out.append(f"{b0['note']}\n\n")
    out.append(md_table(b0["rows"], [("step", "step"), ("cycles", "cycles"), ("x cap", "x_cap"),
                                     ("min txs (even split, est.)", "min_public_txs_if_split_evenly")]))
    if d.get("b0_phases"):
        ph = d["b0_phases"]
        out.append(f"\nPer-phase cycles, {ph['label']} (raw/{ph['source_log']}, trace image "
                   f"{(ph.get('trace_image_id') or '?')[:16]}...). {ph['note']}\n\n")
        cols = [("step", "step"), ("non-signers", "non_signers"), ("committee root (tree_hash)", "committee_root"),
                ("key aggregation", "key_aggregation"), ("sig decompress", "sig_decompress"),
                ("G2 subgroup check", "g2_subgroup_check"), ("hash_to_curve", "hash_to_curve"),
                ("Miller loop (2 pairs)", "miller_loop_2"), ("final exp.", "final_exponentiation"),
                ("BLS sum", "bls_sum"), ("Helios verify+apply", "helios_verify_and_apply")]
        out.append(md_table(ph["steps"], cols))
    cc = d["chain_capacity_per_day"]
    out.append("\n## 4. Chain capacity per day (derived; Path B splittable is an estimate)\n")
    out.append(f"{cc['formula']}\n\n")
    out.append(md_table([r for r in cc["rows"] if r["cadence"] != "chain maximum"],
                        [("block", "block_time"), ("cadence", "cadence"), ("updates/day", "updates_per_day"),
                         ("A exec gas/day", "path_a_exec_gas_per_day"), ("A % exec cap", "path_a_pct_daily_exec_cap"),
                         ("A % stor cap", "path_a_pct_daily_stor_cap"), ("B exec gas/day", "path_b_exec_gas_per_day"),
                         ("B % exec cap", "path_b_pct_daily_exec_cap"), ("B % exec target", "path_b_pct_daily_exec_target")]))
    out.append("\n")
    out.append(md_table([r for r in cc["rows"] if r["cadence"] == "chain maximum"],
                        [("block", "block_time"), ("A updates/block max", "path_a_updates_per_block_max"),
                         ("A updates/day max", "path_a_updates_per_day_max"),
                         ("B blocks/update min", "path_b_blocks_per_update_min"),
                         ("B updates/day max", "path_b_updates_per_day_max")]))
    am = d["amortised_cost_per_attestation"]
    out.append("\n## 5. Amortised cost per attestation (derived)\n")
    out.append(f"{am['formula']}\n\n")
    out.append(md_table(am["rows"], [("case", "case"), ("Attest", "attest_cycles"), ("CheckHeader", "check_header_cycles"),
                                     ("consumer", "consumer_cycles"), ("public gas", "public_gas_exec"),
                                     ("+update/1", "with_update_amortised_over_1"),
                                     ("+update/10", "with_update_amortised_over_10"),
                                     ("+update/100", "with_update_amortised_over_100"),
                                     ("+update/1000", "with_update_amortised_over_1000")]))
    sg = d["state_growth"]
    out.append("\n## 6. State growth (derived; account overhead estimated)\n")
    out.append(f"Header record {fmt(sg['header_record_bytes'])} bytes; instance state {fmt(sg['instance_state_bytes'])} "
               f"bytes. {sg['assumption']}\n\n")
    out.append(md_table(sg["rows"], [("cadence", "cadence"), ("years", "years"), ("records", "records"),
                                     ("payload bytes", "payload_bytes"),
                                     ("with overhead (est.)", "with_account_overhead_bytes_est")]))
    pd = d["precompile_delta"]
    out.append("\n## 7. Precompile delta (ESTIMATE)\n")
    out.append(f"{pd['assumption']}\n\n")
    out.append(md_table(pd["rows"], [("step", "step"), ("Verify cycles now", "verify_cycles_now"),
                                     ("BLS cycles removed", "bls_cycles_removed"),
                                     ("of which pairing + subgroup", "pairing_and_subgroup"),
                                     ("precompile charge P", "precompile_charge"),
                                     ("update gas after", "update_gas_after"), ("x cap after", "x_cap_after")])
               if pd["rows"] else "_Pending: W1's per-phase trace (raw/b0-phases.json) has not landed._\n")
    out.append(f"\nConstants: {LEZ_SRC}.\n")
    return "\n".join(out)


# ------------------------------------------------------------------------------------------------------------------
# prove-record / prove-summary
# ------------------------------------------------------------------------------------------------------------------

def rel(p):
    """Repo-relative path when inside $BV_REPO (so evidence survives a clone), else absolute."""
    repo = os.environ.get("BV_REPO")
    ap_ = os.path.abspath(p)
    if repo and ap_.startswith(os.path.abspath(repo) + os.sep):
        return os.path.relpath(ap_, repo)
    return ap_


def parse_time_v(path):
    out = {}
    try:
        txt = open(path, encoding="utf-8", errors="replace").read()
    except OSError:
        return out
    m = re.search(r"Elapsed \(wall clock\) time \(h:mm:ss or m:ss\): (\S+)", txt)
    if m:
        parts = [float(x) for x in m.group(1).split(":")]
        s = 0.0
        for p in parts:
            s = s * 60 + p
        out["wall_s"] = round(s, 2)
    for key, rx in (("max_rss_kib_time_v", r"Maximum resident set size \(kbytes\): (\d+)"),
                    ("user_s", r"User time \(seconds\): ([\d.]+)"), ("sys_s", r"System time \(seconds\): ([\d.]+)"),
                    ("cpu_pct", r"Percent of CPU this job got: (\d+)%"), ("major_faults", r"Major .*page faults: (\d+)"),
                    ("exit_status_time_v", r"Exit status: (\d+)")):
        m = re.search(rx, txt)
        if m:
            out[key] = float(m.group(1)) if "." in m.group(1) else int(m.group(1))
    return out


CYCLE_BENCH_PROVE = re.compile(
    r"prove\((?P<prog>[^/]+)/(?P<instr>[^/]+)/(?P<phase>[^)]+)\): (?P<ms>[\d.]+) ms \([\d.]+s\), "
    r"total_cycles=(?P<total>\d+), segments=(?P<seg>\d+)")


def cmd_prove_record(a):
    d = a.dir
    meta = {}
    for line in open(os.path.join(d, "meta.txt"), encoding="utf-8", errors="replace"):
        k, _, v = line.rstrip("\n").partition("=")
        meta[k] = v
    rec = {
        "name": a.name, "label": "measured", "dev_mode": a.dev_mode == "true", "exit": int(a.exit),
        "start_utc": meta.get("start_utc"), "end_utc": meta.get("end_utc"), "machine": meta.get("machine"),
        "command": meta.get("cmd"), "loadavg_start": meta.get("loadavg_start"), "loadavg_end": meta.get("loadavg_end"),
        "mem_start": meta.get("mem_start"), "risc0": meta.get("risc0_dev_mode"), "repo_commit": meta.get("repo_commit"),
        "note": meta.get("note"),
        "raw": {k: rel(os.path.join(d, k)) for k in ("meta.txt", "time.txt", "output.log", "samples.csv", "sampler.json")},
    }
    rec.update(parse_time_v(os.path.join(d, "time.txt")))
    try:
        rec["sampler"] = load(os.path.join(d, "sampler.json"))
        rec["peak_rss_gib"] = round(max(rec["sampler"]["peak_tree_rss_kib"], rec.get("max_rss_kib_time_v", 0)) / 1048576, 3)
    except (OSError, ValueError, KeyError):
        pass
    log = ""
    try:
        log = open(os.path.join(d, "output.log"), encoding="utf-8", errors="replace").read()
    except OSError:
        pass
    if a.receipt:
        try:
            rec["receipt_bytes"] = os.path.getsize(a.receipt)
            rec["receipt_file"] = a.receipt
        except OSError:
            rec["receipt_bytes"] = None
    elif a.receipt_regex:
        m = re.search(a.receipt_regex, log)
        rec["receipt_bytes"] = int(m.group(1)) if m else None
    if a.cycles:
        rec["cycles_padded"] = int(a.cycles)
        if rec.get("wall_s"):
            rec["s_per_mcycle"] = round(rec["wall_s"] / (int(a.cycles) / 1e6), 2)
    # cycle_bench rows (LEZ tools/cycle_bench --prove): per-case prove seconds.
    cb = [{"program": m["prog"], "instruction": m["instr"], "prove_s": round(float(m["ms"]) / 1000, 2),
           "total_cycles": int(m["total"]), "segments": int(m["seg"])} for m in CYCLE_BENCH_PROVE.finditer(log)]
    if cb:
        rec["cycle_bench_prove_rows"] = cb
    path = os.path.join(d, "record.json")
    dump(rec, path)
    print(path)
    return 0


def cmd_prove_summary(a):
    recs = []
    for root, _, files in os.walk(a.prove_dir):
        if "record.json" in files:
            recs.append(load(os.path.join(root, "record.json")))
    recs.sort(key=lambda r: r.get("start_utc") or "")
    os.makedirs(a.out, exist_ok=True)
    dump({"what": "proof-window job records (make cost-prove)", "jobs": recs}, os.path.join(a.out, "prove.json"))
    lines = ["# Proof-window jobs (measured; generated from evidence/prove/*/record.json)\n",
             "| job | start (UTC) | dev mode | exit | wall s | peak RSS GiB | receipt bytes | load start | machine |",
             "|---|---|---|---|---|---|---|---|---|"]
    for r in recs:
        lines.append(f"| {r['name']} | {r.get('start_utc')} | {r.get('dev_mode')} | {r.get('exit')} | {fmt(r.get('wall_s'))} | "
                     f"{fmt(r.get('peak_rss_gib'))} | {fmt(r.get('receipt_bytes'))} | {(r.get('loadavg_start') or '')[:14]} | "
                     f"{r.get('machine')} |")
    with open(os.path.join(a.out, "prove.md"), "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")
    print(f"prove-summary: {len(recs)} jobs -> {a.out}/prove.json")
    return 0


# ------------------------------------------------------------------------------------------------------------------
# project: K4 proving-time projection
# ------------------------------------------------------------------------------------------------------------------

def cmd_project(a):
    seg20, small, peaks, loads, srcs = [], [], [], [], []
    for d in a.calib:
        r = load(os.path.join(d, "record.json"))
        srcs.append(os.path.join(d, "record.json"))
        peaks.append(r.get("peak_rss_gib"))
        loads.append(r.get("sampler", {}).get("load1_mean"))
        for row in r.get("cycle_bench_prove_rows", []):
            (seg20 if row["total_cycles"] == 1 << 20 and row["segments"] == 1 else small).append(row)
    if not seg20:
        print("project: no po2=20 single-segment rows in the calibration records", file=sys.stderr)
        return 1
    t20 = [r["prove_s"] for r in seg20]
    t_seg = statistics.mean(t20)
    sd = statistics.stdev(t20) if len(t20) > 1 else 0.0
    ppe = None
    if a.ppe:
        ppe = load(os.path.join(a.ppe, "record.json"))
        srcs.append(os.path.join(a.ppe, "record.json"))
    ppe_rows = {}
    if a.ppe_json and os.path.exists(a.ppe_json):
        for r in load(a.ppe_json).get("ppe", []):
            ppe_rows[r.get("case") or r.get("label") or json.dumps(r)[:40]] = r
    wrap_s = None
    if ppe_rows:
        vals = [v.get("prove_ms", 0) / 1000 for k, v in ppe_rows.items() if "chain_caller depth=1" in k]
        wrap_s = vals[0] if vals else None

    cost = load(a.exec)
    verify = [r for r in cost["rows"] if r["id"].startswith("lc_verify.v1.verify.") or r["id"] == "attest.init"]
    for c in a.case or []:
        # label:padded_cycles:segments[:source], e.g. "V2 finality:39845888:38:.tmp/w1/v2-exec.json"
        parts = c.split(":")
        verify.append({"id": parts[0], "step": parts[0], "total_cycles_padded": int(parts[1]),
                       "segments": int(parts[2]), "source": parts[3] if len(parts) > 3 else None})
    # Recursion overhead per segment (lift, and join for all but one): ESTIMATE, as a fraction of one po2-20 segment.
    rec_lo, rec_hi = a.recursion_lo, a.recursion_hi
    t_rec_measured = None
    if a.rec_composite and a.rec_succinct and a.rec_segments:
        rc_ = load(os.path.join(a.rec_composite, "record.json"))
        rs_ = load(os.path.join(a.rec_succinct, "record.json"))
        srcs += [os.path.join(a.rec_composite, "record.json"), os.path.join(a.rec_succinct, "record.json")]
        t_rec_measured = (rs_["wall_s"] - rc_["wall_s"]) / (2 * a.rec_segments - 1)
    proj = []
    for v in verify:
        n = v["segments"]
        t_prog = v["total_cycles_padded"] / (1 << 20) * t_seg
        if t_rec_measured is not None:
            # measured per lift-or-join on small (po2 15) segments; lifting a po2-20 segment costs somewhat more, so
            # the upper end doubles it (estimate)
            t_rec_lo, t_rec_hi = (2 * n - 1) * t_rec_measured, (2 * n - 1) * 2 * t_rec_measured
        else:
            t_rec_lo = (2 * n - 1) * rec_lo * t_seg
            t_rec_hi = (2 * n - 1) * rec_hi * t_seg
        if a.wrap_s is not None:
            wrap_s = a.wrap_s
        wrap = wrap_s if wrap_s is not None else a.wrap_s_est
        wrap_hi = a.wrap_s_hi if a.wrap_s_hi else wrap
        lo, hi = t_prog + t_rec_lo + wrap, t_prog + t_rec_hi + wrap_hi
        proj.append({"step": v["id"] if v["id"] == "attest.init" else v["step"], "source": v.get("source", "cost.json"), "segments": n, "padded_cycles": v["total_cycles_padded"],
                     "segment_proving_h": round(t_prog / 3600, 2),
                     "recursion_h_range_est": [round(t_rec_lo / 3600, 2), round(t_rec_hi / 3600, 2)],
                     "circuit_wrap_s": [round(wrap, 1), round(wrap_hi, 1)],
                     "circuit_wrap_label": ("measured (--wrap-s or cycle_bench --ppe)" if wrap_s is not None else "estimate"),
                     "total_h_range": [round(lo / 3600, 2), round(hi / 3600, 2)],
                     "fits_8h_window": hi <= 8 * 3600})
    peak = max(p for p in peaks if p is not None) if any(p is not None for p in peaks) else None
    ppe_peak = ppe.get("peak_rss_gib") if ppe else None
    doc = {
        "what": "K4 projection: real-proof wall time and peak RSS for lc_verify V1 updates on this machine",
        "generated_utc": dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "machine": machine_line(),
        "inputs": {"calibration_records": srcs, "cost_json": a.exec},
        "measured": {
            "po2_20_segment_prove_s": t20, "mean_s": round(t_seg, 1), "stdev_s": round(sd, 1),
            "us_per_padded_cycle": round(t_seg / (1 << 20) * 1e6, 1),
            "small_rows": small, "peak_rss_gib_calibration": peak, "peak_rss_gib_ppe": ppe_peak,
            "load1_mean_during_runs": loads,
            "recursion_s_per_lift_or_join": round(t_rec_measured, 2) if t_rec_measured is not None else None,
        },
        "model": ("T = padded_cycles / 2^20 x t_seg20 (segment proofs, one at a time; measured per-segment rate) "
                  f"+ (2n - 1) x r x t_seg20 (lift each segment, join n - 1 times; r in [{rec_lo}, {rec_hi}] is an "
                  "ESTIMATE) + privacy-circuit wrap (measured if --ppe given, else an estimate). Peak RSS: segments are "
                  "proven one at a time and spilled to $TMPDIR on disk (risc0-zkvm 3.0.5 ExecutorImpl::run), so the "
                  "peak is that of one po2-20 segment proof, independent of segment count (measured)."),
        "projection": proj,
        "peak_rss_projection_gib": peak if ppe_peak is None else max(peak or 0, ppe_peak),
        "label": "estimate (built on measured per-segment time and peak RSS)",
    }
    os.makedirs(a.out, exist_ok=True)
    dump(doc, os.path.join(a.out, "k4-projection.json"))
    print(json.dumps({k: doc[k] for k in ("measured", "projection", "peak_rss_projection_gib")}, indent=1))
    return 0


# ------------------------------------------------------------------------------------------------------------------
# import-trace: W1's cycle-trace build log -> raw/b0-phases.json
# ------------------------------------------------------------------------------------------------------------------

BLS_PHASES = ("key_aggregation", "sig_decompress", "g2_subgroup_check", "hash_to_curve", "miller_loop_2",
              "final_exponentiation")


def cmd_import_trace(a):
    txt = open(a.log, encoding="utf-8", errors="replace").read()
    img = re.search(r"image id ([0-9a-f]{64})", txt)
    blocks = re.split(r"^R0VM\[\d+\] cycle-trace start at \d+$", txt, flags=re.M)[1:]
    steps = []
    for b in blocks:
        st = {}
        m = re.search(r"phases \(duplicate computation\): (.*)$", b, re.M)
        if m:
            for k, v in re.findall(r"(\w+) (\d+)", m.group(1)):
                st[k] = int(v)
            nm = re.search(r"\((\d+) non-signers\)", m.group(1))
            st["non_signers"] = int(nm.group(1)) if nm else None
        for key, rx in (("witness_load_and_pre_store", r"witness load \+ pre-store commitment[^:]*: (\d+)"),
                        ("decode_and_prechecks", r"decode update \+ pre-checks: (\d+)"),
                        ("helios_verify_and_apply", r"Helios verify_\* \+ apply_\*: (\d+)"),
                        ("post_store_and_head", r"post-store commitment \+ head: (\d+)")):
            mm = re.search(rx, b)
            st[key] = int(mm.group(1)) if mm else None
        ex = re.search(r"^executor (\S+): user cycles", b, re.M)
        st["step"] = ex.group(1) if ex else None
        st["bls_sum"] = sum(st.get(k, 0) for k in BLS_PHASES)
        steps.append(st)
    doc = {"what": "B0 per-phase cycles of lc_verify V1 (W1 cycle-trace build: Helios's BLS steps run once more on "
                   "the same inputs and timed with env::cycle_count; Helios itself unmodified)",
           "label": "measured by duplicate computation", "source_log": os.path.basename(a.log),
           "trace_image_id": img.group(1) if img else None,
           "note": "The trace ELF predates W1's glue fix (committee root via accelerated sha2, no re-hash of an "
                   "unchanged next committee). That fix changes witness_load_and_pre_store and post_store_and_head, "
                   "not the BLS phases or the Helios call.",
           "steps": steps}
    dump(doc, a.out)
    print(f"import-trace: {len(steps)} steps -> {a.out}")
    return 0


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("build")
    b.add_argument("--raw", required=True)
    b.add_argument("--out", required=True)
    p = sub.add_parser("prove-record")
    p.add_argument("--dir", required=True)
    p.add_argument("--name", required=True)
    p.add_argument("--exit", required=True)
    p.add_argument("--dev-mode", required=True)
    p.add_argument("--receipt")
    p.add_argument("--receipt-regex")
    p.add_argument("--cycles")
    s = sub.add_parser("prove-summary")
    s.add_argument("--prove-dir", required=True)
    s.add_argument("--out", required=True)
    j = sub.add_parser("project")
    j.add_argument("--calib", action="append", required=True)
    j.add_argument("--ppe")
    j.add_argument("--ppe-json")
    j.add_argument("--exec", required=True)
    j.add_argument("--out", required=True)
    j.add_argument("--recursion-lo", type=float, default=0.10)
    j.add_argument("--recursion-hi", type=float, default=0.35)
    j.add_argument("--wrap-s-est", type=float, default=600.0)
    j.add_argument("--wrap-s", type=float, help="measured privacy-circuit wrap seconds (e.g. W0 hello-world private)")
    j.add_argument("--rec-composite")
    j.add_argument("--rec-succinct")
    j.add_argument("--rec-segments", type=int)
    j.add_argument("--case", action="append", help="extra case label:padded_cycles:segments[:source]")
    j.add_argument("--wrap-s-hi", type=float, help="upper end of the circuit wrap (estimate)")
    t = sub.add_parser("import-trace")
    t.add_argument("--log", required=True)
    t.add_argument("--out", required=True)
    a = ap.parse_args()
    return {"import-trace": cmd_import_trace, "build": cmd_build, "prove-record": cmd_prove_record, "prove-summary": cmd_prove_summary,
            "project": cmd_project}[a.cmd](a)


if __name__ == "__main__":
    sys.exit(main())
