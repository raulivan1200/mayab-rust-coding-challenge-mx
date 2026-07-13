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

if [ -n "${CANDIDATE_TAG:-}" ]; then
  CANDIDATE_TAG="${CANDIDATE_TAG}"
elif [ -n "${GITHUB_RUN_ID:-}" ]; then
  CANDIDATE_TAG="candidate-${GITHUB_RUN_ID}-${GITHUB_RUN_ATTEMPT:-1}"
else
  CANDIDATE_TAG="candidate-manual-$(date +%s)-$$"
fi

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

case "$CANDIDATE_TAG" in
  [a-z]* ) ;;
  *)
    echo "CANDIDATE_TAG debe iniciar con una letra minúscula" >&2
    exit 2
    ;;
esac
case "$CANDIDATE_TAG" in
  *[!a-z0-9-]*|*-)
    echo "CANDIDATE_TAG sólo acepta letras minúsculas, números y guiones, y no puede terminar en guion" >&2
    exit 2
    ;;
esac
if [ "${#CANDIDATE_TAG}" -gt 63 ]; then
  echo "CANDIDATE_TAG no puede superar 63 caracteres" >&2
  exit 2
fi

if [ "$MAYAB_ENV" = "production" ] && [ -n "${IMAGE:-}" ] \
  && ! printf '%s\n' "$IMAGE" | grep -Eq '@sha256:[0-9a-f]{64}$'; then
  echo "IMAGE debe ser una referencia inmutable por digest (@sha256:...) en producción" >&2
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

TMP_DIR="$(mktemp -d)"
SERVICE_EXISTED=false
PREVIOUS_TRAFFIC=""
CANDIDATE_REVISION=""
CANDIDATE_URL=""
SERVICE_URL=""
DEPLOY_ATTEMPTED=false
PROMOTION_ATTEMPTED=false

load_candidate_metadata() {
  if ! gcloud run services describe "$SERVICE" \
    --project "$PROJECT" \
    --region "$REGION" \
    --format=json > "$TMP_DIR/candidate-service.json" 2>/dev/null; then
    return 1
  fi

  CANDIDATE_REVISION="$(python3 - "$TMP_DIR/candidate-service.json" "$CANDIDATE_TAG" <<'PY'
import json
import sys

service = json.load(open(sys.argv[1]))
tag = sys.argv[2]
status = service.get("status") or {}
for target in status.get("traffic") or []:
    if target.get("tag") == tag:
        print(target.get("revisionName") or status.get("latestCreatedRevisionName") or "")
        break
PY
)"
  CANDIDATE_URL="$(python3 - "$TMP_DIR/candidate-service.json" "$CANDIDATE_TAG" <<'PY'
import json
import sys

service = json.load(open(sys.argv[1]))
tag = sys.argv[2]
for target in (service.get("status") or {}).get("traffic") or []:
    if target.get("tag") == tag:
        print(target.get("url") or "")
        break
PY
)"
  SERVICE_URL="$(python3 - "$TMP_DIR/candidate-service.json" <<'PY'
import json
import sys

service = json.load(open(sys.argv[1]))
print((service.get("status") or {}).get("url") or "")
PY
)"
  [ -n "$CANDIDATE_REVISION" ] && [ -n "$CANDIDATE_URL" ] && [ -n "$SERVICE_URL" ]
}

candidate_traffic_percent() {
  [ -n "$CANDIDATE_REVISION" ] || return 1
  gcloud run services describe "$SERVICE" \
    --project "$PROJECT" \
    --region "$REGION" \
    --format=json > "$TMP_DIR/current-service.json" 2>/dev/null || return 1
  python3 - "$TMP_DIR/current-service.json" "$CANDIDATE_REVISION" <<'PY'
import json
import sys

service = json.load(open(sys.argv[1]))
revision = sys.argv[2]
percent = 0
for target in (service.get("status") or {}).get("traffic") or []:
    if target.get("revisionName") == revision:
        percent += int(target.get("percent") or 0)
print(percent)
PY
}

