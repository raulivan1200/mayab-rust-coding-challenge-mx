#!/usr/bin/env sh
set -eu

BASE_URL="${BASE_URL:-http://127.0.0.1:8080}"
TMP_DIR="${TMPDIR:-/tmp}/mayab-smoke-demo.$$"

cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

mkdir -p "$TMP_DIR"

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Falta comando requerido: $1" >&2
    exit 127
  fi
}

json_get() {
  path="$1"
  out="$2"
  curl -fsS "$BASE_URL$path" -o "$out"
}

json_post() {
  path="$1"
  payload="$2"
  out="$3"
  if [ -n "${ADMIN_TOKEN:-}" ]; then
    curl -fsS -X POST "$BASE_URL$path" \
      -H "Content-Type: application/json" \
      -H "Authorization: Bearer ${ADMIN_TOKEN}" \
      -d "$payload" \
      -o "$out"
    return
  fi

  case "$BASE_URL" in
    http://127.0.0.1:*|http://localhost:*) ;;
    *)
      echo "ADMIN_TOKEN es obligatorio para mutaciones contra un servidor remoto" >&2
      return 2
      ;;
  esac

  curl -fsS -X POST "$BASE_URL$path" \
    -H "Content-Type: application/json" \
    -d "$payload" \
    -o "$out"
}

wait_preflight_ready() {
  out="$1"
  attempts="${2:-45}"
  for _ in $(seq 1 "$attempts"); do
    if json_get "/api/preflight" "$out" 2>/dev/null \
      && python3 - "$out" <<'PY'
import json
import sys

preflight = json.load(open(sys.argv[1]))
readiness = preflight.get("judgeReadiness") or {}
checks = readiness.get("checks") or []
matrix = readiness.get("executionMatrix") or {}
persistence = preflight.get("persistencia") or {}
ok = (
    preflight.get("listo") is True
    and readiness.get("status") == "ready"
    and readiness.get("passed") == 12
    and readiness.get("total") == 12
    and len(checks) == 12
    and all(check.get("ok") is True for check in checks)
    and matrix.get("passed") == 12
    and matrix.get("total") == 12
    and matrix.get("allPassed") is True
    and (((readiness.get("twoLegEvidence") or {}).get("invariants") or {}).get("allPassed") is True)
    and persistence.get("queueDropped", 0) == 0
    and persistence.get("queueFailed", 0) == 0
)
sys.exit(0 if ok else 1)
PY
    then
      return 0
    fi
    sleep 1
  done
  return 1
}

require_cmd curl
require_cmd python3

echo "Smoke Mayab contra $BASE_URL"

if ! json_get "${HEALTH_PATH:-/api/healthz}" "$TMP_DIR/healthz.json" 2>/dev/null; then
  json_get "/api/preflight" "$TMP_DIR/healthz.json"
fi
json_get "/api/jurado" "$TMP_DIR/jurado-inicial.json"
json_get "/api/preflight" "$TMP_DIR/preflight-inicial.json"
json_post "/api/ga/evolucionar" '{"usarReplaySiVacio":true,"muestras":96}' "$TMP_DIR/ga.json"
json_post "/api/demo" '{"escenario":"mercado_rentable"}' "$TMP_DIR/demo-rentable.json"
json_post "/api/demo" '{"escenario":"rebalanceo"}' "$TMP_DIR/demo-rebalanceo.json"
json_post "/api/demo/final" '{}' "$TMP_DIR/demo-final.json"
json_post "/api/demo/caos" '{}' "$TMP_DIR/demo-caos.json"
wait_preflight_ready "$TMP_DIR/preflight-demo.json" 60 || true
json_get "/api/estado" "$TMP_DIR/estado.json"
json_get "/api/jurado" "$TMP_DIR/jurado.json"
json_get "/api/paquete-evaluacion" "$TMP_DIR/paquete.json"
json_get "/api/resumen-llm" "$TMP_DIR/resumen.json"
json_get "/api/mcp/manifest" "$TMP_DIR/mcp-manifest.json"
json_post "/api/mcp/call" '{"tool":"summarize_for_llm"}' "$TMP_DIR/mcp-summary.json"
json_get "/api/backtest" "$TMP_DIR/backtest.json"
json_get "/api/lab/sweep" "$TMP_DIR/lab-sweep.json"
json_get "/api/version" "$TMP_DIR/version.json"
json_get "/api/research/economics" "$TMP_DIR/economics.json"
json_get "/api/research/execution-matrix" "$TMP_DIR/execution-matrix.json"
json_get "/api/research/ledger-audit" "$TMP_DIR/ledger-audit.json"
json_get "/api/ga/sensibilidad" "$TMP_DIR/ga-sensibilidad.json"
json_get "/api/export/json" "$TMP_DIR/export.json"
json_get "/api/export/csv" "$TMP_DIR/export.csv"
json_post "/api/demo" '{"escenario":"liquidez_insuficiente"}' "$TMP_DIR/demo-liquidez.json"
json_post "/api/demo" '{"escenario":"circuit_breaker"}' "$TMP_DIR/demo-circuit.json"
json_get "/api/estado" "$TMP_DIR/estado-adverso.json"
json_post "/api/demo/final" '{}' "$TMP_DIR/demo-restaurada.json"

