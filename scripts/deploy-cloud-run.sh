#!/usr/bin/env sh
set -eu

SERVICE="${SERVICE:-mayab-btc-arbitrage}"
REGION="${REGION:-us-central1}"
PROJECT="${PROJECT:-}"
RUNTIME_SERVICE_ACCOUNT="${RUNTIME_SERVICE_ACCOUNT:-}"
MIN_INSTANCES="${MIN_INSTANCES:-1}"
MAX_INSTANCES="${MAX_INSTANCES:-1}"
MEMORY="${MEMORY:-512Mi}"
CPU="${CPU:-1}"
CONCURRENCY="${CONCURRENCY:-20}"
TIMEOUT="${TIMEOUT:-3600}"
MAYAB_ENV="${MAYAB_ENV:-production}"
MAYAB_JUDGE_MODE="${MAYAB_JUDGE_MODE:-true}"
AUDITORIA_DB_PATH="${AUDITORIA_DB_PATH:-/data/mayab-auditoria.sqlite}"
STORAGE_MODE="${STORAGE_MODE:-sqlite_ephemeral}"

if [ -n "${DATABASE_URL_SECRET:-}" ]; then
  STORAGE_MODE="timescaledb"
fi

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Falta comando requerido: $1" >&2
    exit 127
  fi
}

require_cmd gcloud
require_cmd curl
require_cmd grep
require_cmd mktemp
require_cmd python3

if [ -z "$PROJECT" ]; then
  PROJECT="$(gcloud config get-value project 2>/dev/null || true)"
fi

case "$PROJECT" in
  ""|"(unset)")
    echo "Define PROJECT o configura un proyecto activo con gcloud" >&2
    exit 2
    ;;
esac

case "$MIN_INSTANCES:$MAX_INSTANCES:$CONCURRENCY:$TIMEOUT" in
  *[!0-9:]*|*::*|:*|*:)
    echo "MIN_INSTANCES, MAX_INSTANCES, CONCURRENCY y TIMEOUT deben ser enteros" >&2
    exit 2
    ;;
esac

case "$MAYAB_JUDGE_MODE" in
  true|false) ;;
  *)
    echo "MAYAB_JUDGE_MODE debe ser true o false" >&2
    exit 2
    ;;
esac

if [ "$MIN_INSTANCES" -gt "$MAX_INSTANCES" ]; then
  echo "MIN_INSTANCES no puede ser mayor que MAX_INSTANCES" >&2
  exit 2
fi

if [ "$MAYAB_ENV" = "production" ] && [ -z "${ADMIN_TOKEN_SECRET:-}" ]; then
  echo "ADMIN_TOKEN_SECRET es obligatorio cuando MAYAB_ENV=production (ej. mayab-admin-token:3)" >&2
  exit 2
fi

if [ "$MAYAB_ENV" = "production" ] && [ -z "$RUNTIME_SERVICE_ACCOUNT" ]; then
  echo "RUNTIME_SERVICE_ACCOUNT es obligatorio en producción; usa una identidad dedicada con privilegios mínimos" >&2
  exit 2
fi

if [ "$MAYAB_ENV" = "production" ] && [ "$STORAGE_MODE" != "timescaledb" ]; then
  echo "DATABASE_URL_SECRET es obligatorio en producción para persistencia durable" >&2
  exit 2
fi

# Build --set-secrets only for secrets that actually exist
SECRETS=""
if [ -n "${ADMIN_TOKEN_SECRET:-}" ]; then
  SECRETS="ADMIN_TOKEN=${ADMIN_TOKEN_SECRET}"
fi
if [ -n "${NVIDIA_API_KEY_SECRET:-}" ]; then
  if [ -n "$SECRETS" ]; then
    SECRETS="${SECRETS},NVIDIA_API_KEY=${NVIDIA_API_KEY_SECRET}"
  else
    SECRETS="NVIDIA_API_KEY=${NVIDIA_API_KEY_SECRET}"
  fi
fi
if [ -n "${DISCORD_BOT_TOKEN_SECRET:-}" ]; then
  if [ -n "$SECRETS" ]; then
    SECRETS="${SECRETS},DISCORD_BOT_TOKEN=${DISCORD_BOT_TOKEN_SECRET}"
  else
    SECRETS="DISCORD_BOT_TOKEN=${DISCORD_BOT_TOKEN_SECRET}"
  fi
