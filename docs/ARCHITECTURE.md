# Architecture

Mayab is one Rust process with explicit boundaries: public market-data adapters feed a simulated decision engine; Axum exposes read APIs, sandbox mutations, Prometheus metrics and static UI. No component can place a real order.

```text
public WS/REST -> normalize quote -> Motor -> risk/decision -> simulated wallets
                                      |             |
                                      v             v
                                  GA strategy   SQLite audit
                                      \             /
                                       Axum + WS -> dashboard/operator
```

## Runtime flow

1. `mercado.rs` parses exchange-specific frames into `Cotizacion`.
2. `Motor::recibir_cotizacion` maintains fresh books and `Motor::analizar` evaluates compatible USD or USDT lanes.
3. Cost, depth, inventory, latency and risk checks produce an auditable decision code.
4. Accepted routes mutate only in-memory simulated wallets. Audit records may be written to SQLite.
5. The API and `/tiempo-real` publish a bounded snapshot every 450 ms. `/metrics` exposes low-cardinality counters, gauges and histograms.
6. Discord Interactions and the MCP-lite HTTP/JSON bridge reuse validated simulator operations; neither integration can place real orders.

## Ownership

| Area | Source of truth |
|---|---|
| Domain JSON | `mayab-arbitrage/src/types.rs` |
| Market adapters | `mayab-arbitrage/src/mercado.rs` |
| Decisions and simulation | `mayab-arbitrage/src/motor.rs` |
| Genetic optimization | `mayab-arbitrage/src/ga.rs` |
| HTTP and WebSocket | `mayab-arbitrage/src/server.rs` and `src/http/` |
| Discord bot and NVIDIA tools | `mayab-arbitrage/src/discord.rs` |
| MCP-lite manifest and dispatch | `mayab-arbitrage/src/server.rs` |
| Audit persistence | `mayab-arbitrage/src/persistencia.rs` |
| Browser UI | `internal/webui/web/` |

## Concurrency and failure boundaries

Feeds and periodic analysis run as Tokio tasks behind `Arc<Motor>`. A single-trade-in-flight guard prevents incompatible simulated wallet updates. Broadcast is bounded; slow WebSocket clients may miss snapshots and recover on the next one. Market ingestion continues if a browser disconnects. SQLite failure is reported in state and must not be confused with durable storage on Cloud Run's ephemeral filesystem.

See [design decisions](DESIGN_DECISIONS.md), [security model](SECURITY_MODEL.md), [agent integrations](MCP_DISCORD.md), and [operations](OPERATIONS.md).
