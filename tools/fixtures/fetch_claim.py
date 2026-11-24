#!/usr/bin/env python3
"""Fetch the witnesses of one storage claim under a header record's execution block, with a manifest.

Stdlib only (keccak through Foundry's `cast`, as fetch.py). Run with
`python3 -I tools/fixtures/fetch_claim.py --block-hash 0x.. --beacon-slot N --address 0x.. --slot-key 0x.. --out DIR`.

The execution block is named by its hash (the `exec_hash` of the header record), so the fetched header is the one the
record commits to: its RLP is rebuilt locally and kept only if its keccak equals the hash (same code as fetch.py's
`exec_headers`). The storage proof is `eth_getProof(address, [slot_key], block number)`; it needs a node that keeps
historical state, so the archive provider goes first.

What it writes into DIR:
  block.json       eth_getBlockByHash(hash, false) body, byte for byte
  header.rlp       the RLP execution header rebuilt from block.json (keccak equals the block hash)
  proof.json       eth_getProof body, byte for byte
  claim.json       the claim: chain id, beacon slot, block hash and number, state root, address, slot key, value
  manifest.json    URL, provider, method, UTC time, bytes and sha256 of every file
Response bodies are stored as received; they are never executed.
"""

import argparse
import datetime as dt
import hashlib
import importlib.util
import json
import os
import sys
import time
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("bv_fetch", os.path.join(HERE, "fetch.py"))
fetch = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(fetch)

# Archive first: eth_getProof at a block more than ~128 blocks old needs historical state, which publicnode refuses.
EXEC_RPCS = [
    ("ethpandaops", "https://rpc.sepolia.ethpandaops.io"),
    ("publicnode", "https://ethereum-sepolia-rpc.publicnode.com"),
]
CHAIN_ID = 11_155_111


def rpc(method, params):
    req = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    errors = []
    for pid, url in EXEC_RPCS:
        try:
            r = urllib.request.Request(url, data=req, headers={"content-type": "application/json",
                                                               "User-Agent": fetch.UA})
            with urllib.request.urlopen(r, timeout=60) as resp:
                body = resp.read()
            if json.loads(body).get("result"):
                return pid, url, body
            errors.append(f"{pid}: {json.loads(body).get('error')}")
        except Exception as e:  # noqa: BLE001 - try the next provider
            errors.append(f"{pid}: {type(e).__name__}")
        time.sleep(1)
    raise RuntimeError(f"{method}: no provider answered: {errors}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--block-hash", required=True)
    ap.add_argument("--beacon-slot", type=int, required=True)
    ap.add_argument("--address", required=True)
    ap.add_argument("--slot-key", required=True)
    ap.add_argument("--note", default="")
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    bh = a.block_hash.lower()
    files = {}

    def save(name, body, meta):
        with open(os.path.join(a.out, name), "wb") as f:
            f.write(body)
        files[name] = dict(meta, bytes=len(body), sha256=hashlib.sha256(body).hexdigest())

    at = dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    pid, url, body = rpc("eth_getBlockByHash", [bh, False])
    blk = json.loads(body)["result"]
    rlp = fetch.header_rlp(blk)
    if fetch.keccak_hex(rlp) != bh:
        raise RuntimeError("rebuilt header RLP does not hash to the block hash")
    number = int(blk["number"], 16)
    save("block.json", body, {"provider": pid, "url": url, "method": "eth_getBlockByHash(hash,false)",
                              "fetched_at_utc": at})
    save("header.rlp", rlp, {"derived_from": "block.json", "keccak256_equals_block_hash": True,
                             "field_order": fetch.HEADER_FIELDS})

    at = dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    pid, url, body = rpc("eth_getProof", [a.address, [a.slot_key], hex(number)])
    proof = json.loads(body)["result"]
    save("proof.json", body, {"provider": pid, "url": url, "fetched_at_utc": at,
                              "method": f"eth_getProof({a.address},[{a.slot_key}],{hex(number)})"})
    sp = proof["storageProof"][0]
    value = "0x" + int(sp["value"], 16).to_bytes(32, "big").hex()
    claim = {
        "chain_id": CHAIN_ID,
        "beacon_slot": a.beacon_slot,
        "block_hash": bh,
        "block_number": number,
        "state_root": blk["stateRoot"],
        "address": a.address,
        "slot_key": a.slot_key,
        "value": value,
        "storage_hash": proof["storageHash"],
        "note": a.note,
    }
    save("claim.json", (json.dumps(claim, indent=2) + "\n").encode(), {"derived_from": ["block.json", "proof.json"]})
    manifest = {"what": "witnesses of one storage claim under a header record's execution block", "files": files}
    with open(os.path.join(a.out, "manifest.json"), "w") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")
    print(f"block {number} {bh}: {a.address} [{a.slot_key}] = {value}", file=sys.stderr)


if __name__ == "__main__":
    main()
