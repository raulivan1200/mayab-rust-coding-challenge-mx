# Top-Tier Upgrade Checklist

Tracking the upgrade of Mayab BTC Arbitrage from "excellent challenge delivery" to "top 1-2 serious project".

> **Status note (2026-07-12):** this is a roadmap, not a declaration that every
> unchecked item is missing from the product. Checked items below are backed by
> the repository; unchecked items remain post-challenge work or still require a
> public CI/release run. The authoritative delivery evidence is
> [`FINAL_EVIDENCE.md`](../FINAL_EVIDENCE.md).

## P0 — Security (Production Deployment)

### P0.1 Route Classification
- [ ] Classify all routes as `public_read`, `public_demo_sandbox`, `admin_mutation`, `internal_observability`
- [ ] Centralize classification in HTTP architecture
- [ ] No mutable endpoints mixed with public routes
- [ ] **Files**: `src/server.rs` → `src/http/routes/*`, `src/http/auth.rs`

### P0.2 ADMIN_TOKEN Required in Production
- [ ] `MAYAB_ENV=production` requires non-empty `ADMIN_TOKEN` with minimum length
- [ ] Server fails to start if missing in production
- [ ] Dev/test allow explicit insecure config (documented)
- [ ] Token never logged or partially exposed
- [ ] Constant-time comparison where available
- [ ] Accept `Authorization: Bearer <token>` (primary) and `X-Admin-Token` (compat)
- [ ] **Tests**: prod without token fails, prod with token starts, 401 without token, 401 wrong token, token works, token not in logs
- [ ] **Files**: `src/config.rs`, `src/http/auth.rs`, `src/main.rs`

### P0.3 Protect All Mutations
- [ ] All state-mutating endpoints require auth
- [ ] Move admin mutations to `/admin/*` namespace
- [ ] Keep temporary aliases for old routes (deprecated, same auth)
- [ ] Public demo endpoints: isolated session, no global state, no secrets, rate limited, tested for isolation
- [ ] **Files**: `src/http/routes/admin.rs`, `src/http/routes/demo.rs`, `src/http/auth.rs`

### P0.4 Origin Validation (ALLOWED_ORIGINS)
- [ ] Exact allowlist comparison (scheme, host, port normalized)
- [ ] No `ends_with` or wildcard in production
- [ ] Apply to WebSocket, mutable endpoints, CORS, admin forms
- [ ] 403 on denied origin
- [ ] Dev: reasonable localhost config
- [ ] Prod: no wildcard
- [ ] **Files**: `src/http/origin.rs`, `src/http/router.rs`

### P0.5 Rate Limiting
- [ ] Configurable limits per route class
- [ ] Public routes: reasonable general limit
- [ ] Auth/mutations: strict limit
- [ ] WebSocket creation: per-IP limit
- [ ] Max body size, read timeout, handler timeout, concurrency limit for expensive ops
- [ ] No auto-trust `X-Forwarded-For` without explicit proxy config
- [ ] **Tests**: 429 response, recovery after window, public vs admin separation, oversized body rejected
- [ ] **Files**: `src/http/rate_limit.rs`, `src/http/router.rs`

### P0.6 Security Headers & Observability
- [ ] Content-Security-Policy
- [ ] X-Content-Type-Options
- [ ] X-Frame-Options / frame-ancestors
- [ ] Referrer-Policy
- [ ] Permissions-Policy
- [ ] HSTS (when HTTPS)
- [ ] Cache-Control for sensitive APIs
- [ ] `/metrics` exposure controlled by `METRICS_PUBLIC=false` (secure default in prod)
- [ ] **Files**: `src/http/router.rs`, `src/config.rs`
- [ ] **Doc**: `docs/SECURITY_MODEL.md` (threat model, routes, simulation limits, tokens, rate limiting, Origin/CORS, persisted data, remaining risks)

## P0 — Architectural Refactor

### P0.7 HTTP Module Structure
```
src/http/
  mod.rs
  router.rs
  auth.rs
  origin.rs
  rate_limit.rs
  error.rs
  dto.rs
  routes/
    health.rs
    state.rs
    websocket.rs
    admin.rs
    exports.rs
    demo.rs
    jury.rs
    metrics.rs
```
- [ ] Declarative, auditable router
- [ ] DTOs separated from domain
- [ ] Centralized HTTP errors
- [ ] Reusable middleware
- [ ] Small handlers
- [ ] Per-route tests
- [ ] Explicit dependencies
- [ ] No financial logic in handlers
- [ ] **Files**: New modules under `src/http/`

