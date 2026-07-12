# Security model

## Scope and assets

Mayab consumes public market data and simulates trades. It has no exchange credentials, signing keys, custody, deposits, withdrawals, or on-chain transfers. The protected assets are service availability, configuration integrity, audit evidence, and truthful separation between live market data and synthetic P&L.

## Trust boundaries

- Exchange frames and HTTP clients are untrusted input.
- Read endpoints expose public demo state; mutable endpoints change shared simulated state only.
- `ADMIN_TOKEN`, when configured, protects mutations through `Authorization: Bearer` or `X-Admin-Token`. Never put it in a URL or commit it.
- `MAYAB_JUDGE_MODE=true` is an explicit evaluation-only exception: it makes only `/api/demo/reset`, `/api/demo/final`, and `/api/demo/caos` public. These deterministic routes mutate simulated in-memory state and remain rate-limited; configuration, exchange toggles, arbitrary wallet/risk changes, GA controls, capture, and MCP mutations still require `ADMIN_TOKEN`.
- Browser Origin checks are defense in depth, not authentication.
- `/metrics` can reveal operational detail and should be ingress-restricted in production.

## Controls

- Strict JSON deserialization, request limits, timeouts, rate limiting and security headers belong at the HTTP boundary.
- USD and USDT remain separate unless explicitly configured, preventing false cross-currency profit.
- Stale data, circuit breaker, inventory and single-flight checks can reject simulated execution.
- Demo events are labeled synthetic and real execution remains `false` in public state.
- SQLite audit files can contain market and operational evidence but no secrets or personal data.

## Deployment guidance

Use a strong `ADMIN_TOKEN`, exact `ALLOWED_ORIGINS`, HTTPS, least-privilege Cloud Run identity, restricted `/metrics`, immutable image tags and `MIN_INSTANCES=1` only when latency warrants its cost. Treat `/tmp` as ephemeral and export evidence before instance replacement.

## Remaining risks

The demo is not a hardened trading platform. Shared simulated state permits authorized users to affect each other's demo, in-process metrics reset on restart, public exchange data may be wrong, and local SQLite is not an immutable external ledger. Adding real execution requires a new threat model, secret manager, scoped exchange permissions, approval controls, reconciliation and regulatory review.

Report vulnerabilities through [SECURITY.md](../SECURITY.md); do not open a public issue containing exploit details.
