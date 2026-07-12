# Adding an exchange

1. Implement the `ExchangeAdapter` contract in `mayab-arbitrage/src/mercado.rs`: public WebSocket URL, subscription, frame parser and public REST fallback.
2. Normalize symbols, bid/ask, depth, exchange timestamp, receive timestamp, source and connection state into `Cotizacion`. Never pass an exchange payload into the engine.
3. Add fees, reliability and withdrawal assumptions to the default exchange configuration. Do not add credentials or private trading endpoints.
4. Register the adapter in the feed startup registry and expose its enabled state through `exchangesActivos`.
5. Add parser fixtures for valid data, malformed frames, heartbeat/subscription messages, sequence gaps and reconnect behavior.
6. Verify symbol semantics. BTC/USD and BTC/USDT are different lanes; a new quote currency requires an explicit basis policy.
7. Exercise both real history and synthetic replay:

```bash
cargo test --workspace --all-targets --locked
cargo run
curl -sS http://127.0.0.1:8080/api/estado
curl -sS -X POST http://127.0.0.1:8080/api/ga/evolucionar \
  -H 'Content-Type: application/json' \
  -H "Authorization: Bearer ${ADMIN_TOKEN}" \
  -d '{"usarReplaySiVacio":true,"muestras":96}'
```

Acceptance means malformed data cannot panic, stale books cannot execute, the UI renders the exchange without special-case secrets, and `mercado_rentable` still demonstrates positive simulated P&L independently of live opportunity availability.