### P0.8 Engine Module Structure
```
src/
  domain/
    opportunity.rs
    quote.rs
    costs.rs
    execution.rs
    wallets.rs
    risk.rs
    rebalance.rs
    audit.rs
  engine/
    mod.rs
    state.rs
    scoring.rs
    analyzer.rs
    executor.rs
    circuit_breaker.rs
    coordinator.rs
  infra/
    persistence/
    metrics/
```
- [ ] Separate: profitability calc, scoring, sizing, partial fills, wallets, rebalance, circuit breaker, stale-feed, audit, persistence, snapshot serialization, concurrency
- [ ] Move code without behavior change first
- [ ] Then functional improvements with tests
- [ ] Cohesive, reviewable files
- [ ] **Files**: New modules under `src/domain/`, `src/engine/`, `src/infra/`

### P0.9 Persistence Off Hot Path
- [ ] Audit if SQLite/exports/serialization blocks hot path
- [ ] Bounded channel + dedicated worker
- [ ] `spawn_blocking` for blocking APIs
- [ ] Policy when queue full (documented, tested)
- [ ] No silent block on decisions
- [ ] Metrics: queue depth, drops, latency
- [ ] Critical events not lost silently
- [ ] **Tests**: backpressure, clean shutdown, full queue, failed persistence, retry/degradation, no deadlocks
- [ ] **Files**: `src/infra/persistence/`, `src/engine/coordinator.rs`

## P0 — Configuration & Parametrization

### ENABLED_EXCHANGES, SYMBOLS
- [ ] Exchange registry by name
- [ ] Unknown name validation
- [ ] Symbol normalization per exchange
- [ ] Exchange config without recompile
- [ ] Fee/withdrawal/reliability/execution profiles
- [ ] Config visible securely in API/UI
- [ ] Secrets excluded from responses
- [ ] Defaults compatible with demo
- [ ] Clear error messages
- [ ] Documented env vars
- [ ] **Files**: `src/config.rs`, `src/mercado.rs`, `src/types.rs`

## P0 — Operational Risk & Adversarial Scenarios

### Policies (guarantee + test)
1. [ ] Halt on required feed stale beyond limit
2. [ ] Halt on net profitability below safety margin during execution
3. [ ] Halt/degrade on excessive wallet skew
4. [ ] Simulated drawdown limit
5. [ ] Authenticated manual kill switch
6. [ ] Single execution per route/resource when needed
7. [ ] Idempotency on retries
8. [ ] Partial fills never exceed liquidity/balances
9. [ ] No negative balances
10. [ ] Rebalance suggestions with estimated cost
11. [ ] Timeout + partial leg failure
12. [ ] Market moves against during execution
13. [ ] Feed disconnected/degraded
14. [ ] Persistence unavailable without killing ingestion

### Halt States
- [ ] Explicit, auditable, visible in API, visible in UI
- [ ] With reason, safely recoverable, tested

## P0 — CI & Supply Chain

### Quality
- [x] `cargo fmt --all -- --check`
- [x] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [x] `cargo test --workspace --all-targets --locked`
- [x] `cargo build --workspace --release --locked`
- [ ] `--all-features` when possible, separate compile for external deps

### Security
- [ ] CodeQL for supported languages
- [ ] Dependency Review on PRs
- [ ] Dependabot for Cargo + GitHub Actions
- [ ] `cargo audit`
- [x] `cargo deny`
- [x] Secret scanning
- [x] SBOM (CycloneDX)
- [x] Minimal permissions per job
- [ ] Official/trusted actions, pinned versions
- [ ] No secrets in workflows

### Files
- [ ] `.github/workflows/ci.yml`
- [x] `.github/workflows/security.yml`
- [x] `.github/workflows/benchmarks.yml`
- [x] `.github/workflows/release.yml`
- [x] `.github/dependabot.yml`
- [x] `deny.toml`

## P0 — Reproducible Benchmarks

