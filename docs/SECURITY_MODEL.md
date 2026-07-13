# Security model

## Scope and assets

Mayab consumes public market data and simulates trades. It has no exchange credentials, signing keys, custody, deposits, withdrawals, or on-chain transfers. The protected assets are service availability, configuration integrity, audit evidence, and truthful separation between live market data and synthetic P&L.

## Trust boundaries

- Exchange frames and HTTP clients are untrusted input.
- Read endpoints expose public demo state; mutable endpoints change shared simulated state only.
- `ADMIN_TOKEN`, when configured, protects mutations through `Authorization: Bearer` or `X-Admin-Token`. Never put it in a URL or commit it.
- `WEBHOOK_URL` is private configuration because providers often embed credentials in the URL; it is never serialized into public state, WebSocket snapshots or exports.
- `DATABASE_URL` is injected from Secret Manager, requires TLS by default and is always represented publicly as `timescaledb://[redacted]`.
- `MAYAB_JUDGE_MODE=true` is an explicit evaluation-only exception: it makes only `/api/demo/reset`, `/api/demo/final`, and `/api/demo/caos` public. These deterministic routes mutate simulated in-memory state and remain rate-limited; configuration, exchange toggles, arbitrary wallet/risk changes, GA controls, capture, and MCP mutations still require `ADMIN_TOKEN`.
- Browser Origin checks are defense in depth, not authentication.
- `/metrics` can reveal operational detail and should be ingress-restricted in production.

## Controls

- Strict JSON deserialization, request limits, timeouts, rate limiting, HSTS
  and a self-hosted-asset CSP belong at the HTTP boundary.
- USD and USDT remain separate unless explicitly configured, preventing false cross-currency profit.
- Stale data, circuit breaker, inventory and single-flight checks can reject simulated execution.
- Demo events are labeled synthetic and real execution remains `false` in public state.
- Audit stores can contain market and operational evidence but no secrets or personal data.
- A full persistence queue or a backend write failure is fail-visible: counters remain monotonic, `storageStatus` becomes `degraded`, and evaluation readiness is blocked until a clean process is restored.

## Deployment guidance

Use a strong `ADMIN_TOKEN`, exact `ALLOWED_ORIGINS`, HTTPS, least-privilege Cloud Run identity, restricted `/metrics`, immutable image tags and `MIN_INSTANCES=1` only when latency warrants its cost. The production deploy requires durable TimescaleDB and fails closed when its schema or connection is unavailable.

The production image runs as `nonroot`, handles `SIGTERM`, keeps application
files root-owned and excludes source documentation/scripts from the runtime
layer. The Compose profile additionally uses a read-only root filesystem,
`no-new-privileges`, no Linux capabilities and bounded tmpfs storage.

## Remaining risks

The demo is not a hardened trading platform. Shared simulated state permits authorized users to affect each other's demo, in-process metrics reset on restart, public exchange data may be wrong, and local SQLite is not an immutable external ledger. Adding real execution requires a new threat model, secret manager, scoped exchange permissions, approval controls, reconciliation and regulatory review.

Report vulnerabilities through [SECURITY.md](../SECURITY.md); do not open a public issue containing exploit details.
