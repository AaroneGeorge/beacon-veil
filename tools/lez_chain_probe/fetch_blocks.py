#!/usr/bin/env python3
"""Fetch LEZ blocks as raw borsh bytes through the sequencer's read RPC (getBlockRange, getLastBlockId).

Usage: fetch_blocks.py URL OUT_DIR [FROM [TO]]
Writes OUT_DIR/<id>.borsh for every block in [FROM, TO] (default 1 .. last). Existing files are kept. Reads only.
"""
import base64
import json
import os
import sys
import time
import urllib.request

CHUNK = 32


def rpc(url, method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(url, body, {"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as resp:
        out = json.load(resp)
    if "error" in out:
        raise SystemExit(f"{method}{params}: {out['error']}")
    return out["result"]


def main():
    if len(sys.argv) < 3:
        raise SystemExit(__doc__)
    url, out = sys.argv[1], sys.argv[2]
    first = int(sys.argv[3]) if len(sys.argv) > 3 else 1
    last = int(sys.argv[4]) if len(sys.argv) > 4 else rpc(url, "getLastBlockId", [])
    os.makedirs(out, exist_ok=True)
    n = 0
    for lo in range(first, last + 1, CHUNK):
        hi = min(lo + CHUNK - 1, last)
        if all(os.path.exists(os.path.join(out, f"{i}.borsh")) for i in range(lo, hi + 1)):
            continue
        blocks = rpc(url, "getBlockRange", [lo, hi])
        for i, b64 in zip(range(lo, hi + 1), blocks):
            raw = base64.b64decode(b64)
            if int.from_bytes(raw[:8], "little") != i:
                raise SystemExit(f"block {i}: id in bytes is {int.from_bytes(raw[:8], 'little')}")
            with open(os.path.join(out, f"{i}.borsh"), "wb") as f:
                f.write(raw)
            n += 1
        time.sleep(0.05)
    print(json.dumps({"url": url, "first": first, "last": last, "written": n}))


if __name__ == "__main__":
    main()
