# tools/seq_read: a public read-only endpoint for the sequencer

`seq-pub` (LEZ v0.4.0-rc3, dev mode unset) holds every transaction cited in `docs/PUBLIC-PATH.md`. Its JSON-RPC
listens on `127.0.0.1:3043` only and has no authentication of its own, including for `sendTransaction`. This folder
makes 12 of its read methods reachable from the internet and nothing else.

```
 visitor --HTTPS--> Tailscale Funnel --> HTTPS sidecar --> relay.py (hosted app, gVisor, port 8080: a queue)
                                                              ^ port 8081: GET /pull, POST /push, bearer token
                                                              | the host opens this connection
                                     dialer.py on the host ---+--> http://127.0.0.1:3043 (allow-listed reads only)
```

The hosting firewall refuses every new connection from an app container to the host, so the relay cannot call the
sequencer; the host dialer calls out to the relay instead. Nothing new listens on the host, and no firewall rule,
Tailscale policy or sequencer setting changes.

| File | What |
|---|---|
| `policy.py` | the allow-list and every limit; used by both halves |
| `relay.py` | the hosted app: checks each request, queues it, hands it to the host dialer, returns the reply |
| `dialer.py` | the host half: long-polls the relay, checks each request again, calls the sequencer, returns the reply |
| `Dockerfile` | the app image (`python:3.12-alpine` pinned by digest, user 65534, standard library only) |
| `test_policy.py` | unit and in-process tests (no network, no sequencer) |
| `verify.sh` | the live test against a URL; writes `evidence/public-path/endpoint/verify-<utc>.json` |

## Policy

- Forwarded: `getTransaction`, `getBlock`, `getBlockRange` (at most 16 blocks), `getLastBlockId`, `getAccount`,
  `getAccountView`, `getAccountBalance`, `getAccountsNonces` (at most 16 ids), `getProgramIds`, `getFeeState`,
  `getChannelId`, `checkHealth`.
- Refused with `-32601` "not available on this read-only endpoint": `sendTransaction`,
  `requeueCrossZoneDeadLetter`, `getPublications`, `getMessagePath`, `getProofsAndRoot`, `getRecoveryBinding`,
  `getCrossZoneDeadLetters`, `rpc.*`, any other name.
- One JSON object per request (a batch is `-32600`), only the keys `jsonrpc`, `id`, `method`, `params`, no duplicate
  keys, no notifications. The request is re-encoded before it reaches the sequencer, so the sequencer sees only what
  the policy checked.
- `POST /` only (plus `GET /` help and `GET /health`); 2 KiB per request (HTTP 413 above); replies at most 2 MiB.
- 5 requests a second per visitor, burst 20, 20 a second in total. A visitor a little over the rate is slowed by up
  to 1 s (a client re-reading many transactions one by one keeps working); more gets HTTP 429.
- At most 64 requests waiting, 4 sequencer calls at once, 5 s per sequencer call, 10 s per request (HTTP 504);
  HTTP 503 while the host dialer is away.

The host dialer checks every request again with the same `policy.py`, refuses to talk to a sequencer that is not on
loopback, and keeps per-method counters (`stats.json` next to the token file): a relay that is broken or taken over
can still ask only for allow-listed reads.

## Run

Paths: `B` is the build directory, `R=$B/repo`. The relay token lives only in `$B/run/seq-read/token` (mode 600) and
in the app's settings.

Start (relay app, then the host dialer in its own scope, so a memory kill stays inside it):

```
sudo lechu-hosting start beacon-veil-seq
cd $B && XDG_RUNTIME_DIR=/run/user/$(id -u) DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/bus \
  setsid nohup systemd-run --user --scope --collect --unit=bv-seq-read-dialer -p MemoryMax=64M -p CPUQuota=25% -- \
  env -i PATH=/usr/bin:/bin SEQ_URL=http://127.0.0.1:3043 RELAY_APP=beacon-veil-seq \
  RELAY_TOKEN_FILE=$B/run/seq-read/token python3 -I $R/tools/seq_read/dialer.py \
  > $B/run/seq-read/dialer.log 2>&1 < /dev/null &
```

Stop (both):

```
XDG_RUNTIME_DIR=/run/user/$(id -u) systemctl --user stop bv-seq-read-dialer.scope
sudo lechu-hosting stop beacon-veil-seq
```

The relay answers 503 while the host dialer is stopped; the dialer reconnects by itself when the relay restarts
(it reads the container's address with `docker inspect` at each reconnect).

## Test

```
python3 -I tools/seq_read/test_policy.py                       # unit and in-process tests
tools/seq_read/verify.sh https://beacon-veil-seq.tailcdec7a.ts.net --public
```

`verify.sh` checks: the two writes refused and never forwarded (the dialer's counters), each refusal with its code,
four reads equal to the direct answer byte for byte, the rate limit, and memory (relay under 64 MiB, dialer under
30 MiB) with `seq-pub`'s start time unchanged. `--public` connects to the address the public DNS gives, so the
requests go through Funnel even from a machine of the tailnet.
