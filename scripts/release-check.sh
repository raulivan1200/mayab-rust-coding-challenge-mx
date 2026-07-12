#!/usr/bin/env sh
set -eu

if [ -n "${PORT:-}" ]; then
  PORT="$PORT"
else
  PORT="$(python3 - <<'PY'
import socket

with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)"
fi
BASE_URL="http://127.0.0.1:${PORT}"
TMP_DIR="${TMPDIR:-/tmp}/mayab-release-check.$$"
DB_PATH="${TMP_DIR}/auditoria.sqlite"
APP_PID=""
CHECK_ADMIN_TOKEN="${CHECK_ADMIN_TOKEN:-mayab-release-check-token-32-chars}"

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
ADMIN_TOKEN="$CHECK_ADMIN_TOKEN" \
target/release/mayab-arbitrage &
APP_PID=$!

# Dar oportunidad al proceso de reportar errores inmediatos (por ejemplo, un
# puerto ocupado) antes de consultar HTTP. Sin esta barrera, otro servicio en
# el mismo puerto podría responder al readiness y producir un falso positivo.
sleep 1
if ! kill -0 "$APP_PID" 2>/dev/null; then
  echo "El binario release termino al arrancar; verifica que ${BASE_URL} este libre" >&2
  wait "$APP_PID" 2>/dev/null || true
  exit 1
fi

ready=0
for _ in $(seq 1 60); do
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    echo "El binario release terminó antes de quedar listo en ${BASE_URL}" >&2
    wait "$APP_PID" 2>/dev/null || true
    exit 1
  fi
  if curl -fsS "${BASE_URL}/healthz" >/dev/null 2>&1 \
    && curl -fsS "${BASE_URL}/api/healthz" >/dev/null 2>&1 \
    && curl -fsS -X POST "${BASE_URL}/api/demo/reset" \
      -H "Authorization: Bearer ${CHECK_ADMIN_TOKEN}" \
      -o "${TMP_DIR}/demo-reset.json" 2>/dev/null \
    && curl -fsS -X POST "${BASE_URL}/api/demo/final" \
      -H "Authorization: Bearer ${CHECK_ADMIN_TOKEN}" \
      -o "${TMP_DIR}/demo-final.json" 2>/dev/null \
    && curl -fsS "${BASE_URL}/api/jurado" -o "${TMP_DIR}/jurado.json" 2>/dev/null \
    && curl -fsS "${BASE_URL}/api/preflight" -o "${TMP_DIR}/preflight.json" 2>/dev/null \
    && curl -fsS "${BASE_URL}/api/paquete-evaluacion" -o "${TMP_DIR}/paquete.json" 2>/dev/null \
    && python3 - "${TMP_DIR}/demo-reset.json" "${TMP_DIR}/demo-final.json" "${TMP_DIR}/jurado.json" "${TMP_DIR}/preflight.json" "${TMP_DIR}/paquete.json" <<'PY'
import json
import sys

reset = json.load(open(sys.argv[1]))
demo = json.load(open(sys.argv[2]))
jurado = json.load(open(sys.argv[3]))
preflight = json.load(open(sys.argv[4]))
paquete = json.load(open(sys.argv[5]))
readiness = preflight.get("judgeReadiness") or {}
jury_state = jurado.get("estado") or {}
checks = readiness.get("checks") or []
rubrica = readiness.get("rubricaOficial") or []
required_rubric_fields = {
    "criterio", "pesoPct", "estado", "preguntaComite",
    "evidenciaActual", "siguienteMovimientoDemo",
}
rubric_contract_ok = (
    len(rubrica) == 5
    and sum((item.get("pesoPct", 0) for item in rubrica)) == 100
    and all(required_rubric_fields.issubset(item) for item in rubrica)
    and all(item.get("criterio") and item.get("evidenciaActual") for item in rubrica)
)
evidence = paquete.get("evidencia") or {}
metrics = evidence.get("metricas") or {}
validation = (((paquete.get("evidencia") or {}).get("backtest") or {}).get("validacionMultisemilla") or {})
ok = (
    reset.get("ok") is True
    and reset.get("corridaId", "").startswith("jury-")
    and demo.get("ok") is True
    and jurado.get("nombre") == "Mayab Jury Mode"
    and jury_state.get("status") == "ready"
    and readiness.get("status") == "ready"
    and len(checks) >= 9
    and all(check.get("ok") is True for check in checks)
    and rubric_contract_ok
    and metrics.get("operaciones", 0) > 0
    and metrics.get("pnlUsd", 0) > 0
    and evidence.get("ultimaAuditoria")
    and evidence.get("ga")
    and demo.get("riesgoSegundaPierna", {}).get("estadoFinal") == "RECONCILED_LOSS"
    and demo.get("riesgoSegundaPierna", {}).get("exposicionFinalBtc") == 0
    and paquete.get("huellaAuditoria")
    and (validation.get("base") or {}).get("corridas") == 24
    and (validation.get("optimizada") or {}).get("corridas") == 24
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
BASE_URL="$BASE_URL" ADMIN_TOKEN="$CHECK_ADMIN_TOKEN" ./scripts/smoke-demo.sh

echo "Release check OK"
