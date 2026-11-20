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

Run `relay.py` as a container app (this `Dockerfile`) behind a TLS front that forwards HTTPS to its port 8080 and
sets `X-Forwarded-For`; give it `RELAY_TOKEN` (at least 32 characters, for example
`head -c 32 /dev/urandom | xxd -p -c 64`) in its settings. Port 8081 must stay unpublished: only the sequencer's host
opens connections to it. Then run `dialer.py` on the sequencer's host, with the same token in a file of mode 600,
best in its own cgroup so a memory kill stays inside it:

```
env -i PATH=/usr/bin:/bin SEQ_URL=http://127.0.0.1:3043 RELAY_APP=<app name> \
  RELAY_TOKEN_FILE=<token file> python3 -I tools/seq_read/dialer.py
```

`RELAY_APP` names the container (`<app name>.web.1`, address read with `docker inspect` at each reconnect);
`RELAY_URL=http://HOST:8081` gives the relay's address directly instead. Stop the dialer with SIGTERM (it writes its
counters and exits) and stop the app with the hosting platform. The relay answers 503 while the dialer is away; the
dialer reconnects by itself when the relay restarts. Both are switched off during proof windows on the machine that
runs `seq-pub`.

## Test

```
python3 -I tools/seq_read/test_policy.py                       # unit and in-process tests
tools/seq_read/verify.sh https://beacon-veil-seq.tailcdec7a.ts.net --public
```

`verify.sh` checks: the two writes refused and never forwarded (the dialer's counters), each refusal with its code,
four reads equal to the direct answer byte for byte, the rate limit, and memory (relay under 64 MiB, dialer under
30 MiB) with `seq-pub`'s start time unchanged. It records the sha256 of `policy.py` and `relay.py` in the repository
and in the running container. `--public` connects to the address the public DNS gives, so the
requests go through Funnel even from a machine of the tailnet.
