#!/usr/bin/env sh
set -eu

BASE_URL="${BASE_URL:-http://localhost:8080}"
OUT_DIR="${OUT_DIR:-artifacts/evidence/$(date -u +%Y%m%dT%H%M%SZ)}"
ADMIN_TOKEN="${ADMIN_TOKEN:-}"
COMMIT="$(git rev-parse HEAD 2>/dev/null || printf '%s' unknown)"
if [ -n "$(git status --porcelain --untracked-files=normal 2>/dev/null || true)" ]; then
  DIRTY_WORKTREE=true
else
  DIRTY_WORKTREE=false
fi

if [ -e "$OUT_DIR" ]; then
  echo "El destino ya existe; usa otro OUT_DIR para no sobrescribir evidencia: $OUT_DIR" >&2
  exit 2
fi

STAGE_DIR="${OUT_DIR}.tmp.$$"
cleanup() {
  if [ -n "${STAGE_DIR:-}" ] && [ -d "$STAGE_DIR" ]; then
    rm -rf "$STAGE_DIR"
  fi
}
trap cleanup EXIT INT TERM
mkdir -p "$(dirname "$OUT_DIR")" "$STAGE_DIR"

fetch() {
  path="$1"
  output="$2"
  curl --fail --silent --show-error --location \
    --retry 3 --retry-delay 1 --retry-all-errors \
    --connect-timeout 5 --max-time 120 \
    "$BASE_URL$path" -o "$STAGE_DIR/$output"
}

echo "Generando snapshot de evidencia sellada desde $BASE_URL..."

if [ -n "$ADMIN_TOKEN" ]; then
  curl --fail --silent --show-error --location \
    --connect-timeout 5 --max-time 120 \
    -X POST -H "Authorization: Bearer ${ADMIN_TOKEN}" \
    "$BASE_URL/api/demo/final" -o "$STAGE_DIR/demo-final.json"
else
  curl --fail --silent --show-error --location \
    --connect-timeout 5 --max-time 120 \
    -X POST "$BASE_URL/api/demo/final" -o "$STAGE_DIR/demo-final.json"
fi

fetch "/api/paquete-evaluacion" "paquete-evaluacion.json"
fetch "/api/jurado" "jurado.json"
fetch "/api/export/json" "auditoria-completa.json"
fetch "/api/export/csv" "auditoria-completa.csv"
fetch "/api/export/evidence" "evidencia-resumen.md"
fetch "/api/latencias" "benchmark-latencias.json"
fetch "/api/backtest" "backtest.json"
fetch "/api/version" "version.json"
fetch "/api/ga/sensibilidad" "ga-sensibilidad.json"
fetch "/api/research/bootstrap" "bootstrap.json"
fetch "/api/research/walk-forward" "walk-forward.json"
fetch "/api/research/economics" "economics.json"
fetch "/api/research/execution-matrix" "execution-matrix.json"
fetch "/api/research/ledger-audit" "ledger-audit.json"
fetch "/api/research/microstructure" "microestructura.json"
fetch "/api/preflight" "preflight.json"

EVIDENCE_COMMIT="$COMMIT" \
EVIDENCE_DIRTY="$DIRTY_WORKTREE" \
EVIDENCE_BASE_URL="$BASE_URL" \
python3 - "$STAGE_DIR" <<'PY'
import datetime
import json
import os
import pathlib
import sys

root = pathlib.Path(sys.argv[1])

def load(name):
    with (root / name).open(encoding="utf-8") as handle:
        return json.load(handle)

version = load("version.json")
preflight = load("preflight.json")
matrix = load("execution-matrix.json")
ledger = load("ledger-audit.json")
audit = load("auditoria-completa.json")
package = load("paquete-evaluacion.json")
jury = load("jurado.json")
demo = load("demo-final.json")

errors = []
readiness = preflight.get("judgeReadiness") or {}
readiness_checks = readiness.get("checks") or []
runtime_invariants = ((readiness.get("twoLegEvidence") or {}).get("invariants") or {})
matrix_summary = readiness.get("executionMatrix") or {}
matrix_cases = matrix.get("cases") or []
matrix_reports = matrix.get("reports") or []
ledger_checks = ledger.get("checks") or {}
persistence = preflight.get("persistencia") or {}

if not (
    demo.get("ok") is True
    and demo.get("persistenciaDrenada") is True
    and (demo.get("deterministicProof") or {}).get("allPassed") is True
    and str(demo.get("resultSha256", "")).startswith("sha256:")
):
    errors.append("la prueba completa determinista no terminó en verde")

if not (
    preflight.get("listo") is True
    and readiness.get("status") == "ready"
    and readiness.get("passed") == 12
    and readiness.get("total") == 12
    and len(readiness_checks) == 12
    and all(item.get("ok") is True for item in readiness_checks)
):
    errors.append("preflight no está exactamente 12/12")

if not runtime_invariants or not all(value is True for value in runtime_invariants.values()):
    errors.append("la reconciliación adversa runtime no pasa todas sus invariantes")

