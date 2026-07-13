# Mayab Arbitraje BTC — Arquitectura

## Diagrama

```
┌─────────────┐    WebSocket/HTTP     ┌──────────────┐
│   Binance   │◄─────────────────────►│              │
│   Kraken    │                       │   mercado    │
│   Coinbase  │                       │  (10 feeds)  │
│   OKX       │                       │              │
│   Bybit     │                       │  Cotizacion  │
│   Bitfinex  │                       │     →        │
│   KuCoin    │                       │  motor       │
│   Gate.io   │                       │              │
│   Bitstamp  │         Arc::<Motor>  │  analizar()  │
│   Gemini    │──────────────────────►│  ejecutar()  │
└─────────────┘                       └──────┬───────┘
                                            │
                    ┌───────────────────────┼───────────┐
                    │                       │           │
              ┌─────▼─────┐          ┌──────▼──────┐   │
              │   ga.rs   │          │ persistencia│   │
              │ (GA híbrido │          │  (SQLite)   │   │
              │  multi-     │          │  Auditoria  │   │
              │  objetivo)  │          │   trait     │   │
              └───────────┘          └─────────────┘   │
                    │                       │           │
                    └───────────────────────┼───────────┘
                                            │
                                     ┌──────▼──────┐
                                     │   server    │
                                     │  (Axum)     │
                                     │             │
                                     │  /api/*     │
                                     │  WS /tiempo-│
                                     │     real    │
                                     │  /metrics   │
                                     │  dashboard  │
                                     └─────────────┘
```

## Módulos

### Fronteras operativas

- `server.rs` ensambla HTTP y aplica tasa, tamaño, timeout, concurrencia y cache.
- `motor.rs` es dueño exclusivo del estado simulado, balances y P&L.
- `persistencia.rs` encapsula SQLite; sus variantes `try_*` propagan fallos y los
  adaptadores heredados registran errores explícitamente.
- `internal/webui/web/` consume contratos públicos; Playwright verifica el flujo
  navegador–API y que no haya logs fuera de `?debug=1`.

| Módulo | Propósito | Dependencias externas |
|--------|-----------|----------------------|
| `mercado` | Feeds WS + REST por exchange, `ExchangeAdapter` trait | tokio-tungstenite, reqwest |
| `motor` | Simulación, carteras, adversidad, demo, GA loop | chrono, rand |
| `ga` | Población, fitness, selección, cruce, mutación, recocido, evolución diferencial | rand |
| `server` | Axum router, WebSocket push, preflight, LLM resumen, backtest, lab sweep, prometheus | axum, tower-http |
| `http` | Grupos de rutas y política de origen | axum, tower-http |
| `discord` | Firma Ed25519, slash commands y agente NVIDIA acotado | ed25519-dalek, reqwest |
| `execution` | Máquina de estados para dos piernas, unwind y conciliación | rust_decimal |
| `types` | Contrato JSON del dominio, serde | serde, serde_json |
| `persistencia` | SQLite (WAL, indices, aggregate queries), `Auditoria` trait impl | rusqlite |
| `auditoria` | `Auditoria` trait (repository pattern) | — |
| `metricas` | Prometheus hand-rolled: HTTP counters, engine gauges | — |
| `config` | Config desde env vars con defaults seguros | — |

## Flujo de datos

1. `mercado::start_feeds()` lanza 10 tareas WebSocket + 10 REST fallback
2. Cada feed parsea frames → `Cotizacion` → `motor.recibir_cotizacion()`
3. `motor::analizar()` (cada ~70ms) busca oportunidades cross-exchange
4. GA optimiza umbral y tamaño de posición cada 500 ciclos
5. `motor::ejecutar()` aplica carteras y adversidad únicamente sobre el estado simulado
6. `server` expone estado vía WebSocket cada 450 ms y REST API
7. `persistencia::Persistencia` audita operaciones, eventos, rebalanceos, oportunidades
8. Discord y MCP-lite reutilizan los mismos contratos y DTO validados; no tienen
   acceso a ejecución real ni a llaves de exchanges

MCP-lite es una interfaz HTTP/JSON propia y no el transporte MCP estándar. El
contrato y los límites del bot se describen en
[`docs/MCP_DISCORD.md`](docs/MCP_DISCORD.md).

## Carpetas

```
/
├── Cargo.toml          # Workspace root
├── mayab-arbitrage/    # Library crate
│   ├── src/
│   └── tests/
├── mayab-cli/          # Binary crate
│   └── src/main.rs
├── internal/webui/web/ # Dashboard estático (HTML+JS, Vanilla)
├── scripts/            # Deploy, smoke, release
└── .github/workflows/  # CI
```