cleanup() {
  status=$?
  trap - EXIT INT TERM

  if [ "$status" -ne 0 ] && [ "$DEPLOY_ATTEMPTED" = "true" ]; then
    echo "Rollout fallido; restaurando el estado anterior de Cloud Run" >&2

    if [ "$SERVICE_EXISTED" = "false" ]; then
      if ! gcloud run services delete "$SERVICE" \
        --project "$PROJECT" --region "$REGION" --quiet; then
        echo "No se pudo eliminar el servicio nuevo; requiere limpieza manual" >&2
      fi
    else
      if [ -z "$CANDIDATE_REVISION" ]; then
        load_candidate_metadata || true
      fi

      rollback_ok=true
      if [ "$PROMOTION_ATTEMPTED" = "true" ]; then
        echo "Restaurando tráfico previo: $PREVIOUS_TRAFFIC" >&2
        if ! gcloud run services update-traffic "$SERVICE" \
          --project "$PROJECT" --region "$REGION" \
          --to-revisions "$PREVIOUS_TRAFFIC" --quiet; then
          rollback_ok=false
          echo "No se pudo restaurar el tráfico previo; no se eliminará la candidata" >&2
        fi
      fi

      if ! gcloud run services update-traffic "$SERVICE" \
        --project "$PROJECT" --region "$REGION" \
        --remove-tags "$CANDIDATE_TAG" --quiet; then
        echo "No se pudo retirar el tag $CANDIDATE_TAG; requiere limpieza manual" >&2
      fi

      if [ "$rollback_ok" = "true" ] && [ -n "$CANDIDATE_REVISION" ]; then
        candidate_percent="$(candidate_traffic_percent || true)"
        if [ "$candidate_percent" = "0" ]; then
          if ! gcloud run revisions delete "$CANDIDATE_REVISION" \
            --project "$PROJECT" --region "$REGION" --quiet; then
            echo "La revisión candidata quedó sin tráfico pero no pudo eliminarse" >&2
          fi
        else
          echo "No se elimina la candidata: no pudo probarse que tenga 0% de tráfico" >&2
        fi
      fi
    fi
  fi

  rm -rf "$TMP_DIR"
  exit "$status"
}
trap 'exit 130' INT
trap 'exit 143' TERM
trap cleanup EXIT

if gcloud run services describe "$SERVICE" \
  --project "$PROJECT" \
  --region "$REGION" \
  --format=json > "$TMP_DIR/previous-service.json" 2> "$TMP_DIR/previous-service.err"; then
  SERVICE_EXISTED=true
  if ! PREVIOUS_TRAFFIC="$(python3 - "$TMP_DIR/previous-service.json" <<'PY'
import json
import sys

service = json.load(open(sys.argv[1]))
status = service.get("status") or {}
latest_ready = status.get("latestReadyRevisionName") or ""
allocations = {}
for target in status.get("traffic") or []:
    percent = int(target.get("percent") or 0)
    if percent <= 0:
        continue
    revision = target.get("revisionName")
    if not revision and target.get("latestRevision"):
        revision = latest_ready
    if not revision:
        raise SystemExit("un target con tráfico no expone revisionName")
    allocations[revision] = allocations.get(revision, 0) + percent
if sum(allocations.values()) != 100:
    raise SystemExit("la asignación previa de tráfico no suma 100")
print(",".join(f"{revision}={percent}" for revision, percent in sorted(allocations.items())))
PY
)"; then
    echo "No se pudo capturar una asignación de tráfico restaurable" >&2
    exit 1
  fi
  if [ -z "$PREVIOUS_TRAFFIC" ]; then
    echo "El servicio existe pero no expone tráfico previo restaurable" >&2
    exit 1
  fi
elif grep -Eqi 'NOT_FOUND|not found|cannot find|does not exist' "$TMP_DIR/previous-service.err"; then
  SERVICE_EXISTED=false
else
  cat "$TMP_DIR/previous-service.err" >&2
  echo "No se pudo determinar de forma segura si el servicio ya existe" >&2
  exit 1
fi

