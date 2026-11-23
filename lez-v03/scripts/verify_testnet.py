#!/usr/bin/env python3
"""Re-check evidence/public-testnet against the public LEZ testnet 0.3 (or, with --offline, against its own files).

Stdlib only; run with `python3 -I`. Reads: deployment.json, run.json, txs.tsv, IMAGE_IDS, raw/<hash>.tx.b64,
raw/<block>.block.b64. Exit 0 when every check passes, 1 when any fails.

Online (--rpc URL), at most one request a second:
  1. each program header (deployment.json) holds an immutable ProgramHeader whose image id equals deployment.json
     and IMAGE_IDS;
  2. the instance shard equals run.json instance_state_hex; each record shard equals run.json records[].bytes_hex;
  3. every transaction of txs.tsv: getTransaction(hash) returns it in the block txs.tsv names, its bytes hash to
     the hash, and the block (getBlock) contains its bytes; the block's Bedrock status is printed;
  4. exactly the updates of run.json's mode applied (minimal 1, medium 2, full 3; at least 2 when no mode is recorded),
     each as Begin, Miller, Finish; the instance head slot is above the checkpoint slot;
  5. the refused transaction (expected_exit set) is in a block and nothing after the last Finish changed the
     instance (its state equals run.json, recorded after the refusal).
Offline (--offline): the same files without the network: hashes of the saved raw bytes, blocks containing them,
block ids, IMAGE_IDS against deployment.json, run.json's instance state decoding to the recorded head.
"""

import argparse
import base64
import hashlib
import json
import struct
import sys
import time
import urllib.request

LOADER = bytes([0xFE] * 32)
B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b58decode(s):
    n = 0
    for c in s:
        n = n * 58 + B58.index(c)
    raw = n.to_bytes(32, "big")
    return raw


def b58encode(b):
    n = int.from_bytes(b, "big")
    out = ""
    while n:
        n, r = divmod(n, 58)
        out = B58[r] + out
    pad = len(b) - len(b.lstrip(b"\0"))
    return "1" * pad + out


class Rpc:
    def __init__(self, url):
        self.url = url
        self.last = 0.0
        self.calls = 0

    def call(self, method, params):
        wait = 2
        for attempt in range(5):
            dt = time.monotonic() - self.last
            if dt < 1.0:
                time.sleep(1.0 - dt)
            self.last = time.monotonic()
            self.calls += 1
            body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
            req = urllib.request.Request(self.url, data=body, headers={"content-type": "application/json"})
            try:
                with urllib.request.urlopen(req, timeout=30) as r:
                    v = json.loads(r.read())
            except Exception as e:  # transport: retry
                if attempt == 4:
                    raise
                print(f"  {method}: {e}; retry in {wait} s", file=sys.stderr)
                time.sleep(wait)
                wait *= 2
                continue
            if "error" in v:
                raise RuntimeError(f"{method}: {v['error']}")
            return v.get("result")


class Checks:
    def __init__(self):
        self.failed = 0
        self.passed = 0

    def check(self, ok, what):
        print(("ok    " if ok else "FAIL  ") + what)
        if ok:
            self.passed += 1
        else:
            self.failed += 1
        return ok


def read_tsv(path):
    with open(path) as f:
        lines = f.read().splitlines()
    head = lines[0].split("\t")
    return [dict(zip(head, l.split("\t"))) for l in lines[1:] if l.strip()]


def shard(account_json, program_b58):
    """Bytes of `program`'s shard in a getAccount answer."""
    shards = (account_json or {}).get("data", {}).get("shards", {})
    return bytes(shards.get(program_b58, []))


class Reader:
    def __init__(self, b):
        self.b, self.i = b, 0

    def take(self, n):
        v = self.b[self.i:self.i + n]
        if len(v) != n:
            raise ValueError("short")
        self.i += n
        return v

    def u8(self):
        return self.take(1)[0]

    def u16(self):
        return struct.unpack("<H", self.take(2))[0]

    def u32(self):
        return struct.unpack("<I", self.take(4))[0]

    def u64(self):
        return struct.unpack("<Q", self.take(8))[0]