- [ ] Criterion benchmarks
- [ ] Measure separately: parse/normalize, scoring, net profitability, sizing/partial fill, wallet updates, risk checks, snapshot serialization, audit write/enqueue, replay pipeline, throughput (exchanges, symbols)
- [ ] Distinguish: hot path, e2e latency, network, persistence, serialization, WS delivery, browser render
- [ ] `BENCHMARKING.md` with: hardware, OS, Rust version, commit, date, commands, dataset, sample size, p50/p95/p99, throughput, memory, limitations, reproduction
- [ ] `benches/` + `scripts/run-benchmarks.sh`
- [ ] No `target/criterion` in repo
- [ ] CI benchmark: manual/scheduled/informational/CodSpeed (not blocking PRs)

## P1 — Advanced Testing

- [ ] Unit, integration, property tests, concurrency tests
- [ ] `proptest` where appropriate
- [ ] **Invariants**: net ≤ gross, costs ≥ 0, size ≤ liquidity, size ≤ BTC balance, cost ≤ fiat balance, final balances ≥ 0, rejected op = no wallet change, partial fill = exact volume, stale quote = no exec, circuit breaker = no exec, rebalance preserves book value, serialization round-trip, idempotent events, no concurrent incompatible ops, clean shutdown, reconnect with backoff
- [ ] **HTTP tests**: auth, Origin, rate limit, body limit, headers, invalid JSON, stable errors, endpoint compat, WS allow/deny, server shutdown, healthz, metrics, exports, preflight, demo, jury routes
- [ ] No long sleeps; injected clock, controlled pauses, Tokio time

## P1 — Useful Observability

- [ ] Metrics per stage: ingestion, normalization, decision, risk checks, persistence, serialization, WS broadcast, events/sec, reconnections, stale feeds, opportunities detected, rejected by reason, simulated executions, partial fills, circuit breaker activations, audit queue depth/drops, wallet skew, rebalance suggestions
- [x] Histograms with reasonable buckets
- [x] No unbounded cardinality labels
- [x] Metrics docs + example queries

## P1 — README & Documentation

### README Order
1. [ ] Title + one-line pitch
2. [x] Real workflow badges
3. [ ] Demo
4. [ ] 3-5 min Quick Start
5. [ ] Problem solved
6. [ ] Verifiable evidence
7. [ ] Architecture
8. [ ] Net profitability model
9. [ ] Exchanges & symbols
10. [ ] Wallet & risk management
11. [ ] Benchmarks
12. [ ] Observability
13. [ ] Security
14. [ ] Local operation
15. [ ] Docker & Cloud Run
16. [ ] Configuration
17. [ ] Endpoints
18. [ ] Testing
19. [ ] How to add an exchange
20. [ ] Roadmap
21. [ ] Contribution
22. [ ] Jury appendix

### Docs
- [x] `docs/ARCHITECTURE.md`
- [x] `docs/SECURITY_MODEL.md`
- [x] `docs/ADDING_EXCHANGE.md`
- [x] `docs/OPERATIONS.md`
- [x] `docs/DESIGN_DECISIONS.md`
- [x] `docs/TOP_TIER_UPGRADE.md` (this file)

### Specific Topics
- [ ] USD/USDT separation
- [ ] Stale guard
- [ ] Partial fills
- [ ] Circuit breaker
- [ ] Rebalance
- [ ] Persistence
- [ ] Replay
- [ ] Financial precision decisions
- [ ] Concurrency model
- [ ] Simulation limits
- [ ] Paper vs hypothetical live differences

## P1 — Community & Maintenance

- [x] `CONTRIBUTING.md` (setup, toolchain, commands, arch, fmt, lint, test, bench, add exchange, commit/PR policy)
- [x] `SECURITY.md` (supported versions, responsible disclosure channel, what not to publish, paper-only model, expected response)
- [x] `CODE_OF_CONDUCT.md`
- [x] `CHANGELOG.md` (Keep a Changelog + SemVer, start with Unreleased, reconstruct verifiable from Git)
- [x] `.github/ISSUE_TEMPLATE/bug_report.yml`
- [x] `.github/ISSUE_TEMPLATE/feature_request.yml`
- [x] `.github/ISSUE_TEMPLATE/config.yml`
- [x] `.github/pull_request_template.md`
- [ ] GitHub labels + topics (commands provided if no auth)

