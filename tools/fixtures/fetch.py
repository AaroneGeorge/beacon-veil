#!/usr/bin/env python3
"""Fetch Beacon API light-client fixtures with a manifest.

Stdlib only. Run with `python3 -I tools/fixtures/fetch.py --chain mainnet|sepolia [--out fixtures]`.
The Rust binary `fixtures fetch` writes the same layout and manifest schema; this script exists so that
fixtures could be fetched while the shared cargo target was locked, and as a cross-check of the Rust code.

What it writes into <out>/<chain>/:
  genesis.json, fork_schedule.json, spec.json         network constants for InstanceConfig
  updates_<P0>_<n>.json (+ .ssz from a nimbus provider) `updates_by_range` for n adjacent periods (handoffs)
  bootstrap_<slot>.json                              bootstrap at the first available epoch boundary of period P0
  finality_update_<attested_slot>.json               latest finality update (repeat runs add more)
  optimistic_update_<attested_slot>.json             latest optimistic update
  manifest.json                                      URL, provider, UTC time, sha256, bytes, consensus version,
                                                      cross-provider check per file

Providers are the logos-evm-beacon-api-module v0.2.0 defaults (rust-lib/src/beacon.rs DEFAULT_PROVIDERS), https first.
No API keys. Response bodies are stored byte for byte as received; they are never executed.
"""

import argparse
import datetime as dt
import hashlib
import json
import os
import sys
import time
import urllib.error
import urllib.request

SLOTS_PER_EPOCH = 32
EPOCHS_PER_PERIOD = 256
SLOTS_PER_PERIOD = SLOTS_PER_EPOCH * EPOCHS_PER_PERIOD

CHAINS = {
    "mainnet": {
        "chain_id": 1,
        "genesis_time": 1_606_824_023,
        "providers": [
            ("lodestar", "https://lodestar-mainnet.chainsafe.io"),
            ("nimbus-unstable", "http://unstable.mainnet.beacon-api.nimbus.team"),
            ("nimbus-testing", "http://testing.mainnet.beacon-api.nimbus.team"),
        ],
    },
    "sepolia": {
        "chain_id": 11_155_111,
        "genesis_time": 1_655_733_600,
        "providers": [
            ("lodestar", "https://lodestar-sepolia.chainsafe.io"),
            ("nimbus-unstable", "http://unstable.sepolia.beacon-api.nimbus.team"),
        ],
        # Execution JSON-RPC for gloas headers (only execution_block_hash in the light-client header).
        # rpc.sepolia.ethpandaops.io is the verified-proxy module's default (src/proxy_config.cpp); publicnode is the
        # fallback. Neither serves debug_getRawHeader, so the RLP is rebuilt locally and checked against the hash.
        "exec_rpcs": [
            ("publicnode", "https://ethereum-sepolia-rpc.publicnode.com"),
            ("ethpandaops", "https://rpc.sepolia.ethpandaops.io"),
        ],
    },
}

UA = "beacon-veil-fixtures/0.1"


def now_utc():
    return dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def get(url, accept="application/json", timeout=90):
    req = urllib.request.Request(url, headers={"Accept": accept, "User-Agent": UA})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        body = r.read()
        return r.status, dict((k.lower(), v) for k, v in r.headers.items()), body


def normalise(obj):
    """Lower-case every hex string so providers that checksum addresses compare equal."""
    if isinstance(obj, dict):
        return {k: normalise(v) for k, v in obj.items() if k != "version"}
    if isinstance(obj, list):
        return [normalise(v) for v in obj]
    if isinstance(obj, str) and obj.startswith("0x"):
        return obj.lower()
    return obj


HEADER_FIELDS = ["parentHash", "sha3Uncles", "miner", "stateRoot", "transactionsRoot", "receiptsRoot", "logsBloom",
                 "difficulty", "number", "gasLimit", "gasUsed", "timestamp", "extraData", "mixHash", "nonce",
                 "baseFeePerGas", "withdrawalsRoot", "blobGasUsed", "excessBlobGas", "parentBeaconBlockRoot",
                 "requestsHash", "blockAccessListHash", "slotNumber"]
INT_FIELDS = {"difficulty", "number", "gasLimit", "gasUsed", "timestamp", "baseFeePerGas", "blobGasUsed",
              "excessBlobGas", "slotNumber"}


