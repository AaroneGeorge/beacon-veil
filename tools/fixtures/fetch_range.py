#!/usr/bin/env python3
"""Fetch light-client fixtures for the differential test: explicit period ranges, Hoodi (Fulu), repeated finality polls.

Stdlib only; builds on `fetch.py` (same directory: Fetcher, manifest schema, cross-provider checks).
Run with `python3 -I tools/fixtures/fetch_range.py --chain mainnet|sepolia|hoodi --out DIR ...`.

What it writes into <out>/<chain>/:
  genesis.json, fork_schedule.json, spec.json          network constants (cross-checked against a second provider)
  updates_<P0>_<n>.json + .ssz                         `updates_by_range` for each --range P0:n (SSZ cross-checked byte
                                                       for byte against a second provider)
  finality_update_<attested_slot>.json + .ssz          one per poll (--polls N, --interval S seconds apart)
  optimistic_update_<attested_slot>.json + .ssz        one per poll
  manifest.json                                        URL, provider, UTC time, sha256, bytes, consensus version,
                                                       per-update slots and participation, cross-checks

Requests are sequential with a pause between them (--pause, default 1 s). Bodies are stored exactly as received.
"""

import argparse
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fetch  # noqa: E402  (same directory)

fetch.CHAINS.setdefault("hoodi", {
    "chain_id": 560_048,
    "genesis_time": 1_742_213_400,
    "providers": [
        ("lodestar", "https://lodestar-hoodi.chainsafe.io"),
        ("nimbus-unstable", "http://unstable.hoodi.beacon-api.nimbus.team"),
        ("nimbus-testing", "http://testing.hoodi.beacon-api.nimbus.team"),
    ],
})

LC = "/eth/v1/beacon/light_client/"
PAUSE = 1.0

_orig_get = fetch.get


def polite_get(url, accept="application/json", timeout=90):
    time.sleep(PAUSE)
    return _orig_get(url, accept, timeout)


fetch.get = polite_get


class RangeFetcher(fetch.Fetcher):
    """Cross-checks skip a provider that answers an empty list (seen: nimbus-unstable on Hoodi (Fulu) `updates`) and try
    the next one, so the recorded check is against a provider that actually served the data."""

    def cross_check(self, path, body, used_pid):
        mine = fetch.normalise(json.loads(body))
        skipped = []
        for pid, base in self.cfg["providers"]:
            if pid == used_pid:
                continue
            try:
                st, _, other = fetch.get(base + path)
            except Exception as e:
                skipped.append(f"{pid}: {type(e).__name__}")
                continue
            if st != 200 or json.loads(other) in ([], {}):
                skipped.append(f"{pid}: empty answer")
                continue
            out = {"provider": pid, "equal_after_hex_lowercase": fetch.normalise(json.loads(other)) == mine}
            if skipped:
                out["skipped"] = skipped
            return out
        return {"provider": None, "equal_after_hex_lowercase": None, "note": "no second provider answered",
                "skipped": skipped}

    def cross_check_ssz(self, path, body, used_pid):
        skipped = []
        for pid, base in self.cfg["providers"]:
            if pid == used_pid:
                continue
            try:
                st, _, other = fetch.get(base + path, "application/octet-stream")
            except Exception as e:
                skipped.append(f"{pid}: {type(e).__name__}")
                continue
            if st != 200 or not other:
                skipped.append(f"{pid}: empty answer")
                continue
            out = {"provider": pid, "bytes_equal": other == body}
            if skipped:
                out["skipped"] = skipped
            return out
        return {"provider": None, "bytes_equal": None, "note": "no second provider answered", "skipped": skipped}


def constants(f):
    for n, route, cross in (("genesis.json", "/eth/v1/beacon/genesis", True),
                            ("fork_schedule.json", "/eth/v1/config/fork_schedule", True),
                            ("spec.json", "/eth/v1/config/spec", False)):
        if n not in f.manifest["files"]:
            f.save(n, route, cross=cross)


