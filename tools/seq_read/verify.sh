#!/usr/bin/env bash
# Checks the read-only sequencer endpoint from the outside and writes the result as evidence.
#
#   tools/seq_read/verify.sh URL [--public]
#
# --public: connect to the address the public DNS (1.1.1.1) gives for URL's host, so the requests travel through the
# internet and Funnel even on a machine of the tailnet (TLS and Host stay URL's name).
# Runs on the sequencer's host: test 3 compares each answer with the direct one from SEQ_URL (default
# http://127.0.0.1:3043), test 1 reads the host dialer's counters (STATS_FILE, default $B/run/seq-read/stats.json),
# test 7 reads memory with docker stats and /proc. Sends only read calls to the sequencer, plus refused calls to URL.
# Writes evidence/public-path/endpoint/verify-<utc>.json and prints one PASS or FAIL line per test.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
[ $# -ge 1 ] || { sed -n '2,12p' "$0"; exit 2; }
url="$1"
connect_ip=""
if [ "${2:-}" = "--public" ]; then
  host="$(python3 -I -c 'import sys,urllib.parse; print(urllib.parse.urlsplit(sys.argv[1]).hostname)' "$url")"
  connect_ip="$(dig +short @1.1.1.1 "$host" A | grep -E '^[0-9.]+$' | head -1)"
  [ -n "$connect_ip" ] || { echo "no public address for $host" >&2; exit 1; }
fi
B="${BV_BUILD_DIR:-$(cd "$repo/.." && pwd)}"
export VERIFY_URL="$url" VERIFY_CONNECT_IP="$connect_ip" VERIFY_REPO="$repo" \
  SEQ_URL="${SEQ_URL:-http://127.0.0.1:3043}" STATS_FILE="${STATS_FILE:-$B/run/seq-read/stats.json}" \
  RELAY_APP="${RELAY_APP:-beacon-veil-seq}" SEQ_PID="${SEQ_PID:-$(cat "$B/run/seq-pub.pid" 2>/dev/null || true)}"
exec python3 -I - <<'PY'
import concurrent.futures
import hashlib
import http.client
import json
import os
import socket
import ssl
import subprocess
import sys
import time
import urllib.parse

URL = os.environ["VERIFY_URL"]
U = urllib.parse.urlsplit(URL)
CONNECT_IP = os.environ.get("VERIFY_CONNECT_IP") or None
REPO = os.environ["VERIFY_REPO"]
SEQ = urllib.parse.urlsplit(os.environ["SEQ_URL"])
sys.path.insert(0, os.path.join(REPO, "tools/seq_read"))
import policy  # noqa: E402


def now():
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def connect():
    """An open connection to URL (TLS done), so a burst of requests is not spread out by handshakes."""
    host = CONNECT_IP or U.hostname
    port = U.port or (443 if U.scheme == "https" else 80)
    if U.scheme == "https":
        ctx = ssl.create_default_context()
        conn = http.client.HTTPSConnection(host, port, timeout=30, context=ctx)
        if CONNECT_IP:  # TCP to the public address, TLS name and Host header from the URL
            raw = socket.create_connection((CONNECT_IP, port), timeout=30)
            conn.sock = ctx.wrap_socket(raw, server_hostname=U.hostname)
        else:
            conn.connect()
    else:
        conn = http.client.HTTPConnection(host, port, timeout=30)
        conn.connect()
    return conn


def endpoint(method, path="/", body=None, ctype="application/json", conn=None):
    """One request to URL (a new connection unless one is given); returns (status, body bytes)."""
    conn = conn or connect()
    try:
        h = {"Host": U.hostname}
        if body is not None:
            h["Content-Type"] = ctype
        conn.request(method, path, body=body, headers=h)
        r = conn.getresponse()
        return r.status, r.read()
    finally:
        conn.close()


def direct(body):
    conn = http.client.HTTPConnection(SEQ.hostname, SEQ.port, timeout=10)
    try:
        conn.request("POST", "/", body=body, headers={"Content-Type": "application/json"})
        r = conn.getresponse()
        return r.status, r.read()
    finally:
        conn.close()


def rpc(method, params, rid=1):
    return json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params}).encode()


def err_code(body):
    try:
        return json.loads(body)["error"]["code"]
    except (ValueError, KeyError, TypeError):
        return None


def start_time(pid):
    try:
        with open(f"/proc/{pid}/stat") as f:
            return int(f.read().rsplit(")", 1)[1].split()[19])
    except (OSError, ValueError, IndexError):
        return None