if not (
    matrix.get("available") is True
    and matrix.get("allPassed") is True
    and matrix.get("passed") == 12
    and matrix.get("total") == 12
    and len(matrix_cases) == 12
    and len(matrix_reports) == 12
    and all(((case.get("invariants") or {}).get("allPassed") is True) for case in matrix_cases)
    and matrix_summary.get("allPassed") is True
    and matrix_summary.get("passed") == 12
    and matrix_summary.get("total") == 12
    and matrix_summary.get("matrixSha256") == matrix.get("matrixSha256")
):
    errors.append("la matriz determinista no concilia exactamente 12/12")

if not (
    ledger.get("allPassed") is True
    and ledger_checks
    and all(value is True for value in ledger_checks.values())
):
    errors.append("la auditoría de ledger no pasa todas sus comprobaciones")

if persistence.get("queueDropped", 0) != 0 or persistence.get("queueFailed", 0) != 0:
    errors.append("la persistencia reporta escrituras descartadas o fallidas")

for key in ("datasetHash", "configHash"):
    if not str(version.get(key, "")).startswith("sha256:"):
        errors.append(f"version.{key} no contiene una huella SHA-256")

session = version.get("evidenceSessionId")
audit_session = (audit.get("provenance") or {}).get("evidenceSessionId")
jury_session = (jury.get("version") or {}).get("evidenceSessionId")
ledger_session = ledger.get("runId")
package_session = (package.get("provenance") or {}).get("evidenceSessionId")
demo_session = (demo.get("provenance") or {}).get("evidenceSessionId")
if not session or len({session, audit_session, jury_session, ledger_session, package_session, demo_session}) != 1:
    errors.append("los artefactos no pertenecen a la misma evidenceSessionId")

if (
    not package.get("huellaAuditoria")
    or not str(package.get("packageSha256", "")).startswith("sha256:")
    or not audit.get("exportSha256")
):
    errors.append("faltan las huellas del paquete o del export de auditoría")

if errors:
    for error in errors:
        print(f"[ERROR] {error}", file=sys.stderr)
    raise SystemExit(1)

assertions = {
    "schemaVersion": 1,
    "allPassed": True,
    "preflight": {"passed": 12, "total": 12},
    "executionMatrix": {
        "passed": 12,
        "total": 12,
        "matrixSha256": matrix.get("matrixSha256"),
    },
    "ledgerChecks": ledger_checks,
    "runtimeInvariants": runtime_invariants,
    "persistence": {
        "drainedBeforeSnapshot": demo.get("persistenciaDrenada") is True,
        "queueDropped": persistence.get("queueDropped", 0),
        "queueFailed": persistence.get("queueFailed", 0),
    },
}
(root / "assertions.json").write_text(
    json.dumps(assertions, ensure_ascii=False, indent=2) + "\n",
    encoding="utf-8",
)

manifest = {
    "schemaVersion": 3,
    "timestamp": datetime.datetime.now(datetime.timezone.utc).isoformat().replace("+00:00", "Z"),
    "commit": os.environ["EVIDENCE_COMMIT"],
    "dirtyWorktree": os.environ["EVIDENCE_DIRTY"] == "true",
    "origin": os.environ["EVIDENCE_BASE_URL"],
    "provenance": version,
    "evidenceSessionId": session,
    "matrixSha256": matrix.get("matrixSha256"),
    "auditExportSha256": audit.get("exportSha256"),
    "packageSha256": package.get("packageSha256"),
    "demoResultSha256": demo.get("resultSha256"),
    "classification": {
        "feeds": "mercado_publico",
        "orders": "simuladas",
        "demo": "sintetica_etiquetada",
        "deterministicMatrix": "sintetica_determinista_etiquetada",
        "capitalRealUsd": 0,
    },
    "verification": {
        "assertions": "assertions.json",
        "checksums": "SHA256SUMS",
        "reproduction": "POST /api/demo/final; GET /api/research/execution-matrix; GET /api/paquete-evaluacion",
    },
}
(root / "manifest.json").write_text(
    json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
    encoding="utf-8",
)
PY
echo "[OK] Preflight, invariantes, ledger, procedencia y sesión conciliados."

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$STAGE_DIR" && sha256sum ./*.json ./*.csv ./*.md > SHA256SUMS)
  (cd "$STAGE_DIR" && sha256sum --check SHA256SUMS >/dev/null)
else
  (cd "$STAGE_DIR" && shasum -a 256 ./*.json ./*.csv ./*.md > SHA256SUMS)
  (cd "$STAGE_DIR" && shasum -a 256 --check SHA256SUMS >/dev/null)
fi
echo "[OK] Checksums SHA-256 generados y verificados."

mv "$STAGE_DIR" "$OUT_DIR"
STAGE_DIR=""
trap - EXIT INT TERM

echo "Snapshot de evidencia publicado atómicamente en: $OUT_DIR/"