def decode_instance(b):
    """borsh InstanceState { config: InstanceConfig, head: Head }: returns (config dict, head dict)."""
    r = Reader(b)
    cfg = {"chain_id": r.u64(), "gvr": r.take(32).hex(), "genesis_time": r.u64()}
    cfg["forks"] = [(r.u64(), r.take(4).hex()) for _ in range(r.u32())]
    cfg["checkpoint_root"] = r.take(32).hex()
    cfg["checkpoint_slot"] = r.u64()
    cfg["verifiers"] = [b58encode(r.take(32)) for _ in range(r.u32())]
    cfg["min_participants"] = r.u16()
    head = {"slot": r.u64(), "beacon_root": r.take(32).hex(), "exec_number": r.u64(), "exec_hash": r.take(32).hex(),
            "exec_state_root": r.take(32).hex(), "exec_receipts_root": r.take(32).hex(), "period": r.u64(),
            "store_commitment": r.take(32).hex()}
    if r.i != len(b):
        raise ValueError("trailing bytes")
    return cfg, head


def decode_header(b):
    """borsh ProgramHeader { image_id: [u32; 8], program_first_segment: [u8; 32], immutable: bool }."""
    r = Reader(b)
    words = [r.u32() for _ in range(8)]
    first = r.take(32)
    immutable = r.u8() == 1
    return b"".join(struct.pack("<I", w) for w in words).hex(), b58encode(first), immutable


def block_meta(raw):
    """(block_id, timestamp_ms, bedrock status name) of a borsh Block."""
    block_id = struct.unpack("<Q", raw[:8])[0]
    ts = struct.unpack("<Q", raw[72:80])[0]
    status = {0: "Pending", 1: "Safe", 2: "Finalized"}.get(raw[-1], f"?{raw[-1]}")
    return block_id, ts, status