results = {}
report = {"url": URL, "via": f"public address {CONNECT_IP}" if CONNECT_IP else "system resolver",
          "started": now(), "tests": results}
with open(os.path.join(REPO, "tools/seq_read/policy.py"), "rb") as f:
    report["policy_sha256"] = hashlib.sha256(f.read()).hexdigest()
seq_pid = os.environ.get("SEQ_PID", "")
seq_start_before = start_time(seq_pid) if seq_pid else None

# 2. Refusals, each with its code.
cases = []
for m in policy.REFUSED_READS + ("noSuchMethod", "rpc.discover"):
    cases.append((f"method {m}", lambda m=m: endpoint("POST", "/", rpc(m, [])), 200, policy.METHOD_NOT_FOUND))
cases += [
    ("batch of two", lambda: endpoint("POST", "/", b"[" + rpc("getBlock", [1], 1) + b"," + rpc("getBlock", [2], 2)
                                      + b"]"), 200, policy.INVALID_REQUEST),
    ("3 KiB body", lambda: endpoint("POST", "/", rpc("getBlock", ["x" * 3072])), 413, None),
    ("getBlockRange over 17 blocks", lambda: endpoint("POST", "/", rpc("getBlockRange", [100, 116])), 200,
     policy.INVALID_PARAMS),
    ("non-JSON body", lambda: endpoint("POST", "/", b"this is not json"), 200, policy.PARSE_ERROR),
    ("GET with a path", lambda: endpoint("GET", "/rpc"), 404, None),
]
rows, ok2 = [], True
for name, fn, want_status, want_code in cases:
    status, body = fn()
    code = err_code(body)
    good = status == want_status and (want_code is None or code == want_code)
    ok2 &= good
    rows.append({"case": name, "http": status, "code": code, "want_http": want_status, "want_code": want_code,
                 "pass": good})
    time.sleep(0.25)
results["2_refusals"] = {"pass": ok2, "cases": rows}

# 1. The two writes are refused with -32601 (their counter at the host dialer is checked at the end).
rows, ok1 = [], True
for m in policy.WRITES:
    status, body = endpoint("POST", "/", rpc(m, [{"anything": 1}] if m == "sendTransaction" else ["00" * 32]))
    good = status == 200 and err_code(body) == policy.METHOD_NOT_FOUND
    ok1 &= good
    rows.append({"method": m, "http": status, "code": err_code(body), "pass": good})
    time.sleep(0.25)
results["1_writes_refused"] = {"pass": ok1, "cases": rows}

# 3. Allowed reads equal the direct answer, byte for byte.
run = json.load(open(os.path.join(REPO, "evidence/public-path/onchain/mainnet/run.json")))
step0 = run["updates"][0]["steps"][0]
reads = [
    ("getBlock", [step0["block"]]),
    ("getTransaction", [step0["tx"]]),
    ("getAccountView", [{"account_id": run["instance"], "program_account_id": run["programs"]["attest"]}]),
    ("getProgramIds", []),
]
_, last_direct = direct(rpc("getLastBlockId", []))
rows, ok3 = [], True
for m, p in reads:
    body = rpc(m, p, 3)
    d_status, d_body = direct(body)
    e_status, e_body = endpoint("POST", "/", body)
    same = (d_status, d_body) == (e_status, e_body) and e_status == 200 and b'"result"' in e_body
    ok3 &= same
    rows.append({"method": m, "params": p, "bytes": len(e_body), "sha256": hashlib.sha256(e_body).hexdigest(),
                 "equal_to_direct": same})
    time.sleep(0.25)
e_status, e_body = endpoint("POST", "/", rpc("getLastBlockId", []))
last_d, last_e = json.loads(last_direct)["result"], json.loads(e_body).get("result", -1)
ok3 &= e_status == 200 and last_e >= last_d
rows.append({"method": "getLastBlockId", "direct_before": last_d, "through_endpoint": last_e,
             "not_lower": last_e >= last_d})
results["3_reads_equal_direct"] = {"pass": ok3, "cases": rows, "step": {"tx": step0["tx"], "block": step0["block"]}}

# 4. 60 requests in about 2 seconds: some 429; 5 seconds later a request works.
time.sleep(5)
def connect_retry(_):
    for attempt in range(4):
        try:
            return connect()
        except (OSError, ssl.SSLError):
            time.sleep(0.5 * (attempt + 1))
    return connect()


def request_on(conn, body):
    conn.request("POST", "/", body=body, headers={"Host": U.hostname, "Content-Type": "application/json"})
    r = conn.getresponse()
    r.read()
    return r.status