# Los feeds públicos pueden tardar en conectar al arrancar o atravesar una
# reconexión breve después de los escenarios adversos. Esperar evidencia fresca
# evita falsos negativos sin esconder un fallo persistente.
wait_preflight_ready "$TMP_DIR/preflight-final.json" 60 || true

python3 - "$TMP_DIR" <<'PY'
import json
import pathlib
import sys

tmp = pathlib.Path(sys.argv[1])

def load(name):
    return json.loads((tmp / name).read_text())

healthz = load("healthz.json")
jurado_inicial = load("jurado-inicial.json")
preflight = load("preflight-inicial.json")
ga = load("ga.json")
demo = load("demo-rentable.json")
rebalanceo = load("demo-rebalanceo.json")
demo_final = load("demo-final.json")
demo_caos = load("demo-caos.json")
estado = load("estado.json")
jurado = load("jurado.json")
paquete = load("paquete.json")
resumen = load("resumen.json")
mcp_manifest = load("mcp-manifest.json")
mcp_summary = load("mcp-summary.json")
backtest = load("backtest.json")
lab = load("lab-sweep.json")
version = load("version.json")
economics = load("economics.json")
execution_matrix = load("execution-matrix.json")
ledger_audit = load("ledger-audit.json")
sensibilidad = load("ga-sensibilidad.json")
export_json = load("export.json")
demo_liquidez = load("demo-liquidez.json")
demo_circuit = load("demo-circuit.json")
estado_adverso = load("estado-adverso.json")
demo_restaurada = load("demo-restaurada.json")
preflight_final = load("preflight-final.json")
export_csv = (tmp / "export.csv").read_text()

errors = []

if healthz.get("ok") is not True and healthz.get("listo") is not True:
    errors.append("health endpoint no devolvio ok=true")

if jurado_inicial.get("nombre") != "Mayab Jury Mode":
    errors.append("/api/jurado no devolvio Jury Mode")
if len(jurado_inicial.get("rubricaOficial") or []) != 5:
    errors.append("/api/jurado inicial no expone los 5 criterios de rubrica oficial")

readiness = preflight.get("judgeReadiness") or {}
rubrica_preflight = readiness.get("rubricaOficial") or []
if len(rubrica_preflight) != 5:
    errors.append("/api/preflight no expone los 5 criterios de rubrica oficial")

if ga.get("ok") is not True or ga.get("generacion", 0) < 1:
    errors.append("/api/ga/evolucionar no activo una generacion valida")

if demo.get("ok") is not True:
    errors.append("/api/demo mercado_rentable fallo")

if rebalanceo.get("ok") is not True:
    errors.append("/api/demo rebalanceo fallo")

if demo_final.get("ok") is not True:
    errors.append("/api/demo/final fallo")
if demo_final.get("persistenciaDrenada") is not True:
    errors.append("/api/demo/final no confirmó persistencia drenada")
if not str(demo_final.get("resultSha256", "")).startswith("sha256:"):
    errors.append("/api/demo/final no selló resultSha256")
if demo_final.get("mercadoMovido", {}).get("ok") is not True:
    errors.append("/api/demo/final no probo mercado_movido")
if demo_final.get("liquidezInsuficiente", {}).get("ok") is not True:
    errors.append("/api/demo/final no probo liquidez_insuficiente")
if demo_caos.get("ok") is not True or demo_caos.get("aprobados") != demo_caos.get("totalChecks"):
    errors.append("/api/demo/caos no supero todos los checks")
if abs(demo_caos.get("estadoFinal", {}).get("exposicionResidualBtc", 1)) >= 1e-9:
    errors.append("/api/demo/caos termino con exposicion residual")
if demo_caos.get("estadoFinal", {}).get("circuitBreakerActivo"):
    errors.append("/api/demo/caos no restauro el circuit breaker")

metricas = estado.get("metricas") or {}
genetico = estado.get("genetico") or {}
eventos = estado.get("eventosEjecucion") or []
if metricas.get("operaciones", 0) <= 0:
    errors.append("estado no contiene operaciones despues de mercado_rentable")
if metricas.get("utilidadAcumuladaUsd", 0) <= 0:
    errors.append("PnL simulado no es positivo despues de mercado_rentable")
if not genetico.get("activo"):
    errors.append("GA no quedo activo despues del smoke")
