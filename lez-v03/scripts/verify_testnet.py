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
  4. exactly the updates of run.json's mode applied (minimal 1, medium 2, full 3; keeper 1 + the updates of every
     round; at least 2 when no mode is recorded), each as Begin, Miller, Finish; the instance head slot is above the
     checkpoint slot;
  5. the refused transaction (expected_exit set) is in a block and nothing after the last Finish changed the
     instance (its state equals run.json, recorded after the refusal).
A run sent in stages is recorded after each stage. A record whose run.json says "stage": "deploy+init" (the deployment
and Init sent, nothing else yet) is checked for exactly that instead of 4 and 5: txs.tsv holds the deployment
transactions and one Init and nothing else, and the instance head is still the checkpoint. Without "stage" (the record
of the finished run) every check above applies; any other "stage" fails.
Mode "keeper" (the minimal run, then rounds that each applied a fresh Sepolia update; run.json "keeper"): every round's
updates and transactions are in txs.tsv; the instance head strictly increases from the minimal run's head through
every round and ends at run.json's head; every round's head has a header record (checked on chain with the others);
the store's period never goes back, and when it moved (1421 -> 1422) an updates_by_range item (the next sync committee)
was applied in or before that round; each faucet claim's transfer to the payer has its raw bytes in raw/ and, online,
is in the block recorded.
Attest (run.json "attest", when present): the Attest of attest_fact_v03 and that program's deployment are in
txs.tsv; the header record it used is the instance's record of run.json; the Sepolia data recorded (attest/data:
header.rlp, proof.json, claim.json) proves the fact again here, independently of the programs: keccak(header.rlp)
equals the record's execution block hash, the account and storage proofs verify under the header's state root and give
the recorded value; the statement the program wrote (statement_hex) decodes to that fact, its id and predicate hash
recompute, and the fact account is the program's address for it. Online: the fact account holds statement_hex.
Offline (--offline): the same files without the network: hashes of the saved raw bytes, blocks containing them,
block ids, IMAGE_IDS against deployment.json, run.json's instance state decoding to the recorded head, the Attest's
fact against the recorded Sepolia data.
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