echo "Desplegando candidata sin tráfico: $SERVICE en $PROJECT/$REGION (tag=$CANDIDATE_TAG)"
DEPLOY_ATTEMPTED=true
if [ -n "$SECRETS" ]; then
  gcloud run deploy "$SERVICE" "$@" \
    --project "$PROJECT" --region "$REGION" --allow-unauthenticated \
    --memory "$MEMORY" --cpu "$CPU" --port 8080 \
    --concurrency "$CONCURRENCY" --timeout "$TIMEOUT" \
    --min-instances "$MIN_INSTANCES" --max-instances "$MAX_INSTANCES" \
    --execution-environment gen2 --cpu-boost \
    --no-traffic --tag "$CANDIDATE_TAG" \
    --set-env-vars "$ENV_VARS" --set-secrets "$SECRETS" --quiet
else
  gcloud run deploy "$SERVICE" "$@" \
    --project "$PROJECT" --region "$REGION" --allow-unauthenticated \
    --memory "$MEMORY" --cpu "$CPU" --port 8080 \
    --concurrency "$CONCURRENCY" --timeout "$TIMEOUT" \
    --min-instances "$MIN_INSTANCES" --max-instances "$MAX_INSTANCES" \
    --execution-environment gen2 --cpu-boost \
    --no-traffic --tag "$CANDIDATE_TAG" \
    --set-env-vars "$ENV_VARS" --quiet
fi

metadata_attempt=0
while [ "$metadata_attempt" -lt 30 ]; do
  if load_candidate_metadata; then
    break
  fi
  metadata_attempt=$((metadata_attempt + 1))
  sleep 2
done
if [ -z "$CANDIDATE_REVISION" ] || [ -z "$CANDIDATE_URL" ] || [ -z "$SERVICE_URL" ]; then
  echo "No se pudo resolver la revisión/URL etiquetada de la candidata" >&2
  exit 1
fi

case "$CANDIDATE_URL" in
  https://*) ;;
  *)
    echo "La URL candidata no es HTTPS: $CANDIDATE_URL" >&2
    exit 1
    ;;
esac

if [ -n "${IMAGE:-}" ]; then
  deployed_image="$(gcloud run revisions describe "$CANDIDATE_REVISION" \
    --project "$PROJECT" --region "$REGION" \
    --format='value(spec.containers[0].image)')"
  if [ "$deployed_image" != "$IMAGE" ]; then
    echo "La revisión no apunta al digest probado: esperado=$IMAGE recibido=$deployed_image" >&2
    exit 1
  fi
fi

smoke_get() {
  path="$1"
  output="$2"
  curl --fail --silent --show-error --location \
    --retry 8 --retry-delay 2 --retry-all-errors \
    --connect-timeout 10 --max-time 30 \
    "${CANDIDATE_URL}${path}" -o "$output"
}

check_preflight() {
  python3 - "$1" <<'PY'
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
}

echo "Validando revisión candidata fuera de tráfico en ${CANDIDATE_URL}"
smoke_get "/healthz" "$TMP_DIR/healthz-canonical.json"
smoke_get "/api/healthz" "$TMP_DIR/healthz-alias.json"
grep -Eq '"ok"[[:space:]]*:[[:space:]]*true' "$TMP_DIR/healthz-canonical.json"
grep -Eq '"ok"[[:space:]]*:[[:space:]]*true' "$TMP_DIR/healthz-alias.json"

ready=false
ready_attempt=0
while [ "$ready_attempt" -lt 60 ]; do
  if smoke_get "/readyz" "$TMP_DIR/readyz.json" \
    && grep -Eq '"ready"[[:space:]]*:[[:space:]]*true' "$TMP_DIR/readyz.json"; then
    ready=true
    break
  fi
  ready_attempt=$((ready_attempt + 1))
  sleep 2
done
if [ "$ready" != "true" ]; then
  echo "La revisión candidata no alcanzó readiness" >&2
  exit 1
fi
smoke_get "/api/readyz" "$TMP_DIR/readyz-alias.json"
grep -Eq '"ready"[[:space:]]*:[[:space:]]*true' "$TMP_DIR/readyz-alias.json"