def tx_hash(lee_tx_bytes):
    # LeeTransaction::Public is variant 0; the hash is sha256 of the inner PublicTransaction's borsh.
    return hashlib.sha256(lee_tx_bytes[1:]).hexdigest()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("evidence")
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--rpc")
    g.add_argument("--offline", action="store_true")
    a = ap.parse_args()
    ev = a.evidence.rstrip("/")
    c = Checks()

    dep = json.load(open(f"{ev}/deployment.json"))
    run = json.load(open(f"{ev}/run.json"))
    txs = read_tsv(f"{ev}/txs.tsv")
    image_ids = {}
    for line in open(f"{ev}/IMAGE_IDS"):
        p = line.split()
        if len(p) == 2:
            image_ids[p[0]] = p[1]

    # Files agree with each other.
    for name, p in dep["programs"].items():
        c.check(image_ids.get(name) == p["image_id"], f"{name}: IMAGE_IDS equals deployment.json ({p['image_id']})")
    inst_bytes = bytes.fromhex(run["instance_state_hex"])
    cfg, head = decode_instance(inst_bytes)
    c.check(head["slot"] == run["head_slot"] and head["store_commitment"] == run["store_commitment"],
            f"run.json instance state decodes to head slot {head['slot']}")
    c.check(cfg["checkpoint_slot"] == run["checkpoint_slot"], f"checkpoint slot {cfg['checkpoint_slot']}")
    c.check(head["slot"] > cfg["checkpoint_slot"],
            f"finalized slot moved: {cfg['checkpoint_slot']} -> {head['slot']}")
    c.check(cfg["verifiers"] == [run["lc_verify_v3_v03"]], "the instance's only verifier is lc_verify_v3_v03")

    steps = [t for t in txs if t["step"].split(" ")[-1] in ("Begin", "Miller", "Finish") and t["expected_exit"] == "-"]
    by_update = {}
    for t in steps:
        u, s = t["step"].rsplit(" ", 1)
        by_update.setdefault(u, []).append(s)
    full = [u for u, s in by_update.items() if s == ["Begin", "Miller", "Finish"]]
    mode = run.get("mode")
    need = {"minimal": 1, "medium": 2, "full": 3}.get(mode, 2)
    c.check((len(full) == need if mode else len(full) >= need) and len(full) == len(by_update),
            f"{len(full)} updates applied, each as Begin, Miller, Finish ({', '.join(full)}); mode {mode or '-'} needs {need}")
    refusals = [t for t in txs if t["expected_exit"] != "-"]
    c.check(len(refusals) >= 1, f"{len(refusals)} refused transaction(s) recorded")

    # Saved bytes.
    for t in txs:
        raw = base64.b64decode(open(f"{ev}/raw/{t['hash']}.tx.b64").read())
        c.check(tx_hash(raw) == t["hash"] and len(raw) == int(t["bytes"]),
                f"{t['step']}: raw bytes hash to {t['hash'][:16]}... ({len(raw)} bytes)")
        try:
            braw = base64.b64decode(open(f"{ev}/raw/{t['block']}.block.b64").read())
        except FileNotFoundError:
            c.check(False, f"{t['step']}: raw block {t['block']} saved")
            continue
        bid, _, _ = block_meta(braw)
        c.check(bid == int(t["block"]) and raw in braw, f"{t['step']}: saved block {t['block']} contains it")

    if a.offline:
        print(f"\n{c.passed} checks passed, {c.failed} failed (offline)")
        return 1 if c.failed else 0

    rpc = Rpc(a.rpc)
    print(f"\nagainst {a.rpc}: head block {rpc.call('getLastBlockId', {})}")
    for name, p in dep["programs"].items():
        acc = rpc.call("getAccount", {"account_id": p["header"]})
        try:
            img, first, immutable = decode_header(shard(acc, b58encode(LOADER)))
            ok = immutable and img == p["image_id"] == image_ids.get(name) and first == p["segments"][0]
        except Exception as e:
            ok, img, immutable = False, str(e), None
        c.check(ok, f"{name}: header {p['header']} immutable={immutable}, image id {img}")
    attest = run["attest_v03"]
    acc = rpc.call("getAccount", {"account_id": run["instance"]})
    c.check(shard(acc, attest) == inst_bytes, f"instance {run['instance']} equals run.json (head slot {head['slot']})")
    for rec in run["records"]:
        acc = rpc.call("getAccount", {"account_id": rec["account"]})
        c.check(shard(acc, attest).hex() == rec["bytes_hex"], f"record of slot {rec['slot']} equals run.json")
    statuses = {}
    for t in txs:
        got = rpc.call("getTransaction", {"tx_hash": t["hash"]})
        if not got:
            c.check(False, f"{t['step']}: {t['hash'][:16]}... known to the sequencer")
            continue
        raw = base64.b64decode(got[0])
        c.check(got[1] == int(t["block"]) and tx_hash(raw) == t["hash"],
                f"{t['step']}: getTransaction gives block {got[1]}")
        b = int(t["block"])
        if b not in statuses:
            braw = base64.b64decode(rpc.call("getBlock", {"block_id": b}))
            statuses[b] = (braw, block_meta(braw)[2])
        c.check(raw in statuses[b][0], f"{t['step']}: block {b} ({statuses[b][1]}) contains it")
    final = sum(1 for _, s in statuses.values() if s == "Finalized")
    print(f"\nblocks: {len(statuses)}, Finalized on the Logos Blockchain per the sequencer: {final}; "
          + ", ".join(f"{b}:{s}" for b, (_, s) in sorted(statuses.items())))
    print(f"{c.passed} checks passed, {c.failed} failed; {rpc.calls} RPC requests")
    return 1 if c.failed else 0


if __name__ == "__main__":
    sys.exit(main())
