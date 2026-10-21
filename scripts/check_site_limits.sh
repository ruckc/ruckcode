#!/usr/bin/env bash
# Fail clearly when a Pages site directory exceeds GitHub's limits: the published site must stay
# under 1 GB (we stop at 900 MB) and no single file may reach 100 MB.
# Usage: scripts/check_site_limits.sh <site-dir>
set -euo pipefail
dir="${1:?usage: check_site_limits.sh <site-dir>}"
max_total=900000000
max_file=100000000

total=$(find "$dir" -type f -printf '%s\n' | awk '{ s += $1 } END { print s + 0 }')
echo "Pages site: $(find "$dir" -type f | wc -l) files, $((total / 1000000)) MB (limit $((max_total / 1000000)) MB)"
find "$dir" -type f -printf '%s %P\n' | sort -rn | head -5 | awk '{ printf "  %7.1f MB  %s\n", $1 / 1000000, $2 }'

status=0
while read -r size path; do
  echo "::error::$path is $((size / 1000000)) MB; GitHub Pages files must stay under $((max_file / 1000000)) MB. Shrink it or stop publishing it on Pages (the Linux AppImage is the largest)."
  status=1
done < <(find "$dir" -type f -size +"$((max_file - 1))"c -printf '%s %P\n')
if [ "$total" -ge "$max_total" ]; then
  echo "::error::The Pages site is $((total / 1000000)) MB; it must stay under $((max_total / 1000000)) MB (GitHub's published-site limit is 1 GB)."
  status=1
fi
exit "$status"