smoke_get "/api/version" "$TMP_DIR/version.json"
if [ -n "${GITHUB_SHA:-}" ]; then
  grep -Fq "\"gitSha\":\"${GITHUB_SHA}\"" "$TMP_DIR/version.json"
fi

preflight_ready=false
preflight_attempt=0
while [ "$preflight_attempt" -lt 60 ]; do
  if smoke_get "/api/preflight" "$TMP_DIR/preflight.json" \
    && check_preflight "$TMP_DIR/preflight.json" 2>/dev/null; then
    preflight_ready=true
    break
  fi
  preflight_attempt=$((preflight_attempt + 1))
  sleep 2
done
if [ "$preflight_ready" != "true" ]; then
  check_preflight "$TMP_DIR/preflight.json" || true
  echo "La revisión candidata no alcanzó preflight 12/12" >&2
  exit 1
fi

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

ADMIN_TOKEN_VALUE="${ADMIN_TOKEN:-}"
if [ -z "$ADMIN_TOKEN_VALUE" ] && [ -n "${ADMIN_TOKEN_SECRET:-}" ]; then
  secret_name="${ADMIN_TOKEN_SECRET%%:*}"
  secret_version="${ADMIN_TOKEN_SECRET#*:}"
  if [ "$secret_version" = "$ADMIN_TOKEN_SECRET" ]; then
    secret_version=latest
  fi
  ADMIN_TOKEN_VALUE="$(gcloud secrets versions access "$secret_version" \
    --secret "$secret_name" --project "$PROJECT")"
fi

if [ -n "$ADMIN_TOKEN_VALUE" ]; then
  if [ "${GITHUB_ACTIONS:-false}" = "true" ]; then
    printf '::add-mask::%s\n' "$ADMIN_TOKEN_VALUE"
  fi
  test -x ./scripts/smoke-demo.sh
  BASE_URL="$CANDIDATE_URL" ADMIN_TOKEN="$ADMIN_TOKEN_VALUE" ./scripts/smoke-demo.sh
elif [ "$MAYAB_ENV" = "production" ]; then
  echo "No se pudo resolver ADMIN_TOKEN para el smoke completo de la candidata" >&2
  exit 1
else
  echo "Smoke mutante omitido fuera de producción porque no se proporcionó ADMIN_TOKEN" >&2
fi

# El smoke completo termina restaurando /api/demo/final; verificar una última
# vez el estado exacto que recibirá tráfico.
smoke_get "/api/preflight" "$TMP_DIR/preflight-final.json"
check_preflight "$TMP_DIR/preflight-final.json"

echo "Promoviendo ${CANDIDATE_REVISION} a 100% después del smoke"
PROMOTION_ATTEMPTED=true
gcloud run services update-traffic "$SERVICE" \
  --project "$PROJECT" --region "$REGION" \
  --to-revisions "${CANDIDATE_REVISION}=100" --quiet

promoted_percent="$(candidate_traffic_percent)"
if [ "$promoted_percent" != "100" ]; then
  echo "Cloud Run no confirmó 100% de tráfico en la candidata (observado=$promoted_percent)" >&2
  exit 1
fi

# El tag sólo existe para el smoke sin tráfico. Su retiro no cambia la
# asignación por revisión ya promovida y no debe invalidar un rollout sano.
if ! gcloud run services update-traffic "$SERVICE" \
  --project "$PROJECT" --region "$REGION" \
  --remove-tags "$CANDIDATE_TAG" --quiet; then
  echo "Advertencia: la revisión quedó promovida, pero el tag temporal no pudo retirarse" >&2
fi

if [ -n "${GITHUB_OUTPUT:-}" ]; then
  printf 'url=%s\nrevision=%s\nimage=%s\n' \
    "$SERVICE_URL" "$CANDIDATE_REVISION" "${IMAGE:-source-build}" >> "$GITHUB_OUTPUT"
fi

echo "Deploy promovido y validado: ${SERVICE_URL} (${CANDIDATE_REVISION})"
