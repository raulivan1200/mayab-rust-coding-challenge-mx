#!/usr/bin/env bash
set -euo pipefail

ROOT="${ROOT:-$(mktemp -d /tmp/mayab-corpus-smoke.XXXXXX)}"
REPORT="${REPORT:-$ROOT/corpus-verified.json}"
EVALUATION="${EVALUATION:-$ROOT/evaluation}"
DURATION="${DURATION:-40s}"
EXCHANGES="${EXCHANGES:-Kraken,Coinbase}"

cleanup() {
  if [[ "${KEEP_ARTIFACTS:-0}" != "1" ]]; then
    rm -rf "$ROOT"
  fi
}
trap cleanup EXIT

cargo run -p mayab-cli --bin capture-corpus -- \
  --root "$ROOT" --total "$DURATION" --shard "$DURATION" \
  --pair BTC/USD --exchanges "$EXCHANGES" --depth 10

cargo run -p mayab-cli --bin verify-corpus -- \
  --root "$ROOT" --output "$REPORT"

jq -e '
  .classification == "public_market_capture_corpus" and
  .uniqueTapes == 1 and .totalEvents > 0 and
  .evidenceGates.multiVenue == true and
  .evidenceGates.publishableScale == false and
  .evidenceGates.status == "insufficient_scale"
' "$REPORT" >/dev/null

SHARD="$(find "$ROOT" -maxdepth 1 -type d -name 'shard-*' | sort | head -n 1)"
test -n "$SHARD"
cargo run -p mayab-arbitrage --bin evaluate-tape -- \
  --tape "$SHARD" --split 50,20,30 --seed 20260712 --output "$EVALUATION"

jq -e '
  .quantitativeFunnel.rawQuotes > 0 and
  .quantitativeFunnel.validQuotes <= .quantitativeFunnel.rawQuotes and
  .quantitativeFunnel.netDislocations <= .quantitativeFunnel.grossDislocations and
  (.eventCounts | length == 3)
' "$EVALUATION/evaluation.json" >/dev/null

echo "research corpus smoke: PASS"
echo "corpus: $REPORT"
echo "evaluation: $EVALUATION/evaluation.json"