def rlp_bytes(b):
    if len(b) == 1 and b[0] < 0x80:
        return b
    return rlp_len(len(b), 0x80) + b


def rlp_len(n, off):
    if n < 56:
        return bytes([off + n])
    nb = n.to_bytes((n.bit_length() + 7) // 8, "big")
    return bytes([off + 55 + len(nb)]) + nb


def header_rlp(blk):
    """RLP of an execution header from its JSON-RPC form. Fields present in the JSON, in canonical order."""
    items = []
    for k in HEADER_FIELDS:
        if k not in blk:
            continue
        v = blk[k]
        if k in INT_FIELDS:
            n = int(v, 16)
            raw = n.to_bytes((n.bit_length() + 7) // 8, "big") if n else b""
        else:
            raw = bytes.fromhex(v[2:])
        items.append(rlp_bytes(raw))
    payload = b"".join(items)
    return rlp_len(len(payload), 0xC0) + payload


def keccak_hex(data):
    """keccak-256 through Foundry's `cast keccak` (stdlib has no keccak; sha3_256 differs)."""
    import shutil
    import subprocess
    cast = shutil.which("cast") or os.path.expanduser("~/.foundry/bin/cast")
    out = subprocess.run([cast, "keccak", "0x" + data.hex()], capture_output=True, text=True, check=True)
    return out.stdout.strip().lower()


class Fetcher:
    def __init__(self, chain, outdir):
        self.chain = chain
        self.cfg = CHAINS[chain]
        self.dir = os.path.join(outdir, chain)
        os.makedirs(self.dir, exist_ok=True)
        self.manifest_path = os.path.join(self.dir, "manifest.json")
        if os.path.exists(self.manifest_path):
            with open(self.manifest_path) as f:
                self.manifest = json.load(f)
        else:
            self.manifest = {
                "chain": chain,
                "chain_id": self.cfg["chain_id"],
                "tool": "tools/fixtures (fetch.py / fixtures fetch)",
                "note": "Bodies stored exactly as received. sha256 is over the stored file bytes.",
                "files": {},
            }

    def try_providers(self, path, accept="application/json", want=None):
        """First provider that answers 200 wins. Returns (provider_id, url, status, headers, body)."""
        errs = []
        for pid, base in self.cfg["providers"]:
            if want and pid not in want:
                continue
            url = base + path
            for attempt in range(3):
                try:
                    st, hd, body = get(url, accept)
                    if st == 200 and body:
                        return pid, url, st, hd, body
                    errs.append(f"{pid}: HTTP {st}")
                    break
                except urllib.error.HTTPError as e:
                    errs.append(f"{pid}: HTTP {e.code}")
                    break
                except Exception as e:  # timeouts, resets: retry
                    errs.append(f"{pid}: {type(e).__name__}: {e}")
                    time.sleep(2 + 3 * attempt)
        raise RuntimeError(f"all providers failed for {path}: {errs}")

    def cross_check(self, path, body, used_pid):
        """Fetch the same JSON route from one other provider and compare after normalising hex case."""
        mine = normalise(json.loads(body))
        for pid, base in self.cfg["providers"]:
            if pid == used_pid:
                continue
            try:
                st, _, other = get(base + path)
                if st != 200:
                    continue
                return {"provider": pid, "equal_after_hex_lowercase": normalise(json.loads(other)) == mine}
            except Exception as e:
                last = f"{type(e).__name__}"
                continue
        return {"provider": None, "equal_after_hex_lowercase": None, "note": "no second provider answered"}

    def cross_check_ssz(self, path, body, used_pid):
        """Fetch the same route as SSZ from one other provider and compare bytes."""
        for pid, base in self.cfg["providers"]:
            if pid == used_pid:
                continue
            try:
                st, _, other = get(base + path, "application/octet-stream")
                if st == 200:
                    return {"provider": pid, "bytes_equal": other == body}
            except Exception:
                continue
        return {"provider": None, "bytes_equal": None, "note": "no second provider answered"}

    def save(self, name, path, accept="application/json", cross=True, want=None, meta=None):
        pid, url, st, hd, body = self.try_providers(path, accept, want)
        fp = os.path.join(self.dir, name)
        with open(fp, "wb") as f:
            f.write(body)
        entry = {
            "url": url,
            "provider": pid,
            "fetched_at_utc": now_utc(),
            "http_status": st,
            "accept": accept,
            "content_type": hd.get("content-type"),
            "eth_consensus_version": hd.get("eth-consensus-version"),
            "bytes": len(body),
            "sha256": hashlib.sha256(body).hexdigest(),
        }
        if meta:
            entry.update(meta)
        if cross and accept == "application/json":
            entry["cross_check"] = self.cross_check(path, body, pid)
        self.manifest["files"][name] = entry
        self.write_manifest()
        print(f"  {name}: {len(body)} bytes from {pid} sha256={entry['sha256'][:16]}..", file=sys.stderr)
        return body

    def save_pair(self, base, path, live, meta=None, name_from=None):
        """JSON and SSZ of the same route from the same provider. For live routes (finality / optimistic update)
        the SSZ is fetched before and after the JSON and accepted only if both SSZ bodies are identical, so the JSON
        and the SSZ describe the same server state. Returns (json_obj, final_base_name)."""
        errs = []
        for pid, _ in self.cfg["providers"]:
            for attempt in range(6 if live else 1):
                try:
                    _, url_s, st_s, hd_s, ssz1 = self.try_providers(path, "application/octet-stream", want=[pid])
                    _, url_j, st_j, hd_j, js = self.try_providers(path, "application/json", want=[pid])
                    if live:
                        _, _, _, _, ssz2 = self.try_providers(path, "application/octet-stream", want=[pid])
                        if ssz1 != ssz2:
                            errs.append(f"{pid}: head moved during fetch, retry")
                            time.sleep(5)
                            continue
                except RuntimeError as e:
                    errs.append(str(e))
                    break
                obj = json.loads(js)
                name = name_from(obj) if name_from else base
                at = now_utc()
                for ext, body, url, st, hd, acc in ((".json", js, url_j, st_j, hd_j, "application/json"),
                                                    (".ssz", ssz1, url_s, st_s, hd_s, "application/octet-stream")):
                    with open(os.path.join(self.dir, name + ext), "wb") as f:
                        f.write(body)
                    entry = {"url": url, "provider": pid, "fetched_at_utc": at, "http_status": st, "accept": acc,
                             "content_type": hd.get("content-type"),
                             "eth_consensus_version": hd.get("eth-consensus-version"),
                             "bytes": len(body), "sha256": hashlib.sha256(body).hexdigest(),
                             "pair": name + (".ssz" if ext == ".json" else ".json")}
                    if live:
                        entry["pair_check"] = "ssz fetched before and after the json, identical"
                    if meta:
                        entry.update(meta(obj) if callable(meta) else meta)
                    if ext == ".json" and not live:
                        entry["cross_check"] = self.cross_check(path, js, pid)
                    if ext == ".ssz" and not live:
                        entry["cross_check"] = self.cross_check_ssz(path, ssz1, pid)
                    self.manifest["files"][name + ext] = entry
                self.write_manifest()
                print(f"  {name}.json+.ssz: {len(js)}/{len(ssz1)} bytes from {pid}", file=sys.stderr)
                return obj, name
        raise RuntimeError(f"pair fetch failed for {path}: {errs}")

    def exec_headers(self, block_hashes):
        """gloas: fetch eth_getBlockByHash for each hash, rebuild the RLP header, keep it only if keccak matches."""
        d = os.path.join(self.dir, "exec_headers")
        os.makedirs(d, exist_ok=True)
        for bh, slot in sorted(block_hashes.items(), key=lambda x: x[1]):
            name = f"exec_headers/{bh}"
            if name + ".rlp" in self.manifest["files"]:
                continue
            body = None
            for pid, url in self.cfg["exec_rpcs"]:
                req = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "eth_getBlockByHash",
                                  "params": [bh, False]}).encode()
                try:
                    r = urllib.request.Request(url, data=req, headers={"content-type": "application/json",
                                                                       "User-Agent": UA})
                    with urllib.request.urlopen(r, timeout=60) as resp:
                        body = resp.read()
                    if json.loads(body).get("result"):
                        break
                    body = None
                except Exception as e:
                    print(f"  {pid}: {type(e).__name__}", file=sys.stderr)
            if body is None:
                raise RuntimeError(f"no execution RPC returned block {bh}")
            blk = json.loads(body)["result"]
            rlp = header_rlp(blk)
            k = keccak_hex(rlp)
            if k != bh.lower():
                raise RuntimeError(f"rebuilt header RLP does not hash to {bh} (got {k}); field order changed?")
            at = now_utc()
            with open(os.path.join(self.dir, name + ".json"), "wb") as f:
                f.write(body)
            with open(os.path.join(self.dir, name + ".rlp"), "wb") as f:
                f.write(rlp)
            common = {"provider": pid, "fetched_at_utc": at, "beacon_slot": slot, "block_hash": bh,
                      "block_number": int(blk["number"], 16)}
            self.manifest["files"][name + ".json"] = dict(common, url=url, method="eth_getBlockByHash(hash,false)",
                                                          bytes=len(body), sha256=hashlib.sha256(body).hexdigest())
            self.manifest["files"][name + ".rlp"] = dict(common, derived_from=name + ".json",
                                                         keccak256_equals_block_hash=True,
                                                         field_order=HEADER_FIELDS,
                                                         bytes=len(rlp), sha256=hashlib.sha256(rlp).hexdigest())
            self.write_manifest()
            print(f"  {name}.rlp: block {common['block_number']} slot {slot} keccak ok", file=sys.stderr)

    def write_manifest(self):
        self.manifest["updated_at_utc"] = now_utc()
        tmp = self.manifest_path + ".tmp"
        with open(tmp, "w") as f:
            json.dump(self.manifest, f, indent=2, sort_keys=True)
            f.write("\n")
        os.replace(tmp, self.manifest_path)


