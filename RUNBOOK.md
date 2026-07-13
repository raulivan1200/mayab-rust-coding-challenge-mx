# Runbook

## Deploy local

```bash
docker compose up --build
curl http://localhost:8080/healthz
```

## Deploy Cloud Run

```bash
export PROJECT=mi-proyecto
export RUNTIME_SERVICE_ACCOUNT=mayab-runtime@mi-proyecto.iam.gserviceaccount.com
export ADMIN_TOKEN_SECRET=mayab-admin-token:latest
export DATABASE_URL_SECRET=mayab-database-url:latest
./scripts/deploy-cloud-run.sh
```

Inicializa una sola vez el secreto de base de datos antes del deploy:

```bash
psql -v ON_ERROR_STOP=1 "$DATABASE_URL" -f scripts/timescaledb/schema.sql
printf '%s' "$DATABASE_URL" | gcloud secrets versions add mayab-database-url --data-file=-
```

La conexión administrada usa TLS por defecto (`sslmode=require`). `default` y
`prefer` también se elevan a `require`. `sslmode=disable` falla cerrado salvo en
desarrollo con el opt-in exacto `ALLOW_INSECURE_DATABASE=true`; producción lo
rechaza siempre.

Validación automática: `/healthz`, `/readyz`, `/api/version`, `/api/preflight`,
`/api/resumen-llm`, el tape versionado, los exports y los estáticos del dashboard.
En producción, preflight no queda verde si `storagePersistent` es falso.
Cloud Run se despliega con `TRUST_PROXY_HEADERS=false`; no lo habilites sin un
proxy o WAF que elimine encabezados de cliente y reconstruya la cadena confiable.

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

# Preflight (para jurado; deben pasar exactamente 12/12 checks)
curl -sS http://localhost:8080/api/preflight | jq '{listo, modo, judgeReadiness}'

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

`judgeReadiness.checks` contiene doce checks estables. `listo=true` exige que
todos pasen y que el proceso esté operable. Entre ellos están feeds, GA,
ejecución simulada, PnL, fill parcial, circuit breaker, persistencia y una
ejecución de dos piernas `RECONCILED` con residual cero e invariantes de wallet,
ledger, reservas y fills.

## Seguridad

- Sin API keys de exchanges reales
- Ejecución limitada al simulador en memoria; no se aceptan llaves API ni órdenes privadas.
- Sin secretos en logs, sin endpoints de modificación sin autenticación
- Imagen runtime mínima basada en Debian, con solo certificados y `curl` para healthcheck
- Usuario non-root en contenedor