# Funnel takes about 20 open connections from one client, and a request takes about 0.6 s through it: 15 kept-alive
# connections (opened first, so handshakes do not spread the burst) send 4 requests each on a 2-second schedule.
LANES, PER_LANE = 15, 4
with concurrent.futures.ThreadPoolExecutor(5) as ex:
    conns = list(ex.map(connect_retry, range(LANES)))
with concurrent.futures.ThreadPoolExecutor(LANES) as ex:
    t0 = time.monotonic()

    def lane(k):
        out = []
        for j in range(PER_LANE):
            i = k + LANES * j
            time.sleep(max(0.0, i * (2.0 / 60) - (time.monotonic() - t0)))
            out.append(request_on(conns[k], rpc("getLastBlockId", [], i)))
        conns[k].close()
        return out
    statuses = [st for lane_out in ex.map(lane, range(LANES)) for st in lane_out]
spent = time.monotonic() - t0
time.sleep(5)
after = endpoint("POST", "/", rpc("getLastBlockId", []))[0]
counts = {str(s): statuses.count(s) for s in sorted(set(statuses))}
results["4_rate_limit"] = {"pass": statuses.count(429) > 0 and after == 200, "statuses": counts,
                           "seconds": round(spent, 2), "after_5s": after}

# 1 (continued). The host dialer's counters: the writes never reached the sequencer.
time.sleep(6)  # the host dialer writes its counters every 5 s
try:
    stats = json.load(open(os.environ["STATS_FILE"]))
    fwd = stats.get("forwarded", {})
    counters = {m: fwd.get(m, 0) for m in policy.WRITES + policy.REFUSED_READS}
    results["1_writes_refused"]["forwarded_counter"] = counters
    results["1_writes_refused"]["dialer_forwarded_all"] = fwd
    results["1_writes_refused"]["pass"] = ok1 and not any(counters.values())
except (OSError, ValueError) as e:
    results["1_writes_refused"]["forwarded_counter"] = f"not readable: {e}"
    results["1_writes_refused"]["pass"] = False

# 7. Memory and seq-pub unchanged.
mem = {}
try:
    out = subprocess.run(["docker", "stats", "--no-stream", "--format", "{{.Name}}\t{{.MemUsage}}",
                          f"{os.environ['RELAY_APP']}.web.1", f"host-{os.environ['RELAY_APP']}-ts-1"],
                         capture_output=True, text=True, timeout=30, check=False).stdout
    for line in out.splitlines():
        name, use = line.split("\t", 1)
        mem[name] = use.split("/")[0].strip()
except (OSError, ValueError, subprocess.SubprocessError) as e:
    mem["docker"] = f"not readable: {e}"


def mib(s):
    s = s.strip()
    for unit, f in (("GiB", 1024), ("MiB", 1), ("KiB", 1 / 1024)):
        if s.endswith(unit):
            return float(s[:-len(unit)]) * f
    return None


dialer_rss = None
pg = subprocess.run(["pgrep", "-f", "tools/seq_read/dialer.py"], capture_output=True, text=True, check=False).stdout
for pid in pg.split():
    try:
        for line in open(f"/proc/{pid}/status"):
            if line.startswith("VmRSS:"):
                dialer_rss = round(int(line.split()[1]) / 1024, 1)
    except OSError:
        pass
relay_mib = mib(mem.get(f"{os.environ['RELAY_APP']}.web.1", ""))
seq_start_after = start_time(seq_pid) if seq_pid else None
results["7_memory_and_sequencer"] = {
    "pass": relay_mib is not None and relay_mib < 64 and dialer_rss is not None and dialer_rss < 30
    and seq_start_before is not None and seq_start_before == seq_start_after,
    "relay_container": mem.get(f"{os.environ['RELAY_APP']}.web.1"),
    "https_sidecar": mem.get(f"host-{os.environ['RELAY_APP']}-ts-1"),
    "dialer_rss_mib": dialer_rss,
    "seq_pub": {"pid": seq_pid, "start_ticks_before": seq_start_before, "start_ticks_after": seq_start_after},
}

report["finished"] = now()
report["pass"] = all(t["pass"] for t in results.values())
out_dir = os.path.join(REPO, "evidence/public-path/endpoint")
os.makedirs(out_dir, exist_ok=True)
path = os.path.join(out_dir, f"verify-{time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())}.json")
with open(path, "w") as f:
    json.dump(report, f, indent=1)
    f.write("\n")
for name in sorted(results):
    print(f"{'PASS' if results[name]['pass'] else 'FAIL'} {name}")
print(f"wrote {os.path.relpath(path, REPO)}")
sys.exit(0 if report["pass"] else 1)
PY
