# Public read-only endpoint for seq-pub

- URL: `https://beacon-veil-seq.tailcdec7a.ts.net` (JSON-RPC over HTTPS, `POST /`; `GET /` prints the method list
  and the limits).
- Sequencer behind it: `seq-pub`, LEZ v0.4.0-rc3, a standalone sequencer with dev mode unset, the one that holds every
  transaction in `evidence/public-path/onchain/`. Not a Logos network.
- Code: `tools/seq_read/` (`policy.py` is the allow-list and the limits; `README.md` explains the two parts and how
  to start and stop them).
- Made public: 2026-11-20 11:44 UTC. First tested 11:49 to 11:52 UTC; tested again after a fix (non-finite numbers
  refused, visitors keyed by address, fixed refusal counters) at 14:46 to 14:48 UTC. The files below are from the
  second run.
- The endpoint is switched off during proof windows, when proofs run on the same machine.

## Files

| File | What |
|---|---|
| `verify-20261120T144800Z.json` | `tools/seq_read/verify.sh URL --public`: tests 1, 2, 3, 4 and 7 below, through the internet and Funnel |
| `check-through-endpoint.txt` | test 6: `v3_onchain check` re-reads the recorded transactions through the URL |

## Tests

| # | Test | Result | Where |
|---|---|---|---|
| 1 | `sendTransaction` and `requeueCrossZoneDeadLetter` refused with `-32601`; the host dialer's forwarded counter for both (and for the 5 refused reads) is 0 after the whole run | PASS | `verify-*.json` `1_writes_refused` |
| 2 | Refused, each with its code: `getPublications`, `getMessagePath`, `getProofsAndRoot`, `getRecoveryBinding`, `getCrossZoneDeadLetters`, an unknown method and `rpc.discover` (`-32601`); a batch of two (`-32600`); a 3 KiB body (HTTP 413); `getBlockRange` over 17 blocks (`-32602`); a non-JSON body (`-32700`); a `GET` with a path (HTTP 404) | PASS | `2_refusals` |
| 3 | Through the URL and directly from the sequencer, byte for byte equal: `getBlock 355`, `getTransaction a018aa6b...ddc2` (the first step of `onchain/mainnet/run.json`), `getAccountView` of the mainnet instance, `getProgramIds`; `getLastBlockId` through the URL not lower than the direct value read before | PASS | `3_reads_equal_direct` (sizes and sha256 of each answer) |
| 4 | 60 requests sent on a 2-second schedule: 39 answered 200, 21 answered 429; 5 s later a request answered 200 | PASS | `4_rate_limit` |
| 5 | `python3 -I tools/seq_read/test_policy.py`: 27 tests, among them a forged relay that hands the host dialer `sendTransaction`, `requeueCrossZoneDeadLetter`, a batch, a 1,000-block range, an oversized body, a refused read and a smuggled key: only the one allowed read reached the (fake) sequencer; a number such as `1e999` refused with `-32602`; the visitor key parsed as an address (IPv6 by its /64); the dialer's refusal counters keyed by a fixed set of names; every limit has a case | PASS (27 of 27, run 2026-11-20 14:45 UTC) | `tools/seq_read/test_policy.py` |
| 6 | `v3_onchain check` on `onchain/{mainnet,sepolia,hoodi}/run.json` with a wallet whose `sequencer_addr` is the URL: "PASS: 27 transactions re-read from the sequencer match the records"; reads only, a new empty wallet, no transaction | PASS | `check-through-endpoint.txt` |
| 7 | Memory: relay container 45.52 MiB (cap 128 MB; limit for the test 64 MiB), HTTPS sidecar 26.08 MiB, host dialer RSS 20.2 MiB (limit 30 MiB); `seq-pub` (pid 1386665) has the same start time before and after | PASS | `7_memory_and_sequencer` |

## Notes

- **Route of test 6.** The machine that runs the sequencer resolves the endpoint's name to its tailnet address, and
  `v3_onchain` uses the system resolver. So test 6 reached the endpoint's HTTPS sidecar over the tailnet, then the
  same relay and host dialer that public visitors reach; only the Funnel hop is not in its path. Tests 1 to 4 and 7
  (`verify.sh --public`) connect to the address the public DNS gives for the name, so they went through the internet
  and Funnel.
- **Test 4 timing.** Through Funnel a request takes about 0.6 s and one client gets about 20 open connections, so
  `verify.sh` opens 15 connections first and sends 4 requests on each on a 2-second schedule; `seconds` in the JSON
  (4.55) runs from the first send to the last answer, including the up-to-1-second slowdown a visitor a little over
  the rate gets. A first public run that opened a new connection per request spread the 60 requests over 9.8 s; at
  that pace all 60 were within the limit (no 429), and the script was changed as described.
- **Visitors are limited one by one.** Manual check, not recorded in a file: during a flood from the public route,
  5 requests from the tailnet route all answered 200, so the limit keys on the visitor's address, not on the sidecar.
- The JSON records the sha256 of `tools/seq_read/policy.py` and `relay.py` in the repository when the test ran
  (`policy_sha256_repo`, `relay_sha256_repo`) and of the copies in the running relay container (`..._deployed`); they
  are equal.
