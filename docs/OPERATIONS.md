# Operations

## Local start

```bash
cargo run
curl -fsS http://127.0.0.1:8080/healthz
curl -fsS http://127.0.0.1:8080/readyz
```

Use `RUST_LOG=debug cargo run` for backend diagnosis. Browser instrumentation is opt-in through `/?debug=1` or `localStorage.mayabDebug=1`.

## Operator surfaces

- `/operator`: read-only console backed by real `/api/estado` data.
- `/healthz`: process liveness; use for restart decisions.
- `/readyz`: dependencies and feed readiness; use to remove an instance from traffic.
- `/metrics`: Prometheus exposition.
- `/api/preflight`: demo and evaluation readiness.

Useful PromQL:

```promql
histogram_quantile(0.95, sum by (le, etapa) (rate(mayab_stage_duration_ms_bucket[5m])))
sum by (etapa) (rate(mayab_stage_events_total[1m]))
rate(mayab_http_requests_total{status=~"5.."}[5m])
mayab_feeds_conectados
mayab_circuit_breaker
```

Histograms use bounded stage names and millisecond buckets: 0.1, 0.5, 1, 2.5, 5, 10, 25, 50, 100 and 500. Never add symbol, operation ID or error text labels.

## Incident triage

1. Check `/healthz`, then `/readyz` and `/operator`.
2. Confirm feed count, quote freshness, circuit breaker and risk state.
3. Inspect error rate and p95 stage latency. A healthy process with zero feeds is not ready.
4. Verify `persistencia.storagePersistent=true`, `storageStatus=persistent`, `queueDropped=0` and `queueFailed=0`; export `/api/export/json` before restarting local ephemeral instances.
5. Restart only after capturing logs and state; never interpret a reset metric as recovery proof.

Before publishing evidence, run
`OUT_DIR=artifacts/evidence/<revision> ./scripts/generar-evidencia.sh`. The script
starts a clean `/api/demo/final` run (pass `ADMIN_TOKEN` outside Jury Mode),
downloads into a staging directory, rejects anything other than preflight
12/12, matrix 12/12, a fully reconciled ledger and zero persistence loss, then
publishes atomically with a verified `SHA256SUMS` file.

## Deploy and rollback

Initialize `scripts/timescaledb/schema.sql`, configure `ADMIN_TOKEN_SECRET` and `DATABASE_URL_SECRET`, then deploy with `./scripts/deploy-cloud-run.sh` using an immutable image. The smoke requires the TimescaleDB backend and durable storage. Roll back by deploying the previous immutable image digest, then repeat the smoke. CI and deploy gates live in `.github/workflows/rust.yml`.
