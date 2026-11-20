#!/usr/bin/env python3
"""Records of the bridge round trip with real proofs (e2e_v4.sh): the private claim and the burn and release.

    python3 -I tools/bridge_cli/record_v4.py claim|burn --flow DIR --target sepolia|anvil-fork --out FILE
    python3 -I tools/bridge_cli/record_v4.py --self-test

Reads only files the stages wrote into the flow directory (flow.env, broadcast copies, attest-root.json, claim1.json,
claim1-tx.json, claim2.txt, burn-tx.json, the proving jobs' record.json, the Groth16 meta.json). Every field the
record needs is listed in `missing` when absent, and `complete` is true only when none is missing and the target is
Sepolia. A complete record is written to --out; an incomplete one to <out>.incomplete (exit 1). A proof made in dev
mode is refused outright (exit 2, written to <out>.incomplete).
"""

import argparse
import json
import os
import sys
import tempfile


def env(flow):
    out = {}
    p = os.path.join(flow, "flow.env")
    if os.path.exists(p):
        for line in open(p):
            if "=" in line:
                k, v = line.rstrip("\n").split("=", 1)
                out[k] = v
    return out


def load(flow, name):
    p = name if os.path.isabs(name) else os.path.join(flow, name)
    try:
        return json.load(open(p))
    except (OSError, ValueError):
        return None


def text(flow, name):
    try:
        return open(os.path.join(flow, name), errors="replace").read()
    except OSError:
        return ""


def receipt(flow, contract):
    b = load(flow, f"broadcast-{contract}-latest.json") or {}
    r = (b.get("receipts") or [{}])[-1]
    tx = r.get("transactionHash")
    blk = r.get("blockNumber")
    return {"tx": tx, "block": int(blk, 16) if isinstance(blk, str) else blk,
            "gas": int(r["gasUsed"], 16) if isinstance(r.get("gasUsed"), str) else r.get("gasUsed"),
            "status": r.get("status")}


def job(path):
    rec = load("", os.path.join(path, "record.json")) if path else None
    if not rec:
        return None
    return {"wall_seconds": rec.get("wall_s"), "peak_rss_gib": rec.get("peak_rss_gib"), "dev_mode": rec.get("dev_mode"),
            "start_utc": rec.get("start_utc"), "end_utc": rec.get("end_utc"), "job": path}


def finish(doc, need, target, jobs, out, must_be_true=()):
    missing = [k for k in need if doc.get(k) in (None, "", [], {})]
    missing += [k for k in must_be_true if doc.get(k) is not True]
    dev = [j for j in jobs if j and j.get("dev_mode")]
    doc["risc0_dev_mode"] = "unset" if jobs and all(j and j.get("dev_mode") is False for j in jobs) else (
        "SET" if dev else None)
    if doc["risc0_dev_mode"] != "unset":
        missing.append("risc0_dev_mode")
    doc["target"] = target
    doc["missing"] = missing
    doc["complete"] = not missing and target == "sepolia"
    dst = out if doc["complete"] else out + ".incomplete"
    os.makedirs(os.path.dirname(os.path.abspath(dst)), exist_ok=True)
    with open(dst, "w") as f:
        json.dump(doc, f, indent=2)
        f.write("\n")
    print(json.dumps({"complete": doc["complete"], "missing": missing, "out": dst}))
    if dev:
        print("REFUSED: a proof of this record ran with RISC0_DEV_MODE set", file=sys.stderr)
        return 2
    return 0 if doc["complete"] else 1


def balance(flow, name):
    b = load(flow, name)
    try:
        return int(b["balance"]) if b else None
    except (KeyError, ValueError, TypeError):
        return None


