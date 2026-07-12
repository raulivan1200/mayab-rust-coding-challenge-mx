# Runbook

## Deploy local

```bash
docker compose up --build
curl http://localhost:8080/healthz
```

## Deploy Cloud Run

```bash
export PROJECT=mi-proyecto
./scripts/deploy-cloud-run.sh
```

Validación automática: `/healthz`, `/api/preflight`, `/api/resumen-llm`, `/`, `/app.js`, `/styles.css`.

## Debug

```bash
RUST_LOG=debug cargo run
```

Dashboard: `http://127.0.0.1:8080/?debug=1` (logs de consola + performance observers).

## Comandos útiles

En desarrollo local `ADMIN_TOKEN` es opcional. Para reproducir exactamente el comportamiento de producción, arranca el servidor con un token de al menos 32 caracteres y exporta el mismo valor en la terminal de operación:

```bash
export ADMIN_TOKEN='cambia-este-token-local-de-32-chars'
```

```bash
# Demo rentable
curl -sS -X POST http://localhost:8080/api/demo \
  -H 'Content-Type: application/json' \
  -H "Authorization: Bearer ${ADMIN_TOKEN}" \
  -d '{"escenario":"mercado_rentable"}'

# GA
curl -sS -X POST http://localhost:8080/api/ga/evolucionar \
  -H 'Content-Type: application/json' \
  -H "Authorization: Bearer ${ADMIN_TOKEN}" \
  -d '{"usarReplaySiVacio":true,"muestras":96}'

# Ver estado
curl -sS http://localhost:8080/api/estado | jq '.metricas.utilidadAcumuladaUsd'

# Backtest
curl -sS http://localhost:8080/api/backtest | jq '.comparacion.deltaPnlUsd'

# Preflight (para jurado)
curl -sS http://localhost:8080/api/preflight | jq '.scorecardFinal'

# Lab sweep
curl -sS http://localhost:8080/api/lab/sweep | jq '.ganador'
```

## Logs (Cloud Run)

```bash
gcloud logging read "resource.type=cloud_run_revision AND resource.labels.service_name=mayab-btc-arbitrage" --limit 50
```

## Reset

```bash
curl -sS -X POST http://localhost:8080/api/demo/reset \
  -H "Authorization: Bearer ${ADMIN_TOKEN}"
rm -f /tmp/mayab-auditoria.sqlite  # borra auditoría local
```

## Interpretar Preflight

El scorecard final incluye:

- **coberturaFuncional**: % de endpoints contratados que responden
- **exchangesIntegrados**: 10/10 exchanges con cotización reciente
- **persistenciaSqlite**: auditoría activa
- **gaFuncional**: GA ha publicado campeón
- **ejecucionSimulada**: operaciones simuladas en el historial
- **pnlPositivo**: utilidad acumulada > 0
- **exhibeFillParcial**: evidencia forense de fill parcial
- **exhibeCircuitBreaker**: evidencia de circuit breaker

## Seguridad

- Sin API keys de exchanges reales
- Ejecución limitada al simulador en memoria; no se aceptan llaves API ni órdenes privadas.
- Sin secretos en logs, sin endpoints de modificación sin autenticación
- Distroless runtime (sin shell, sin package manager)
- Non-root user en contenedor