if not any(str(e.get("tipo", "")).startswith("demo") for e in eventos):
    errors.append("no hay eventos demo visibles en estado")
if metricas.get("rebalanceosTotales", 0) <= 0:
    errors.append("no hay rebalanceos visibles despues de demo rebalanceo")
if not estado.get("auditoriaDecisiones"):
    errors.append("estado no contiene auditoria de decisiones")
trazas = estado.get("trazasEjecucion") or []
if not any(t.get("estado") == "RECONCILED" and abs(t.get("exposicionBtc", 1)) < 1e-9 for t in trazas):
    errors.append("estado no contiene FSM de segunda pierna reconciliada sin exposicion")
if not any(o.get("parcial") for o in estado.get("operaciones", []) + estado.get("oportunidades", [])):
    errors.append("estado no contiene evidencia de fill parcial")

estado_jurado = jurado.get("estado") or {}
if estado_jurado.get("status") != "ready":
    errors.append("/api/jurado no quedo listo despues de demo/final")
if not jurado.get("scorecard") or len(jurado.get("scorecard") or []) < 8:
    errors.append("/api/jurado no expone scorecard suficiente")
if jurado.get("enlaces", {}).get("demoFinal") != "/api/demo/final":
    errors.append("/api/jurado no enlaza demoFinal")
if jurado.get("enlaces", {}).get("demoCaos") != "/api/demo/caos":
    errors.append("/api/jurado no enlaza demoCaos")

rubrica = paquete.get("rubricaOficialComite") or []
if len(rubrica) != 5:
    errors.append("/api/paquete-evaluacion no incluye 5 criterios oficiales")
campos_rubrica = {
    "criterio", "pesoPct", "estado", "preguntaComite",
    "evidenciaActual", "siguienteMovimientoDemo",
}
for item in rubrica:
    faltantes = campos_rubrica.difference(item)
    if faltantes:
        errors.append(f"criterio oficial sin contrato completo: {sorted(faltantes)}")
    if not item.get("criterio") or not item.get("evidenciaActual"):
        errors.append("criterio oficial sin nombre o evidencia verificable")
if sum((item.get("pesoPct", 0) for item in rubrica)) != 100:
    errors.append("los pesos de la rubrica oficial no suman 100%")

evidencia_paquete = paquete.get("evidencia") or {}
metricas_paquete = evidencia_paquete.get("metricas") or {}
if metricas_paquete.get("operaciones", 0) <= 0 or metricas_paquete.get("pnlUsd", 0) <= 0:
    errors.append("paquete no conserva evidencia de operaciones y PnL demo positivo")
if not evidencia_paquete.get("ultimaAuditoria") or not evidencia_paquete.get("ga"):
    errors.append("paquete no incluye auditoria de decision y estado GA")
if not paquete.get("huellaAuditoria"):
    errors.append("paquete no incluye huella de auditoria")
if not str(paquete.get("packageSha256", "")).startswith("sha256:"):
    errors.append("paquete no incluye packageSha256")

recomendaciones = paquete.get("recomendacionesParaGanar") or []
if not recomendaciones or "Estado listo" not in recomendaciones[0]:
    errors.append("paquete no quedo en recomendacion final de estado listo")

if not resumen.get("persistencia", {}).get("activa"):
    errors.append("/api/resumen-llm no reporta persistencia activa")

mcp_tools = {tool.get("name") for tool in mcp_manifest.get("tools", [])}
if "summarize_for_llm" not in mcp_tools or "prepare_demo_final" not in mcp_tools or "jury_mode" not in mcp_tools:
    errors.append("/api/mcp/manifest no expone herramientas clave")
if mcp_summary.get("ok") is not True or not mcp_summary.get("result", {}).get("resumen"):
    errors.append("/api/mcp/call summarize_for_llm no devolvio resumen")

comparacion = backtest.get("comparacion") or {}
if comparacion.get("ganador") not in {"base", "optimizada"}:
    errors.append("/api/backtest no reporta ganador valido")
if backtest.get("base", {}).get("rutasEvaluadas", 0) <= 0:
    errors.append("/api/backtest no evaluo rutas")

if lab.get("tipo") != "research_lab_sweep" or len(lab.get("resultados") or []) < 4:
    errors.append("/api/lab/sweep no devolvio sweep completo")
if not lab.get("ganador"):
    errors.append("/api/lab/sweep no reporta ganador")

for key in ["datasetHash", "configHash"]:
    if not str(version.get(key, "")).startswith("sha256:"):
        errors.append(f"/api/version no expone {key} canónico")
if not version.get("schemaVersion") or not version.get("evidenceSessionId"):
    errors.append("/api/version no vincula schema y sesión de evidencia")

if economics.get("available") is not True:
    errors.append("/api/research/economics no quedó disponible")
if len((economics.get("edgeWaterfall") or {}).get("items") or []) < 7:
    errors.append("economics no expone waterfall completo")