fi
if [ -n "${DATABASE_URL_SECRET:-}" ]; then
  if [ -n "$SECRETS" ]; then
    SECRETS="${SECRETS},DATABASE_URL=${DATABASE_URL_SECRET}"
  else
    SECRETS="DATABASE_URL=${DATABASE_URL_SECRET}"
  fi
fi

if [ -n "${IMAGE:-}" ]; then
  set -- --image "$IMAGE"
else
  set -- --source .
fi
if [ -n "$RUNTIME_SERVICE_ACCOUNT" ]; then
  set -- "$@" --service-account "$RUNTIME_SERVICE_ACCOUNT"
fi

# Build env vars list
# Cloud Run no garantiza que el primer X-Forwarded-For haya sido saneado si el
# cliente ya envió el encabezado. El limitador de la app usa por defecto la IP
# observada por el socket; habilitar proxy headers requiere una capa perimetral
# que limpie la cadena antes de llegar al servicio.
ENV_VARS="RUST_LOG=info,MAYAB_ENV=${MAYAB_ENV},ENTORNO=${MAYAB_ENV},MAYAB_JUDGE_MODE=${MAYAB_JUDGE_MODE},AUDITORIA_DB_PATH=${AUDITORIA_DB_PATH},STORAGE_MODE=${STORAGE_MODE},DEMO_RENTABLE_INICIAL=false,TRUST_PROXY_HEADERS=false"

# Optional env vars with defaults
if [ -n "${DISCORD_APPLICATION_ID:-}" ]; then
  ENV_VARS="${ENV_VARS},DISCORD_APPLICATION_ID=${DISCORD_APPLICATION_ID}"
fi
if [ -n "${DISCORD_PUBLIC_KEY:-}" ]; then
  ENV_VARS="${ENV_VARS},DISCORD_PUBLIC_KEY=${DISCORD_PUBLIC_KEY}"
fi
if [ -n "${DISCORD_GUILD_ID:-}" ]; then
  ENV_VARS="${ENV_VARS},DISCORD_GUILD_ID=${DISCORD_GUILD_ID}"
fi

# Fee and withdrawal env vars
ENV_VARS="${ENV_VARS},FEE_BINANCE=${FEE_BINANCE:-0.0010},FEE_KRAKEN=${FEE_KRAKEN:-0.0026},FEE_COINBASE=${FEE_COINBASE:-0.0060},FEE_OKX=${FEE_OKX:-0.0010},FEE_BYBIT=${FEE_BYBIT:-0.0010}"
ENV_VARS="${ENV_VARS},RETIRO_BTC_BINANCE=${RETIRO_BTC_BINANCE:-0.00010},RETIRO_BTC_KRAKEN=${RETIRO_BTC_KRAKEN:-0.00020},RETIRO_BTC_COINBASE=${RETIRO_BTC_COINBASE:-0.00012},RETIRO_BTC_OKX=${RETIRO_BTC_OKX:-0.00010},RETIRO_BTC_BYBIT=${RETIRO_BTC_BYBIT:-0.00010}"

echo "Desplegando: $SERVICE en $PROJECT/$REGION"
if [ -n "$SECRETS" ]; then
  gcloud run deploy "$SERVICE" "$@" \
    --project "$PROJECT" --region "$REGION" --allow-unauthenticated \
    --memory "$MEMORY" --cpu "$CPU" --port 8080 \
    --concurrency "$CONCURRENCY" --timeout "$TIMEOUT" \
    --min-instances "$MIN_INSTANCES" --max-instances "$MAX_INSTANCES" \
    --execution-environment gen2 --cpu-boost \
    --set-env-vars "$ENV_VARS" --set-secrets "$SECRETS" --quiet
else
  gcloud run deploy "$SERVICE" "$@" \
    --project "$PROJECT" --region "$REGION" --allow-unauthenticated \
    --memory "$MEMORY" --cpu "$CPU" --port 8080 \
    --concurrency "$CONCURRENCY" --timeout "$TIMEOUT" \
    --min-instances "$MIN_INSTANCES" --max-instances "$MAX_INSTANCES" \
    --execution-environment gen2 --cpu-boost \
    --set-env-vars "$ENV_VARS" --quiet
fi