def claim(flow, target, out):
    e = env(flow)
    ar = load(flow, "attest-root.json") or {}
    c1 = load(flow, "claim1.json") or {}
    ct = load(flow, "claim1-tx.json") or {}
    c2 = text(flow, "claim2.txt")
    j = job(e.get("CLAIM_JOB"))
    dep = receipt(flow, "DepositOne")
    doc = {
        "what": "Private bridge claim with a real proof: a Sepolia deposit into vault v4, its root attested on LEZ "
                "through the Sepolia light-client instance (public transaction), then the private claim on seq-pub "
                "(RISC0_DEV_MODE unset), then the same claim again, refused with E11 while the program runs (no proof "
                "is made for it).",
        "label": "measured, real proof",
        "vault": e.get("VAULT"), "chain_id": int(e["CHAIN_ID"]) if e.get("CHAIN_ID") else None,
        "deposit_tx": dep["tx"] or e.get("DEPOSIT_TX"), "deposit_block": dep["block"] or (
            int(e["DEPOSIT_BLOCK"]) if e.get("DEPOSIT_BLOCK") else None),
        "deposit_index": int(e["DEPOSIT_INDEX"]) if e.get("DEPOSIT_INDEX") else None,
        "attest_instance": ar.get("instance") or "5ydZVRKHVECxKtXBwHnW9N6bndDADwZoGSx5tDsp4UXN",
        "record_slot": ar.get("record_slot"), "record_eth_block": ar.get("eth_block"),
        "attest_root_tx": ar.get("tx"), "root": ar.get("root"),
        "claim_tx": c1.get("tx"), "claim_block": ct.get("block_id"), "claim_reverted": ct.get("reverted"),
        "claim_readable_from_rpc": bool(ct.get("block_id")) and ct.get("reverted") is False,
        "proof_bytes": ct.get("witness_set_bytes"), "claim_tx_wire_bytes": ct.get("tx_wire_bytes"),
        "proof_bytes_note": "borsh size of the claim transaction's witness set (succinct receipt and signatures)",
        "wall_seconds": (j or {}).get("wall_seconds"), "peak_rss_gib": (j or {}).get("peak_rss_gib"),
        "denomination_wei": c1.get("denom"),
        "e11_refused": "E11_ALREADY_CONSUMED" in c2 or ("ESA:E11" in c2 and "panicked" in c2),
        "e11_excerpt": [l for l in c2.splitlines() if "E11" in l][:3],
        "e11_repeat_seconds": float(t11) if (t11 := text(flow, "claim2-seconds.txt").strip()) else None,
        "e11_note": "the repeat is refused while the wallet runs the program, before any proof: its seconds are those "
                    "of an execution, not of a proof",
        "wrapped_balance_before": balance(flow, "balance-before-claim.json"),
        "wrapped_balance_after": balance(flow, "balance-after-claim.json"),
        "prove_job": j,
    }
    b0, b1, d = doc["wrapped_balance_before"], doc["wrapped_balance_after"], doc["denomination_wei"]
    doc["wrapped_balance_moved_by_denomination"] = (
        None if None in (b0, b1, d) else b1 - b0 == int(d))
    need = ["vault", "deposit_tx", "deposit_block", "record_slot", "claim_tx", "claim_block", "wall_seconds",
            "peak_rss_gib", "proof_bytes"]
    need += ["e11_repeat_seconds"]
    return finish(doc, need, target, [j], out,
                  must_be_true=("e11_refused", "claim_readable_from_rpc", "wrapped_balance_moved_by_denomination"))


def burn(flow, target, out):
    e = env(flow)
    bt = load(flow, "burn-tx.json") or {}
    jb = job(e.get("BURN_JOB"))
    jg = job(e.get("GROTH16_JOB"))
    meta = load(flow, f"burn-{e.get('BURN_INDEX', '0')}-groth16/meta.json") or {}
    rel = receipt(flow, "ReleaseOne")
    post = receipt(flow, "PostRoot")
    doc = {
        "what": "LEZ-side burn with a real proof on seq-pub (RISC0_DEV_MODE unset), the burn accumulator root posted to "
                "the STUB anchor by its owner, then after the 600 s delay a Groth16 burn proof (RISC Zero 3.1.0) and "
                "Vault.release on Sepolia through the 3.1.0 emergency-stop verifier.",
        "label": "measured, real proofs",
        "anchor": "stub (owner-posted root)",
        "burn_tx": bt.get("tx"), "burn_block": bt.get("block_id"), "burn_reverted": bt.get("reverted"),
        "burn_proof_wall_seconds": (jb or {}).get("wall_seconds"), "burn_proof_peak_rss_gib": (jb or {}).get("peak_rss_gib"),
        "post_root_tx": post["tx"], "post_root_block": post["block"],
        "groth16_wall_seconds": (jg or {}).get("wall_seconds"), "groth16_peak_rss_gib": (jg or {}).get("peak_rss_gib"),
        "groth16_seal_bytes": meta.get("seal_bytes"), "groth16_image_id": meta.get("image_id"),
        "groth16_risc0_zkvm": meta.get("risc0_zkvm"),
        "release_tx": rel["tx"], "release_block": rel["block"], "release_gas": rel["gas"],
        "release_status": rel["status"],
        "groth16_note": "the Groth16 step runs the prover in a docker container (risczero/risc0-groth16-prover): its "
                        "time and peak memory cover the local process, not the container",
        "wrapped_balance_after_claim": balance(flow, "balance-after-claim.json"),
        "wrapped_balance_after_burn": balance(flow, "balance-after-burn.json"),
        "burn_job": jb, "groth16_job": jg,
    }
    a, z = doc["wrapped_balance_after_claim"], doc["wrapped_balance_after_burn"]
    doc["burn_moved_wrapped_balance_by_denomination"] = None if None in (a, z) else a - z == 1000000000000000
    doc["release_succeeded"] = doc["release_status"] in ("0x1", 1)
    doc["burn_not_reverted"] = doc["burn_reverted"] is False
    need = ["burn_tx", "burn_block", "burn_proof_wall_seconds", "burn_proof_peak_rss_gib", "groth16_wall_seconds",
            "groth16_peak_rss_gib", "release_tx", "release_gas"]
    if doc["groth16_image_id"] not in (None, "0xf929f9f441d9aa652b9914b4ce6c6a7b445aa90d189c075af1e1e9fe3b1497cf"):
        need.append("groth16_image_id (not the vault's burn image id)")
    return finish(doc, need, target, [jb, jg], out,
                  must_be_true=("release_succeeded", "burn_not_reverted", "burn_moved_wrapped_balance_by_denomination"))


