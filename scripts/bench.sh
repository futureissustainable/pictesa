#!/usr/bin/env bash
# Measures Pictesa's own overhead (not model time): binary size, cold start, memory, API latency, UI weight.
# Usage: scripts/bench.sh [path/to/pictesa]   (uses a throwaway data folder and the free mock models)
set -euo pipefail
BIN=${1:-target/release/pictesa}
PORT=47499
DATA=$(mktemp -d)
trap 'kill $PID 2>/dev/null || true; rm -rf "$DATA"' EXIT
export PICTESA_DATA_DIR=$DATA PICTESA_MOCK=1 PICTESA_LOG=warn
ms() { date +%s%N | cut -b1-13; }

echo "binary: $(du -h "$BIN" | cut -f1)"
start=$(ms)
"$BIN" serve --port $PORT >/dev/null 2>&1 &
PID=$!
until curl -sf "http://127.0.0.1:$PORT/api/health" >/dev/null; do sleep 0.005; done
echo "cold start to first API answer: $(( $(ms) - start )) ms"
sleep 1
echo "memory after start: $(( $(ps -o rss= -p $PID) / 1024 )) MB"

lat() { # lat <path> <runs>
  local total=0 best=999999
  for _ in $(seq "$2"); do
    t=$(curl -s -o /dev/null -w '%{time_total}' "http://127.0.0.1:$PORT$1")
    t=$(awk -v t="$t" 'BEGIN{printf "%d", t*1000000}')
    total=$((total + t)); (( t < best )) && best=$t
  done
  echo "$1: median-ish avg $((total / $2 / 1000)).$(( (total / $2) % 1000 / 100 )) ms, best $((best / 1000)).$(( best % 1000 / 100 )) ms"
}
lat /api/models 50
lat /api/presets 50
lat /api/assets 50
lat / 50

# Queue overhead: the mock model takes a fixed 4 s, so anything above that is Pictesa.
start=$(ms)
id=$(curl -s -X POST -H 'content-type: application/json' -d '{"model":"mock:image","prompt":"bench"}' "http://127.0.0.1:$PORT/api/generations" | python3 -c 'import sys,json; print(json.load(sys.stdin)["jobs"][0]["id"])')
for _ in $(seq 1500); do curl -s "http://127.0.0.1:$PORT/api/generations/$id" | grep -q '"status":"succeeded"' && break; sleep 0.02; done
echo "mock image end to end: $(( $(ms) - start )) ms (model time 4000 ms)"

if [ -d web/dist ]; then
  total=$(find web/dist -type f \( -name '*.js' -o -name '*.css' -o -name '*.html' \) -print0 | xargs -0 cat | wc -c)
  gz=$(find web/dist -type f \( -name '*.js' -o -name '*.css' -o -name '*.html' \) -print0 | xargs -0 cat | gzip -9 | wc -c)
  echo "UI code: $((total / 1024)) KB, $((gz / 1024)) KB gzipped"
fi