SERVICE_URL="$(gcloud run services describe "$SERVICE" \
  --project "$PROJECT" \
  --region "$REGION" \
  --format='value(status.url)')"

if [ -z "$SERVICE_URL" ]; then
  echo "No se pudo resolver la URL del servicio desplegado" >&2
  exit 1
fi

TMP_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

smoke_get() {
  path="$1"
  output="$2"
  curl --fail --silent --show-error --location \
    --retry 8 --retry-delay 2 --retry-all-errors \
    --connect-timeout 10 --max-time 30 \
    "${SERVICE_URL}${path}" -o "$output"
}

echo "Validando revision publica en ${SERVICE_URL}"
smoke_get "/api/healthz" "$TMP_DIR/healthz.json"
grep -Eq '"ok"[[:space:]]*:[[:space:]]*true' "$TMP_DIR/healthz.json"

smoke_get "/api/readyz" "$TMP_DIR/readyz.json"
grep -Eq '"ready"[[:space:]]*:[[:space:]]*true' "$TMP_DIR/readyz.json"

smoke_get "/api/version" "$TMP_DIR/version.json"
if [ -n "${GITHUB_SHA:-}" ]; then
  grep -Fq "\"gitSha\":\"${GITHUB_SHA}\"" "$TMP_DIR/version.json"
fi

smoke_get "/api/preflight" "$TMP_DIR/preflight.json"
python3 - "$TMP_DIR/preflight.json" <<'PY'
import json
import sys

preflight = json.load(open(sys.argv[1]))
readiness = preflight.get("judgeReadiness") or {}
checks = readiness.get("checks") or []
persistence = preflight.get("persistencia") or {}
if not (
    preflight.get("listo") is True
    and readiness.get("status") == "ready"
    and readiness.get("evidenceStatus") == "complete"
    and readiness.get("passed") == 12
    and readiness.get("total") == 12
    and len(checks) == 12
    and all(check.get("ok") is True for check in checks)
    and (readiness.get("executionMatrix") or {}).get("passed") == 12
    and (readiness.get("executionMatrix") or {}).get("total") == 12
    and (readiness.get("executionMatrix") or {}).get("allPassed") is True
    and ((readiness.get("twoLegEvidence") or {}).get("invariants") or {}).get("allPassed") is True
    and persistence.get("backend") == "timescaledb"
    and persistence.get("storagePersistent") is True
    and persistence.get("queueDropped", 0) == 0
    and persistence.get("queueFailed", 0) == 0
):
    raise SystemExit("preflight del deploy no quedó completamente verde")
PY

smoke_get "/api/export/csv" "$TMP_DIR/auditoria.csv"
test -s "$TMP_DIR/auditoria.csv"
grep -q '^tipo,tiempo,ruta,detalle,cantidad_btc' "$TMP_DIR/auditoria.csv"
grep -q '^operacion,' "$TMP_DIR/auditoria.csv"
grep -q '^transicion,' "$TMP_DIR/auditoria.csv"

smoke_get "/api/resumen-llm" "$TMP_DIR/resumen-llm.json"
grep -q '"resumen"' "$TMP_DIR/resumen-llm.json"

smoke_get "/api/research/tapes" "$TMP_DIR/research-tapes.json"
python3 - "$TMP_DIR/research-tapes.json" <<'PY'
import json
import sys

report = json.load(open(sys.argv[1]))
tapes = report.get("tapes") or []
if not (
    report.get("available") is True
    and tapes
    and tapes[0].get("provenance") == "repository_sample_unverified"
    and tapes[0].get("classification") == "unverified_market_sample"
    and tapes[0].get("authenticityVerified") is False
    and tapes[0].get("events", 0) >= 2
    and tapes[0].get("sha256")
):
    raise SystemExit("el deploy no publicó el tape de mercado versionado")
PY

smoke_get "/" "$TMP_DIR/index.html"
grep -Eqi '<title>[^<]*Mayab' "$TMP_DIR/index.html"
grep -Eq 'src="/app\.js|href="/styles\.css' "$TMP_DIR/index.html"
smoke_get "/app.js" "$TMP_DIR/app.js"
smoke_get "/styles.css" "$TMP_DIR/styles.css"
test -s "$TMP_DIR/app.js"
test -s "$TMP_DIR/styles.css"

echo "Deploy validado: ${SERVICE_URL}"
