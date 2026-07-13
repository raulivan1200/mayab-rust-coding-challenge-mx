# Architecture

Mayab is one Rust process with explicit boundaries: public market-data adapters feed a simulated decision engine; Axum exposes read APIs, sandbox mutations, Prometheus metrics and static UI. No component can place a real order.

```text
public WS/REST -> normalize quote -> Motor -> risk/decision -> two-leg simulator
                                      |             |              |
                                      v             v              v
                                  GA strategy   simulated wallets  durable audit
                                      \             |              /
                                       Axum + WS -> dashboard/operator
```

## Runtime flow

1. `mercado.rs` parses exchange-specific frames into `Cotizacion`.
2. `Motor::recibir_cotizacion` maintains fresh books and `Motor::analizar` evaluates compatible USD or USDT lanes.
3. Cost, depth, inventory, latency and risk checks produce an auditable decision code.
4. Accepted routes pass through the idempotent two-leg executor and mutate only in-memory simulated wallets after its invariants pass.
5. Audit records use SQLite locally or TLS-protected TimescaleDB in production; the bounded worker drains on SIGTERM.
6. The API and `/tiempo-real` publish a bounded snapshot every 450 ms. `/metrics` exposes low-cardinality counters, gauges and histograms.
7. Discord Interactions and the MCP-lite HTTP/JSON bridge reuse validated simulator operations; neither integration can place real orders.

## Ownership

| Area | Source of truth |
|---|---|
| Domain JSON | `mayab-arbitrage/src/types.rs` |
| Market adapters | `mayab-arbitrage/src/mercado.rs` |
| Decisions and simulation | `mayab-arbitrage/src/motor.rs` |
| Two-leg execution and reconciliation | `mayab-arbitrage/src/execution.rs` |
| Genetic optimization | `mayab-arbitrage/src/ga.rs` |
| HTTP and WebSocket | `mayab-arbitrage/src/server.rs` and `src/http/` |
| Discord bot and NVIDIA tools | `mayab-arbitrage/src/discord.rs` |
| MCP-lite manifest and dispatch | `mayab-arbitrage/src/server.rs` |
| Audit persistence | `mayab-arbitrage/src/persistencia.rs` and `persistencia_timescale.rs` |
| Browser UI | `internal/webui/web/` |

## Concurrency and failure boundaries

Feeds and periodic analysis run as Tokio tasks behind `Arc<Motor>`. A single-trade-in-flight guard prevents incompatible simulated wallet updates. Broadcast is bounded; slow WebSocket clients may miss snapshots and recover on the next one. Market ingestion continues if a browser disconnects. Production readiness fails unless the active backend reports durable storage; credentials are never returned in public state.

See [design decisions](DESIGN_DECISIONS.md), [security model](SECURITY_MODEL.md), [agent integrations](MCP_DISCORD.md), and [operations](OPERATIONS.md).