def self_test():
    """Synthetic flow directories: a complete one passes, an incomplete one names what is missing."""
    with tempfile.TemporaryDirectory() as d:
        jobd = os.path.join(d, "job")
        os.makedirs(jobd)
        json.dump({"wall_s": 2400.5, "peak_rss_gib": 9.2, "dev_mode": False}, open(os.path.join(jobd, "record.json"), "w"))
        open(os.path.join(d, "flow.env"), "w").write(
            f"VAULT=0xabc\nCHAIN_ID=11155111\nDEPOSIT_INDEX=0\nDEPOSIT_BLOCK=12\nCLAIM_JOB={jobd}\n"
            f"BURN_JOB={jobd}\nGROTH16_JOB={jobd}\nBURN_INDEX=0\n")
        json.dump({"receipts": [{"transactionHash": "0xd", "blockNumber": "0xc", "gasUsed": "0x100", "status": "0x1"}]},
                  open(os.path.join(d, "broadcast-DepositOne-latest.json"), "w"))
        json.dump({"record_slot": 5, "eth_block": 13, "tx": "a", "root": "0x1"}, open(os.path.join(d, "attest-root.json"), "w"))
        json.dump({"tx": "c1", "denom": "1000000000000000"}, open(os.path.join(d, "claim1.json"), "w"))
        json.dump({"block_id": 77, "reverted": False, "witness_set_bytes": 223551, "tx_wire_bytes": 300000},
                  open(os.path.join(d, "claim1-tx.json"), "w"))
        open(os.path.join(d, "claim2.txt"), "w").write("panicked at ...\nESA:E11\nE11_ALREADY_CONSUMED\n")
        open(os.path.join(d, "claim2-seconds.txt"), "w").write("3.4\n")
        json.dump({"balance": "0"}, open(os.path.join(d, "balance-before-claim.json"), "w"))
        json.dump({"balance": "1000000000000000"}, open(os.path.join(d, "balance-after-claim.json"), "w"))
        json.dump({"balance": "0"}, open(os.path.join(d, "balance-after-burn.json"), "w"))
        out = os.path.join(d, "claim.json")
        assert claim(d, "sepolia", out) == 0, json.load(open(out))["missing"]
        assert json.load(open(out))["e11_repeat_seconds"] == 3.4
        assert claim(d, "anvil-fork", out) == 1, "a fork is never complete"
        os.remove(os.path.join(d, "claim1-tx.json"))
        assert claim(d, "sepolia", out) == 1
        assert {"claim_block", "proof_bytes", "claim_readable_from_rpc"} <= set(json.load(open(out + ".incomplete"))["missing"])
        json.dump({"balance": "5"}, open(os.path.join(d, "balance-after-claim.json"), "w"))
        assert claim(d, "sepolia", out) == 1
        assert "wrapped_balance_moved_by_denomination" in json.load(open(out + ".incomplete"))["missing"]
        json.dump({"balance": "1000000000000000"}, open(os.path.join(d, "balance-after-claim.json"), "w"))
        # burn
        json.dump({"tx": "b1", "block_id": 80, "reverted": False}, open(os.path.join(d, "burn-tx.json"), "w"))
        os.makedirs(os.path.join(d, "burn-0-groth16"))
        json.dump({"seal_bytes": 260, "image_id": "0xf929f9f441d9aa652b9914b4ce6c6a7b445aa90d189c075af1e1e9fe3b1497cf",
                   "risc0_zkvm": "3.1.0"}, open(os.path.join(d, "burn-0-groth16/meta.json"), "w"))
        for c in ("ReleaseOne", "PostRoot"):
            json.dump({"receipts": [{"transactionHash": "0xe", "blockNumber": "0x20", "gasUsed": "0x80ec3", "status": "0x1"}]},
                      open(os.path.join(d, f"broadcast-{c}-latest.json"), "w"))
        out = os.path.join(d, "burn.json")
        assert burn(d, "sepolia", out) == 0, json.load(open(out))["missing"]
        r = json.load(open(out))
        assert r["anchor"] == "stub (owner-posted root)" and r["release_gas"] == 0x80EC3
        json.dump({"wall_s": 1, "peak_rss_gib": 1, "dev_mode": True}, open(os.path.join(jobd, "record.json"), "w"))
        assert burn(d, "sepolia", out) == 2 and json.load(open(out + ".incomplete"))["risc0_dev_mode"] == "SET"
    print("record_v4 self-test OK")
    return 0


def main():
    if "--self-test" in sys.argv:
        return self_test()
    ap = argparse.ArgumentParser()
    ap.add_argument("kind", choices=["claim", "burn"])
    ap.add_argument("--flow", required=True)
    ap.add_argument("--target", required=True, choices=["sepolia", "anvil-fork"])
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    return (claim if a.kind == "claim" else burn)(a.flow, a.target, a.out)


if __name__ == "__main__":
    sys.exit(main())