def fetch_range(f, p0, count, note):
    base = f"updates_{p0}_{count}"
    if base + ".ssz" in f.manifest["files"]:
        print(f"  {base} already present, kept", file=sys.stderr)
        return
    rng = f"{LC}updates?start_period={p0}&count={count}"
    ub = json.loads(f.save(base + ".json", rng, meta={"start_period": p0, "count": count, "note": note}))
    if len(ub) != count:
        raise RuntimeError(f"{base}: asked for {count} updates, got {len(ub)}")
    f.manifest["files"][base + ".json"]["updates"] = [dict(fetch._upd_meta(u), version=u.get("version")) for u in ub]
    pid = f.manifest["files"][base + ".json"]["provider"]
    body = f.save(base + ".ssz", rng, accept="application/octet-stream", cross=False, want=[pid],
                  meta={"start_period": p0, "count": count, "pair": base + ".json",
                        "format": "Beacon API SSZ list response: per item u64-le length (fork digest + payload), "
                                  "4-byte fork digest, SSZ LightClientUpdate"})
    f.manifest["files"][base + ".ssz"]["cross_check"] = f.cross_check_ssz(rng, body, pid)
    f.write_manifest()


def recheck_ranges(f):
    """Repeats the cross-provider checks of every stored range (the bodies on disk are not refetched)."""
    for name, e in sorted(f.manifest["files"].items()):
        if not name.startswith("updates_"):
            continue
        rng = f"{LC}updates?start_period={e['start_period']}&count={e['count']}"
        with open(os.path.join(f.dir, name), "rb") as fh:
            body = fh.read()
        if name.endswith(".json"):
            e["cross_check"] = f.cross_check(rng, body, e["provider"])
        else:
            e["cross_check"] = f.cross_check_ssz(rng, body, e["provider"])
        e["cross_check"]["checked_at_utc"] = fetch.now_utc()
        print(f"  {name}: {e['cross_check']}", file=sys.stderr)
    f.write_manifest()


def poll(f, polls, interval):
    for i in range(polls):
        if i:
            print(f"  sleeping {interval}s before poll {i + 1}/{polls}", file=sys.stderr)
            time.sleep(interval)
        f.save_pair(None, LC + "finality_update", live=True, meta=fetch._upd_meta,
                    name_from=lambda o: f"finality_update_{o['data']['attested_header']['beacon']['slot']}")
        f.save_pair(None, LC + "optimistic_update", live=True, meta=fetch._upd_meta,
                    name_from=lambda o: f"optimistic_update_{o['data']['attested_header']['beacon']['slot']}")


def main():
    global PAUSE
    ap = argparse.ArgumentParser()
    ap.add_argument("--chain", required=True, choices=sorted(fetch.CHAINS))
    ap.add_argument("--out", required=True)
    ap.add_argument("--range", action="append", default=[], metavar="P0:N[:NOTE]",
                    help="updates_by_range start period and count (repeatable)")
    ap.add_argument("--polls", type=int, default=0, help="finality + optimistic update polls")
    ap.add_argument("--interval", type=int, default=420, help="seconds between polls")
    ap.add_argument("--recheck-ranges", action="store_true", help="repeat the cross-checks of stored ranges")
    ap.add_argument("--pause", type=float, default=1.0, help="seconds before every request")
    a = ap.parse_args()
    PAUSE = a.pause
    f = RangeFetcher(a.chain, a.out)
    f.manifest.setdefault("tool", "tools/fixtures/fetch_range.py (differential test)")
    print(f"[{a.chain}] fetching into {f.dir}", file=sys.stderr)
    constants(f)
    for r in a.range:
        parts = r.split(":", 2)
        p0, n = int(parts[0]), int(parts[1])
        note = parts[2] if len(parts) > 2 else "period range"
        print(f"[{a.chain}] updates_by_range {p0}..{p0 + n - 1}", file=sys.stderr)
        fetch_range(f, p0, n, note)
    if a.recheck_ranges:
        recheck_ranges(f)
    if a.polls:
        poll(f, a.polls, a.interval)
    print(f"[{a.chain}] done", file=sys.stderr)


if __name__ == "__main__":
    main()
