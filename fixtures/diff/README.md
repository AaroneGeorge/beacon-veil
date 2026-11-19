# Fixtures of the differential test (`tests/diff_helios`)

Fetched once on 2026-11-19 (UTC) from public Beacon API light-client endpoints, stored byte for byte as received.
Each network's `manifest.json` lists every file with its URL, provider, fetch time, HTTP status, consensus version,
size, SHA-256 and a cross-check against a second provider (JSON compared after lower-casing hex, SSZ compared byte
for byte). The test checks every SHA-256 before use.

| Network | Periods (`updates_by_range`) | Live updates | Second provider |
|---|---|---|---|
| mainnet | 1904-1915 (12) | 5 finality, 5 optimistic | nimbus-unstable: equal |
| sepolia | 1376-1380 (Fulu to Gloas crossing at period 1379, epoch 353024) and 1406-1417 (Gloas) | 5 finality, 5 optimistic | nimbus-unstable: equal |
| hoodi (Fulu) | 530-538 | 3 finality, 3 optimistic | nimbus-testing: equal (nimbus-unstable answered an empty list) |

Hoodi (Fulu) has no Gloas fork scheduled at fetch time (`fork_schedule.json` and `spec.json` of both providers:
`GLOAS_FORK_EPOCH` = 18446744073709551615), so the fork crossing in the test is Sepolia's.

The last period of each range was the current one when fetched: its best update could still improve upstream, so a
refetch may differ. The test uses the stored bytes.

Fetch command (stdlib Python, providers and politeness as `tools/fixtures/fetch.py`; one request per second):

```
python3 -I tools/fixtures/fetch_range.py --chain mainnet --out fixtures/diff --range 1904:12 --polls 5 --interval 420
python3 -I tools/fixtures/fetch_range.py --chain sepolia --out fixtures/diff --range 1376:5 --range 1406:12 --polls 5 --interval 420
python3 -I tools/fixtures/fetch_range.py --chain hoodi   --out fixtures/diff --range 530:9 --polls 3 --interval 420
python3 -I tools/fixtures/fetch_range.py --chain hoodi   --out fixtures/diff --recheck-ranges
```
