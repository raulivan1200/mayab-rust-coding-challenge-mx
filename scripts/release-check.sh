#!/usr/bin/env sh
set -eu

PORT="${PORT:-18080}"
BASE_URL="http://127.0.0.1:${PORT}"
TMP_DIR="${TMPDIR:-/tmp}/mayab-release-check.$$"
DB_PATH="${TMP_DIR}/auditoria.sqlite"
APP_PID=""

cleanup() {
  if [ -n "$APP_PID" ]; then
    kill "$APP_PID" 2>/dev/null || true
    wait "$APP_PID" 2>/dev/null || true
  fi
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Falta comando requerido: $1" >&2
    exit 127
  fi
}

require_cmd cargo
require_cmd curl
require_cmd node
require_cmd python3

mkdir -p "$TMP_DIR"

echo "Release check Mayab"
echo "- checks estaticos y unitarios"
make check

echo "- build release locked"
cargo build --release --locked

echo "- servidor release temporal en ${BASE_URL}"
PORT="$PORT" \
RUST_LOG=error \
AUDITORIA_DB_PATH="$DB_PATH" \
target/release/mayab-arbitrage &
APP_PID=$!

ready=0
for _ in $(seq 1 60); do
  if curl -fsS "${BASE_URL}/healthz" >/dev/null 2>&1 \
    && curl -fsS -X POST "${BASE_URL}/api/demo/final" -o "${TMP_DIR}/demo-final.json" 2>/dev/null \
    && curl -fsS "${BASE_URL}/api/jurado" -o "${TMP_DIR}/jurado.json" 2>/dev/null \
    && curl -fsS "${BASE_URL}/api/preflight" -o "${TMP_DIR}/preflight.json" 2>/dev/null \
    && curl -fsS "${BASE_URL}/api/paquete-evaluacion" -o "${TMP_DIR}/paquete.json" 2>/dev/null \
    && python3 - "${TMP_DIR}/demo-final.json" "${TMP_DIR}/jurado.json" "${TMP_DIR}/preflight.json" "${TMP_DIR}/paquete.json" <<'PY'
import json
import sys

demo = json.load(open(sys.argv[1]))
jurado = json.load(open(sys.argv[2]))
preflight = json.load(open(sys.argv[3]))
paquete = json.load(open(sys.argv[4]))
readiness = preflight.get("judgeReadiness") or {}
jury_state = jurado.get("estado") or {}
score = float(paquete.get("puntajeTotal") or 0)
checks = readiness.get("checks") or []
rubrica = readiness.get("rubricaOficial") or []
ok = (
    demo.get("ok") is True
    and jurado.get("nombre") == "Mayab Jury Mode"
    and jury_state.get("status") == "ready"
    and readiness.get("status") == "ready"
    and len(checks) >= 9
    and len(rubrica) == 5
    and score >= 90
)
sys.exit(0 if ok else 1)
PY
  then
    ready=1
    break
  fi
  sleep 1
done

if [ "$ready" -ne 1 ]; then
  echo "El servidor release no quedó listo para jurado en ${BASE_URL}" >&2
  exit 1
fi

echo "- smoke demo sobre binario release"
BASE_URL="$BASE_URL" ./scripts/smoke-demo.sh

echo "Release check OK"