def _upd_meta(obj):
    d = obj["data"]
    m = {"attested_slot": int(d["attested_header"]["beacon"]["slot"]), "signature_slot": int(d["signature_slot"]),
         "participants": bin(int(d["sync_aggregate"]["sync_committee_bits"], 16)).count("1")}
    if "finalized_header" in d:
        m["finalized_slot"] = int(d["finalized_header"]["beacon"]["slot"])
    return m


def _gloas_hashes(f):
    """execution_block_hash of every light-client header in the stored JSON fixtures (gloas headers only)."""
    out = {}

    def walk(o):
        if isinstance(o, dict):
            if "beacon" in o and "execution_block_hash" in o:
                out[o["execution_block_hash"].lower()] = int(o["beacon"]["slot"])
            for v in o.values():
                walk(v)
        elif isinstance(o, list):
            for v in o:
                walk(v)

    for name in f.manifest["files"]:
        if name.endswith(".json") and not name.startswith("exec_headers/"):
            with open(os.path.join(f.dir, name)) as fh:
                walk(json.load(fh))
    return out


def run(chain, outdir, periods, finality_only):
    f = Fetcher(chain, outdir)
    print(f"[{chain}] fetching into {f.dir}", file=sys.stderr)

    lc = "/eth/v1/beacon/light_client/"
    fu, _ = f.save_pair(None, lc + "finality_update", live=True, meta=_upd_meta,
                        name_from=lambda o: f"finality_update_{o['data']['attested_header']['beacon']['slot']}")
    f.save_pair(None, lc + "optimistic_update", live=True, meta=_upd_meta,
                name_from=lambda o: f"optimistic_update_{o['data']['attested_header']['beacon']['slot']}")
    att = int(fu["data"]["attested_header"]["beacon"]["slot"])

    if not finality_only:
        cur_period = att // SLOTS_PER_PERIOD
        p0 = cur_period - (periods - 1)
        print(f"[{chain}] current period {cur_period}; updates_by_range {p0}..{cur_period}", file=sys.stderr)

        for n, route, cross in (("genesis.json", "/eth/v1/beacon/genesis", True),
                                ("fork_schedule.json", "/eth/v1/config/fork_schedule", True),
                                ("spec.json", "/eth/v1/config/spec", False)):
            if n not in f.manifest["files"]:
                f.save(n, route, cross=cross)

        base = f"updates_{p0}_{periods}"
        if base + ".json" in f.manifest["files"]:
            print(f"  {base} already present, kept (the current period's update may have changed upstream)",
                  file=sys.stderr)
        else:
            rng = f"{lc}updates?start_period={p0}&count={periods}"
            ub = json.loads(f.save(base + ".json", rng, meta={
                "start_period": p0, "count": periods,
                "note": "the last period is the current one; its best update can still improve"}))
            f.manifest["files"][base + ".json"]["updates"] = [_upd_meta(u) for u in ub]
            f.save(base + ".ssz", rng, accept="application/octet-stream", cross=False,
                   want=[f.manifest["files"][base + ".json"]["provider"]],
                   meta={"start_period": p0, "count": periods, "pair": base + ".json",
                         "format": "Beacon API SSZ list response: per item u64-le length (fork digest + payload), "
                                   "4-byte fork digest, SSZ LightClientUpdate"})

        # Bootstrap at the first epoch boundary of the first range period that has a block (JSON + SSZ pair).
        existing = [n for n in f.manifest["files"] if n.startswith("bootstrap_") and n.endswith(".json")]
        targets = []
        if existing:
            for n in existing:
                e = f.manifest["files"][n]
                if n[:-5] + ".ssz" not in f.manifest["files"]:
                    targets.append((e["slot"], e["block_root"], e.get("block_root_from"), e["sha256"]))
        else:
            start_slot = p0 * SLOTS_PER_PERIOD
            for k in range(0, 64):
                slot = start_slot + k * SLOTS_PER_EPOCH
                try:
                    _, url, _, _, hb = f.try_providers(f"/eth/v1/beacon/headers/{slot}")
                except RuntimeError:
                    continue
                h = json.loads(hb)["data"]
                if int(h["header"]["message"]["slot"]) == slot:
                    targets.append((slot, h["root"], url, None))
                    break
            else:
                raise RuntimeError("no block at an epoch boundary in the first 64 epochs of the period")
        for slot, root, url, old_sha in targets:
            f.save_pair(f"bootstrap_{slot}", f"{lc}bootstrap/{root}", live=False,
                        meta={"slot": slot, "block_root": root, "period": slot // SLOTS_PER_PERIOD,
                              "block_root_from": url})
            new_sha = f.manifest["files"][f"bootstrap_{slot}.json"]["sha256"]
            if old_sha and old_sha != new_sha:
                print(f"  WARNING bootstrap_{slot}.json changed on refetch ({old_sha[:12]} -> {new_sha[:12]})",
                      file=sys.stderr)

    if "exec_rpcs" in f.cfg:
        hashes = _gloas_hashes(f)
        print(f"[{chain}] gloas execution headers to fetch: {len(hashes)}", file=sys.stderr)
        f.exec_headers(hashes)


