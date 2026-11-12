#!/usr/bin/env bash
# Compares the image ids of a guests_release.sh output dir with guests/IMAGE_IDS.expected (CI reproducibility check).
#
#   scripts/check_image_ids.sh <release_dir> [<expected_file>]
#
# Every guest listed in the expected file must be present with the same id. Guests not listed (burn_proof) are ignored.
set -euo pipefail
dir="${1:?release dir}"; exp="${2:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/guests/IMAGE_IDS.expected}"
got="$(sed -n 's/^\([a-z0-9_]*\)\.bin: .* image id \([0-9a-f]\{64\}\)$/\1 \2/p' "$dir/IMAGE_IDS")"
fail=0
while read -r name id; do
  case "$name" in ''|\#*) continue ;; esac
  have="$(awk -v n="$name" '$1==n{print $2}' <<<"$got")"
  if [ "$have" = "$id" ]; then echo "OK       $name $id"
  else echo "MISMATCH $name expected $id got ${have:-<missing>}"; fail=1; fi
done < "$exp"
exit $fail