## P1 — Releases & Packaging

- [x] Release workflow on SemVer tags
- [x] Targets: Linux x86_64, macOS, Windows
- [x] Archives + SHA-256 checksums
- [x] SBOM
- [x] Release notes
- [x] Artifact provenance/attestations
- [ ] Docker tags: version, SHA, `latest` for stable only
- [x] Native runners when more reliable than cross-compile
- [x] Workflow: test → build → checksums → SBOM → provenance → publish on valid tag only
- [x] No real release without authorization

## P2 — Operator Console

- [x] `/operator` (separate from premium dashboard)
- [x] Reuse APIs/styles, no heavy framework
- [ ] Shows: exchange health, stale feeds, best opportunity, gross/net profit, fees, slippage, latency risk, p50/p95/p99 per stage, circuit breaker, wallet skew, rebalance suggestions, audit queue depth, last simulated op
- [x] Fast load, responsive, keyboard nav, contrast, empty states, no fake data, mutations hidden/blocked without auth, explicit confirm for admin actions

## P2 — SEO & Presentation

- [ ] Suggested description, topics
- [x] Open Graph/social preview if mechanism exists
- [x] Optimized screenshots
- [ ] Short lightweight GIF if material exists
- [ ] "Why Mayab stands out" table with evidence links
- [ ] Valid internal links
- [x] Working workflow badges
- [x] No fake stars/coverage/versions/workflow status

---

## Code Quality Standards

- [ ] `thiserror` for domain/library errors
- [ ] `anyhow` at application boundaries
- [ ] Structured `tracing`
- [ ] Cancellation with `CancellationToken` or equivalent
- [ ] Bounded channels
- [ ] Explicit timeouts
- [ ] Clean shutdown
- [ ] No locks during I/O
- [ ] Minimal lock scope
- [ ] Avoid large clones on hot path
- [ ] Avoid unnecessary repeated serialization
- [ ] Small interfaces for persistence, clock, sources
- [ ] Deterministic mocks/fakes
- [ ] Config validated at startup
- [ ] Secrets with appropriate types/wrappers
- [ ] Stable HTTP errors without internal details
- [ ] Docs for non-obvious decisions

---

## Final Validation Commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo build --workspace --release --locked
cargo audit
cargo deny check
cargo llvm-cov --workspace --all-targets --lcov --output-path lcov.info
cargo bench
git diff --check
```

Plus (when viable):
```bash
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --all-features --locked
```

Also:
- Frontend validation
- Docker build
- Local smoke test
- Healthcheck
- Production start without token (should fail)
- Production start with token (should work)
- Auth + unauth requests
- Allowed + rejected Origin
- Rate limit tests
- WS tests
- Secret review
- Generated file review

---

## Commit Strategy

Atomic groups:
- `security: enforce production authentication and route protection`
- `refactor: split HTTP server into auditable route modules`
- `refactor: split arbitrage engine into domain services`
- `perf: move audit persistence out of decision hot path`
- `ci: add hardened quality and supply-chain checks`
- `test: add auth property concurrency and risk tests`
- `perf: add reproducible stage benchmarks and metrics`
- `docs: reposition project and add maintainer documentation`
- `release: add reproducible multi-platform packaging`
- `feat: add minimal operator console`

---

## Completion Criteria

### P0 Complete When:
- [ ] Prod won't start without token
- [ ] All global mutations protected
- [ ] Origin + rate limiting tested
- [ ] HTTP + engine not monolithic (or justified)
- [ ] Blocking persistence off hot path (when applicable)
- [ ] CI has quality + security
- [ ] Supply chain covered
- [ ] Benchmarks reproducible
- [ ] Critical tests pass
- [ ] Demo still works

### P1 Complete When:
- [ ] Useful critical module coverage
- [ ] Property + concurrency tests pass
- [ ] Per-stage metrics exposed
- [ ] README reordered
- [ ] Technical + security docs exist
- [ ] Community files exist
- [ ] Release workflow builds
- [ ] No false claims

### P2 Complete When:
- [ ] P0 + P1 green
- [ ] Operator Console uses real data
- [ ] Packaging works
- [ ] Premium dashboard not degraded
- [ ] Docs reflect implementation