def check_keeper(c, run, head, full, txs, ev):
    """Mode keeper: the rounds after the minimal run (see the module text)."""
    k = run.get("keeper") or {}
    rounds = k.get("rounds", [])
    if not c.check(len(rounds) >= 1, f"keeper: {len(rounds)} round(s) recorded"):
        return
    by_hash = {t["hash"]: t for t in txs}
    labels = [l for r in rounds for l in r["updates"]]
    c.check(len(set(labels)) == len(labels) and set(labels) <= set(full) and len(full) == 1 + len(labels),
            f"keeper: the rounds' {len(labels)} updates and the minimal run's one are the {len(full)} applied")
    for r in rounds:
        ok = all(by_hash.get(t["hash"], {}).get("block") == str(t["block"])
                 and by_hash[t["hash"]]["step"] == t["step"] for t in r["txs"])
        steps = sorted(t["step"] for t in r["txs"])
        want = sorted(f"{l} {s}" for l in r["updates"] for s in ("Begin", "Miller", "Finish"))
        c.check(ok and steps == want, f"keeper round {r['round']}: its {len(r['txs'])} transactions are in txs.tsv "
                f"(blocks {min((t['block'] for t in r['txs']), default='-')}..{max((t['block'] for t in r['txs']), default='-')})")
    heads = [k["base_head_slot"]] + [r["head_slot"] for r in rounds]
    c.check(all(a < b for a, b in zip(heads, heads[1:]))
            and all(r["head_slot_before"] == h for r, h in zip(rounds, heads))
            and heads[-1] == head["slot"],
            "keeper: head slot strictly increases " + " -> ".join(map(str, heads)) + " (the instance's head)")
    rec = {x["slot"] for x in run["records"]}
    c.check(all(h in rec for h in heads), "keeper: every head has a header record in run.json")
    periods = [k["base_period"]] + [r["period"] for r in rounds]
    c.check(all(r["period"] == r["head_slot"] // 8192 for r in rounds)
            and all(a <= b for a, b in zip(periods, periods[1:])) and periods[-1] == head["period"],
            "keeper: store period per round " + " -> ".join(map(str, periods)) + ", never back")
    moved = [r for r in rounds if r["period"] > r["period_before"]]
    if moved:
        r = moved[0]
        before = [l for x in rounds if x["round"] <= r["round"] for l in x["updates"]]
        c.check(any(l.startswith("updates_") for l in before),
                f"keeper: period crossing {r['period_before']} -> {r['period']} in round {r['round']} (head "
                f"{r['head_slot_before']} -> {r['head_slot']}), the next sync committee learned from "
                + ", ".join(l for l in before if l.startswith("updates_")))
    else:
        print(f"info  keeper: no period crossing yet (store period {periods[-1]}, boundary slot "
              f"{k.get('boundary_slot')})")
    claims = {x["n"]: x for x in k.get("claims", [])}
    c.check(all(n in claims for r in rounds for n in r.get("funding_claims", [])),
            f"keeper: {len(claims)} faucet claim(s) listed, every funded round names its claims")
    for x in claims.values():
        if not x.get("transfer_tx"):
            continue
        try:
            raw = base64.b64decode(open(f"{ev}/raw/{x['transfer_tx']}.tx.b64").read())
            ok = tx_hash(raw) == x["transfer_tx"]
        except FileNotFoundError:
            ok = False
        c.check(ok, f"claim {x['n']}: transfer {x['amount']} from {x['donor']} to the payer, raw bytes hash to "
                f"{x['transfer_tx'][:16]}...")


# ---- keccak-256 (Ethereum's, not SHA3-256), RLP and Merkle-Patricia proofs: stdlib only, for the Attest check ----

_RC = [0x0000000000000001, 0x0000000000008082, 0x800000000000808A, 0x8000000080008000, 0x000000000000808B,
       0x0000000080000001, 0x8000000080008081, 0x8000000000008009, 0x000000000000008A, 0x0000000000000088,
       0x0000000080008009, 0x000000008000000A, 0x000000008000808B, 0x800000000000008B, 0x8000000000008089,
       0x8000000000008003, 0x8000000000008002, 0x8000000000000080, 0x000000000000800A, 0x800000008000000A,
       0x8000000080008081, 0x8000000000008080, 0x0000000080000001, 0x8000000080008008]
_ROT = [[0, 36, 3, 41, 18], [1, 44, 10, 45, 2], [62, 6, 43, 15, 61], [28, 55, 25, 21, 56], [27, 20, 39, 8, 14]]
_M = (1 << 64) - 1


def _keccak_f(a):
    for rc in _RC:
        c = [a[x][0] ^ a[x][1] ^ a[x][2] ^ a[x][3] ^ a[x][4] for x in range(5)]
        d = [c[(x - 1) % 5] ^ (((c[(x + 1) % 5] << 1) | (c[(x + 1) % 5] >> 63)) & _M) for x in range(5)]
        a = [[a[x][y] ^ d[x] for y in range(5)] for x in range(5)]
        b = [[0] * 5 for _ in range(5)]
        for x in range(5):
            for y in range(5):
                r = _ROT[x][y]
                b[y][(2 * x + 3 * y) % 5] = ((a[x][y] << r) | (a[x][y] >> (64 - r))) & _M if r else a[x][y]
        a = [[b[x][y] ^ ((~b[(x + 1) % 5][y]) & b[(x + 2) % 5][y]) for y in range(5)] for x in range(5)]
        a[0][0] ^= rc
    return a


def keccak256(data):
    rate = 136
    pad = rate - len(data) % rate
    msg = bytearray(data) + (b"\x81" if pad == 1 else b"\x01" + b"\x00" * (pad - 2) + b"\x80")
    a = [[0] * 5 for _ in range(5)]
    for off in range(0, len(msg), rate):
        blk = msg[off:off + rate]
        for i in range(rate // 8):
            a[i % 5][i // 5] ^= int.from_bytes(blk[8 * i:8 * i + 8], "little")
        a = _keccak_f(a)
    return b"".join(a[i % 5][i // 5].to_bytes(8, "little") for i in range(4))


def rlp_decode(b):
    """One RLP item (bytes or a list of items); the whole input must be that item."""
    def item(i):
        p = b[i]
        if p < 0x80:
            return b[i:i + 1], i + 1
        if p < 0xB8:
            n = p - 0x80
            return b[i + 1:i + 1 + n], i + 1 + n
        if p < 0xC0:
            ll = p - 0xB7
            n = int.from_bytes(b[i + 1:i + 1 + ll], "big")
            return b[i + 1 + ll:i + 1 + ll + n], i + 1 + ll + n
        if p < 0xF8:
            n, j = p - 0xC0, i + 1
        else:
            ll = p - 0xF7
            n, j = int.from_bytes(b[i + 1:i + 1 + ll], "big"), i + 1 + ll
        out, end = [], j + n
        while j < end:
            v, j = item(j)
            out.append(v)
        if j != end:
            raise ValueError("rlp: list length")
        return out, end
    v, end = item(0)
    if end != len(b):
        raise ValueError("rlp: trailing bytes")
    return v


def mpt_get(root, key, proof):
    """Value at `key` (32 bytes, hashed path) under `root` from `proof` nodes (root first); None if absent."""
    nib = [x for byte in key for x in (byte >> 4, byte & 15)]
    want, i = root, 0
    nodes = list(proof)
    while True:
        if isinstance(want, list):
            node = want
        else:
            if len(want) == 0:
                return None
            raw = nodes.pop(0) if nodes else None
            if raw is None or (keccak256(raw) != want if len(want) == 32 else raw != want):
                raise ValueError("proof node does not hash to its reference")
            node = rlp_decode(raw)
        if len(node) == 17:
            if i == len(nib):
                return node[16] or None
            want, i = node[nib[i]], i + 1
            continue
        path, flag = node[0], node[0][0] >> 4
        rest = ([path[0] & 15] if flag & 1 else []) + [x for byte in path[1:] for x in (byte >> 4, byte & 15)]
        if nib[i:i + len(rest)] != rest:
            return None
        i += len(rest)
        if flag & 2:
            return node[1] if i == len(nib) else None
        want = node[1]


def u64le(n):
    return struct.pack("<Q", n)


def pda(program, seed):
    return hashlib.sha256(b"/LEE/v0.2/AccountId/PDA/" + b"\0" * 8 + program + seed).digest()


def check_attest(c, run, dep, txs, ev):
    """Offline part of the Attest check (see the module text). Returns the attest dict, or None."""
    at = run.get("attest")
    if at is None:
        print("info  no Attest recorded (run.json has no attest)")
        return None
    fp = dep["programs"].get("attest_fact_v03", {})
    by_hash = {t["hash"]: t for t in txs}
    c.check(at["program"] == fp.get("header") and at["image_id"] == fp.get("image_id")
            and all(h in by_hash and by_hash[h]["step"].startswith("deploy attest_fact_v03 ") for h in fp.get("txs", []))
            and at["deploy_txs"] == fp.get("txs") and len(fp.get("txs", [])) == len(fp.get("segments", [])) + 1,
            f"attest_fact_v03 {at['program']}: deployed by its {len(fp.get('txs', []))} transactions in txs.tsv")
    t = by_hash.get(at["tx"], {})
    c.check(t.get("step") == "Attest" and t.get("block") == str(at["block"]) and t.get("expected_exit") == "-"
            and t.get("simulated_cycles") == str(at["simulated_cycles"]),
            f"Attest {at['tx'][:16]}... in txs.tsv, block {at['block']}, {at['simulated_cycles']} cycles simulated")
    rec, cl = at["record"], at["claim"]
    inst_rec = {x["slot"]: x for x in run["records"]}.get(rec["slot"], {})
    c.check(at["attest_v03"] == run["attest_v03"] and b58decode(at["attest_v03"]).hex()
            == "6e8bbdee04012449489fab98f1de14401ded8aec606dc5bfba5ee8b7f9cc223e"
            and at["instance"] == run["instance"] and inst_rec.get("account") == rec["account"]
            and inst_rec.get("bytes_hex") == rec["bytes_hex"],
            f"the record used is the instance's header record of slot {rec['slot']} ({rec['account']})")
    r = Reader(bytes.fromhex(rec["bytes_hex"]))
    chain_id, slot, _beacon_root, _num, exec_hash = r.u64(), r.u64(), r.take(32), r.u64(), r.take(32)
    c.check(chain_id == cl["chain_id"] == 11155111 and slot == cl["beacon_slot"] == rec["slot"]
            and exec_hash.hex() == cl["block_hash"] == rec["exec_hash"],
            f"record: Sepolia slot {slot}, execution block {exec_hash.hex()[:16]}...")
    # The recorded Sepolia data proves the fact again.
    d = f"{ev}/attest/data"
    try:
        header = open(f"{d}/header.rlp", "rb").read()
        proof = json.load(open(f"{d}/proof.json"))["result"]
        claim = json.load(open(f"{d}/claim.json"))
    except FileNotFoundError as e:
        c.check(False, f"Attest: Sepolia data saved ({e.filename})")
        return at
    h = rlp_decode(header)
    state_root, number = h[3], int.from_bytes(h[8], "big")
    c.check(keccak256(header) == exec_hash and number == cl["block_number"] == claim["block_number"],
            f"keccak(header.rlp) is the record's execution block hash; block {number}, state root "
            f"{state_root.hex()[:16]}...")
    address, slot_key, value = bytes.fromhex(cl["address"]), bytes.fromhex(cl["slot_key"]), bytes.fromhex(cl["value"])
    c.check(bytes.fromhex(claim["address"][2:]) == address and bytes.fromhex(claim["slot_key"][2:]) == slot_key
            and bytes.fromhex(claim["value"][2:]) == value, "claim.json is run.json's claim")
    try:
        acc = mpt_get(state_root, keccak256(address), [bytes.fromhex(x[2:]) for x in proof["accountProof"]])
        storage_root = rlp_decode(acc)[2]
        sp = proof["storageProof"][0]
        word = mpt_get(storage_root, keccak256(slot_key), [bytes.fromhex(x[2:]) for x in sp["proof"]])
        got = int.from_bytes(rlp_decode(word), "big").to_bytes(32, "big") if word else bytes(32)
        ok, err = got == value, ""
    except (ValueError, IndexError, TypeError) as e:
        ok, err = False, f" ({e})"
    c.check(ok, f"proof.json proves slot {slot_key.hex()[:16]}... of 0x{address.hex()} = {value.hex()[:16]}... under "
            f"that state root{err}")
    # The statement the program wrote.
    st = bytes.fromhex(at["statement_hex"])
    try:
        r = Reader(st)
        s_inst, s_chain, s_slot, s_num, kind = r.take(32), r.u64(), r.u64(), r.u64(), r.u8()
        subject, s_value = r.take(r.u32()), r.take(r.u32())
        pred_hash, sid = r.take(32), r.take(32)
        ok = r.i == len(st)
    except ValueError:
        ok = False
    if not c.check(ok, f"statement_hex decodes ({len(st)} bytes)"):
        return at
    want_id = hashlib.sha256(b"esa/stmt/v1" + u64le(chain_id) + address + slot_key + value).digest()
    c.check(b58encode(s_inst) == run["instance"] and s_chain == chain_id and s_slot == slot and s_num == number
            and kind == 1 and subject == address + slot_key and s_value == value
            and pred_hash == hashlib.sha256(b"\x00\x01" + value).digest() and sid == want_id
            and sid.hex() == at["statement"]["id"],
            f"statement: Storage, instance, slot {s_slot}, block {s_num}, value, predicate Eq, id {sid.hex()[:16]}...")
    fact = pda(b58decode(at["program"]), hashlib.sha256(b"esa/fact/v03" + s_inst + sid).digest())
    c.check(b58encode(fact) == at["fact_account"], f"fact account {at['fact_account']} is the program's for it")
    return at


def check_transfers(c, rpc, run):
    for x in (run.get("keeper") or {}).get("claims", []):
        if not x.get("transfer_tx"):
            continue
        got = rpc.call("getTransaction", {"tx_hash": x["transfer_tx"]})
        c.check(bool(got) and got[1] == x["transfer_block"]
                and tx_hash(base64.b64decode(got[0])) == x["transfer_tx"],
                f"claim {x['n']}: transfer {x['transfer_tx'][:16]}... in block {x['transfer_block']}")


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
    c.check(cfg["verifiers"] == [run["lc_verify_v3_v03"]], "the instance's only verifier is lc_verify_v3_v03")

    stage = run.get("stage")
    if stage is None:
        c.check(head["slot"] > cfg["checkpoint_slot"],
                f"finalized slot moved: {cfg['checkpoint_slot']} -> {head['slot']}")
        steps = [t for t in txs
                 if t["step"].split(" ")[-1] in ("Begin", "Miller", "Finish") and t["expected_exit"] == "-"]
        by_update = {}
        for t in steps:
            u, s = t["step"].rsplit(" ", 1)
            by_update.setdefault(u, []).append(s)
        full = [u for u, s in by_update.items() if s == ["Begin", "Miller", "Finish"]]
        mode = run.get("mode")
        need = {"minimal": 1, "medium": 2, "full": 3}.get(mode, 2)
        if mode == "keeper":
            need = 1 + sum(len(r["updates"]) for r in run.get("keeper", {}).get("rounds", []))
        c.check((len(full) == need if mode else len(full) >= need) and len(full) == len(by_update),
                f"{len(full)} updates applied, each as Begin, Miller, Finish ({', '.join(full)}); "
                f"mode {mode or '-'} needs {need}")
        refusals = [t for t in txs if t["expected_exit"] != "-"]
        c.check(len(refusals) >= 1, f"{len(refusals)} refused transaction(s) recorded")
        if mode == "keeper":
            check_keeper(c, run, head, full, txs, ev)
    elif stage == "deploy+init":
        print("partial record (stage deploy+init): no update and no refusal sent yet; checking exactly that")
        deploy_hashes = {h for p in dep["programs"].values() for h in p["txs"]}
        deploy = [t for t in txs if t["step"].startswith("deploy ")]
        init = [t for t in txs if t["step"] == "Init"]
        c.check(len(deploy) + len(init) == len(txs) and len(init) == 1
                and {t["hash"] for t in deploy} == deploy_hashes
                and all(t["expected_exit"] == "-" for t in txs),
                f"txs.tsv holds the {len(deploy)} deployment transactions of deployment.json and one Init, "
                "nothing else")
        c.check(head["slot"] == cfg["checkpoint_slot"] and head["beacon_root"] == cfg["checkpoint_root"],
                f"the instance head is the checkpoint (slot {head['slot']}): no update applied")
    else:
        c.check(False, f"run.json stage {stage!r} is known")

    attest_rec = check_attest(c, run, dep, txs, ev)

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
    if run.get("mode") == "keeper":
        check_transfers(c, rpc, run)
    if attest_rec:
        acc = rpc.call("getAccount", {"account_id": attest_rec["fact_account"]})
        c.check(shard(acc, attest_rec["program"]).hex() == attest_rec["statement_hex"],
                f"fact account {attest_rec['fact_account']} holds the statement (Attest applied on chain)")
    final = sum(1 for _, s in statuses.values() if s == "Finalized")
    print(f"\nblocks: {len(statuses)}, Finalized on the Logos Blockchain per the sequencer: {final}; "
          + ", ".join(f"{b}:{s}" for b, (_, s) in sorted(statuses.items())))
    print(f"{c.passed} checks passed, {c.failed} failed; {rpc.calls} RPC requests")
    return 1 if c.failed else 0


if __name__ == "__main__":
    sys.exit(main())
