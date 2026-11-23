#!/usr/bin/env bash
# Shows that verify.sh catches a changed record: on a copy of the evidence, four changes in turn (a transaction
# hash, a block number, the last byte of the instance state, an image id); each must make verify.sh fail.
#   lez-v03/scripts/mutation_test.sh [--rpc URL] [EVIDENCE_DIR]
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
rpc=https://testnet.lez.logos.co
if [ "${1:-}" = "--rpc" ]; then rpc="$2"; shift 2; fi
ev="$(cd "${1:-$here/../../evidence/public-testnet}" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
fails=0
mutate() { # name, python snippet run on the copy (cwd = copy)
  rm -rf "$tmp/ev" && cp -r "$ev" "$tmp/ev"
  (cd "$tmp/ev" && python3 -I -c "$2")
  if "$here/verify.sh" --rpc "$rpc" "$tmp/ev" > "$tmp/out.txt" 2>&1; then
    echo "NOT CAUGHT  $1"; fails=$((fails + 1))
  else
    echo "caught      $1 ($(grep -c '^FAIL' "$tmp/out.txt") failed checks)"
  fi
}
mutate "a changed transaction hash" '
import os
l = open("txs.tsv").read().splitlines(); f = l[1].split("\t"); old = f[1]
f[1] = old[:-1] + ("0" if old[-1] != "0" else "1"); l[1] = "\t".join(f)
open("txs.tsv", "w").write("\n".join(l) + "\n")
os.rename(f"raw/{old}.tx.b64", f"raw/{f[1]}.tx.b64")'
mutate "a changed block number" '
l = open("txs.tsv").read().splitlines(); f = l[-1].split("\t"); f[2] = str(int(f[2]) + 1); l[-1] = "\t".join(f)
open("txs.tsv", "w").write("\n".join(l) + "\n")'
mutate "a changed final byte of the instance state" '
import json
r = json.load(open("run.json")); h = r["instance_state_hex"]
r["instance_state_hex"] = h[:-2] + format(int(h[-2:], 16) ^ 1, "02x")
json.dump(r, open("run.json", "w"), indent=2)'
mutate "a changed image id" '
import json
d = json.load(open("deployment.json")); p = d["programs"]["lc_verify_v3_v03"]; i = p["image_id"]
p["image_id"] = i[:-1] + ("0" if i[-1] != "0" else "1")
json.dump(d, open("deployment.json", "w"), indent=2)
l = [x.split() for x in open("IMAGE_IDS") if x.strip()]
new = p["image_id"]
open("IMAGE_IDS", "w").write("".join(n + " " + (new if n == "lc_verify_v3_v03" else v) + "\n" for n, v in l))'
echo "$((4 - fails)) of 4 changes caught"
[ "$fails" -eq 0 ]