if len((economics.get("capacityCurve") or {}).get("points") or []) < 6:
    errors.append("economics no expone curva de capacidad")

if not (
    execution_matrix.get("allPassed") is True
    and execution_matrix.get("passed") == 12
    and execution_matrix.get("total") == 12
):
    errors.append("matriz forense no concilia los 12 escenarios")

ledger_checks = ledger_audit.get("checks") or {}
if not ledger_checks or not all(ledger_checks.values()):
    errors.append("auditoría de ledger no dejó todos sus invariantes en verde")

filas_sensibilidad = sensibilidad.get("resultados") or []
if (
    len(filas_sensibilidad) != 7
    or "24 semillas holdout" not in sensibilidad.get("metodologia", "")
    or sensibilidad.get("sinFugaHoldout") is not True
    or sensibilidad.get("seleccionAntesHoldout") is not True
    or len(sensibilidad.get("semillasEntrenamiento") or []) != 24
    or len(sensibilidad.get("semillasHoldoutNoVistas") or []) != 24
):
    errors.append("/api/ga/sensibilidad no expone las 7 configuraciones y metodología holdout")
for fila in filas_sensibilidad:
    if fila.get("runs") != 24 or not fila.get("modelo") or not fila.get("config"):
        errors.append("/api/ga/sensibilidad contiene una configuración incompleta")
        break

for key in ["operaciones", "eventosEjecucion", "trazasEjecucion", "auditoriaDecisiones", "rebalanceos", "balances", "configuracion", "telemetriaPipeline"]:
    if key not in export_json:
        errors.append(f"/api/export/json no incluye {key}")
if "tipo,tiempo,ruta,detalle,cantidad_btc" not in export_csv.splitlines()[0]:
    errors.append("/api/export/csv no incluye header esperado")
if "operacion," not in export_csv:
    errors.append("/api/export/csv no incluye operaciones")

if demo_liquidez.get("ok") is not True:
    errors.append("/api/demo liquidez_insuficiente fallo")
if demo_circuit.get("ok") is not True:
    errors.append("/api/demo circuit_breaker fallo")
eventos_adversos = estado_adverso.get("eventosEjecucion") or []
tipos_adversos = {str(e.get("tipo", "")) for e in eventos_adversos}
if "liquidez_insuficiente" not in tipos_adversos:
    errors.append("demo liquidez_insuficiente no dejo evento visible")
if "circuit_breaker" not in tipos_adversos:
    errors.append("demo circuit_breaker no dejo evento visible")
if not estado_adverso.get("metricas", {}).get("circuitBreakerActivo"):
    errors.append("circuit_breaker demo no activo metrica de riesgo")

if demo_restaurada.get("ok") is not True:
    errors.append("/api/demo/final no restauro el sistema despues de probar adversidad")
readiness_final = preflight_final.get("judgeReadiness") or {}
if readiness_final.get("status") != "ready":
    errors.append("el smoke no dejo judgeReadiness=ready al terminar")
if not preflight_final.get("listo"):
    errors.append("el smoke no dejo preflight listo=true al terminar")
if any(check.get("ok") is not True for check in readiness_final.get("checks") or []):
    errors.append("el smoke dejo checks de readiness incompletos al terminar")
if readiness_final.get("passed") != 12 or readiness_final.get("total") != 12:
    errors.append("el smoke no terminó con preflight exacto 12/12")
matrix_final = readiness_final.get("executionMatrix") or {}
if not (
    matrix_final.get("passed") == 12
    and matrix_final.get("total") == 12
    and matrix_final.get("allPassed") is True
):
    errors.append("el preflight final no vinculó la matriz determinista 12/12")
persistencia_final = preflight_final.get("persistencia") or {}
if persistencia_final.get("queueDropped", 0) or persistencia_final.get("queueFailed", 0):
    errors.append("la persistencia final reportó escrituras descartadas o fallidas")

if errors:
    print("Smoke fallido:")
    for error in errors:
        print(f"- {error}")
    sys.exit(1)

print("Smoke OK")
print(f"- readiness inicial: {readiness.get('status')} ({readiness.get('passed')}/{readiness.get('total')})")
print(f"- operaciones: {metricas.get('operaciones')} | PnL: {metricas.get('utilidadAcumuladaUsd'):.2f} USD")
print(f"- GA generacion: {genetico.get('generacion')} | activo: {genetico.get('activo')}")
print(f"- rebalanceos: {metricas.get('rebalanceosTotales')}")
print(f"- paquete verificable | huella: {paquete.get('huellaAuditoria')}")
print(f"- lab ganador: {lab.get('ganador')} | export CSV bytes: {len(export_csv)}")
print(f"- estado final: {readiness_final.get('status')} ({readiness_final.get('passed')}/{readiness_final.get('total')})")
PY