def fetch_bootstrap_at(chain, outdir, slot):
    """Bootstrap (JSON + SSZ pair, both cross-checked) for the block at `slot`, which must be an epoch boundary."""
    f = Fetcher(chain, outdir)
    if slot % SLOTS_PER_EPOCH:
        raise SystemExit(f"slot {slot} is not an epoch boundary")
    _, url, _, _, hb = f.try_providers(f"/eth/v1/beacon/headers/{slot}")
    h = json.loads(hb)["data"]
    if int(h["header"]["message"]["slot"]) != slot:
        raise SystemExit(f"no block at slot {slot}")
    root = h["root"]
    f.save_pair(f"bootstrap_{slot}", f"/eth/v1/beacon/light_client/bootstrap/{root}", live=False,
                meta={"slot": slot, "block_root": root, "period": slot // SLOTS_PER_PERIOD, "block_root_from": url})


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--chain", required=True, choices=sorted(CHAINS))
    ap.add_argument("--out", default="fixtures")
    ap.add_argument("--periods", type=int, default=4, help="adjacent periods for updates_by_range (>=2)")
    ap.add_argument("--finality-only", action="store_true", help="only add the latest finality/optimistic update")
    ap.add_argument("--bootstrap-slot", type=int, help="only fetch the bootstrap of the block at this epoch-boundary slot")
    a = ap.parse_args()
    if a.bootstrap_slot is not None:
        fetch_bootstrap_at(a.chain, a.out, a.bootstrap_slot)
        return
    if a.periods < 2:
        ap.error("--periods must be >= 2 to contain a committee handoff")
    run(a.chain, a.out, a.periods, a.finality_only)


if __name__ == "__main__":
    main()
