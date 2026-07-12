#!/usr/bin/env sh
set -eu

BASE_URL="${BASE_URL:-http://localhost:8080}"
OUT_DIR="${OUT_DIR:-artifacts/evidence/$(date -u +%Y%m%dT%H%M%SZ)}"
mkdir -p "$OUT_DIR"

echo "Generando snapshot de evidencia sellada desde $BASE_URL..."

# 1. Paquete de evaluación (Scorecard, GA, estado actual)
curl -fsS "$BASE_URL/api/paquete-evaluacion" -o "$OUT_DIR/paquete-evaluacion.json"
echo "✅ Paquete de evaluación guardado."

# 2. Export completo de auditoría (operaciones, eventos, rebalanceos, GA)
curl -fsS "$BASE_URL/api/export/json" -o "$OUT_DIR/auditoria-completa.json"
echo "✅ Auditoría JSON guardada."

# 3. Export CSV
curl -fsS "$BASE_URL/api/export/csv" -o "$OUT_DIR/auditoria-completa.csv"
echo "✅ Auditoría CSV guardada."

# 4. Benchmark de Latencias (Pipeline y Exchange)
curl -fsS "$BASE_URL/api/latencias" -o "$OUT_DIR/benchmark-latencias.json"
echo "✅ Benchmark de latencias guardado."

# 5. Evidencia experimental separada por metodología.
curl -fsS "$BASE_URL/api/backtest" -o "$OUT_DIR/backtest.json"
curl -fsS "$BASE_URL/api/ga/sensibilidad" -o "$OUT_DIR/ga-sensibilidad.json"
curl -fsS "$BASE_URL/api/research/bootstrap" -o "$OUT_DIR/bootstrap.json"
curl -fsS "$BASE_URL/api/research/out-of-sample" -o "$OUT_DIR/out-of-sample.json"
curl -fsS "$BASE_URL/api/research/microstructure" -o "$OUT_DIR/microestructura.json"
curl -fsS "$BASE_URL/api/preflight" -o "$OUT_DIR/preflight.json"
echo "✅ Backtest, holdout, bootstrap, sensibilidad y preflight guardados."

# 6. Generar un manifiesto de procedencia. Los resultados conservan su
# clasificación de origen; este script no convierte replay/demo en datos reales.
cat <<EOF > "$OUT_DIR/manifest.json"
{
  "schemaVersion": 2,
  "timestamp": "$(date -u +"%Y-%m-%dT%H:%M:%SZ")",
  "commit": "$(git rev-parse HEAD 2>/dev/null || echo 'unknown')",
  "dirtyWorktree": $(if git diff --quiet && git diff --cached --quiet; then echo false; else echo true; fi),
  "origen": "$BASE_URL",
  "clasificacion": {
    "feeds": "mercado_publico",
    "ordenes": "simuladas",
    "demo": "sintetica_etiquetada",
    "capitalRealUsd": 0
  },
  "reproduccion": "POST /api/demo/final; GET /api/paquete-evaluacion"
}
EOF
echo "✅ Manifiesto generado."

# 7. Sellar cada artefacto. Soporta GNU/Linux y macOS.
if command -v sha256sum >/dev/null 2>&1; then
  (cd "$OUT_DIR" && sha256sum ./*.json ./*.csv > SHA256SUMS)
else
  (cd "$OUT_DIR" && shasum -a 256 ./*.json ./*.csv > SHA256SUMS)
fi
echo "✅ Checksums SHA-256 generados."

echo ""
echo "=========================================================="
echo "🎯 Snapshot de evidencia generado en: $OUT_DIR/"
echo "Puedes empaquetar o hacer commit de este directorio para"
echo "probar que el motor funciona más allá del /tmp efímero."
echo "=========================================================="
