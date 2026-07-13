# Respuesta verificable a la auditoría externa

La auditoría recibida describe una revisión anterior escrita para un monolito Go. El entregable
actual es un workspace Rust y varios hallazgos ya no representan el código que se evalúa. Esta
matriz no niega las limitaciones del simulador: enlaza cada crítica material con evidencia local
reproducible y separa claramente lo implementado de lo que sigue fuera de alcance.

| Crítica | Estado actual | Evidencia verificable |
|---|---|---|
| Liquidez ficticia cuando falta cantidad | Corregido: una cotización sin cantidad ni profundidad positiva no es ruteable | `cotizacion_valida` y tests `quote_sin_cantidad_ni_profundidad_no_es_ruteable` / `quote_con_profundidad_explicita_es_ruteable_aunque_bbo_qty_falte` en `src/motor.rs` |
| Replay inexistente | Corregido: captura pública, tape versionado, verificación y replay determinista en sandbox | `src/tape.rs`, `mayab-cli/src/capture_tape.rs`, `mayab-cli/src/verify_tape.rs`, `/api/replay/*` |
| Replay contamina el estado live | Corregido: el tape se ejecuta en un `Motor` desechable y devuelve resultados aislados | `Motor::ejecutar_replay_capturado` en `src/motor.rs` |
| Inventario/rebalanceo instantáneo | Corregido para la simulación: capital bloqueado, settlement configurable, costo explícito y ledger auditable | `rebalanceSettlementMs`, `rebalanceos_pendientes` y test `rebalanceo_genera_evento_y_bloquea_capital_hasta_liquidacion` |
| Saldos negativos | Protegido con validación previa y aplicación atómica sobre wallets simuladas | tests `carteras_aplica_operacion_atomica`, `carteras_conservan_btc_y_contabilizan_pnl_en_multiples_escenarios` |
| Backoff no se restablece | Corregido: reconexión con jitter y reset después de una sesión saludable | loops de conexión en `src/mercado.rs` |
| Fallos de integridad invisibles | Corregido: secuencias, checksum, resync e invalidación viajan en el contrato público y en Prometheus | `Cotizacion` en `src/types.rs`, parsers de `src/mercado.rs`, métricas `mayab_feed_*` |
| Sin kill switch ni límites de pérdida | Corregido para ejecución simulada | endpoints admin, `Motor::activar_kill_switch`, circuit breaker y tests asociados |
| Sin auth/rate limit/origin policy | Corregido en mutaciones; lecturas de mercado permanecen públicas por diseño | `src/server.rs`, `src/http/origin.rs` y tests del router |
| Sin CI, race/integración/cobertura | Corregido | `.github/workflows/rust.yml`, `coverage.yml`, `security.yml`, `benchmarks.yml`, `tests/integration_test.rs`, Playwright E2E |
| Sin evidencia experimental | Corregido sin prometer rentabilidad futura | `src/evaluation.rs`, `src/microestructura.rs`, `src/ou.rs`, `evaluate-tape`, `/api/backtest`, `/api/lab/sweep`, `FINAL_EVIDENCE.md` |

## Límites que se mantienen deliberadamente

- No hay órdenes reales, API keys privadas, custodia ni transferencias on-chain.
- Un replay demuestra comportamiento reproducible bajo sus datos y supuestos; no demuestra
  rentabilidad futura.
- Los rebalanceos son movimientos contables simulados con settlement y costo, no retiros reales.
- SQLite local es efímero por defecto; el deploy productivo exige TimescaleDB externo y bloquea readiness si deja de estar saludable.
- Una cotización REST fallback está identificada como tal y no se presenta como WebSocket live.

## Verificación mínima para jurado

```bash
cargo fmt -- --check
cargo test --workspace
BASE_URL=http://127.0.0.1:8080 ./scripts/smoke-demo.sh
curl -sS http://127.0.0.1:8080/metrics | grep mayab_feed_
curl -sS http://127.0.0.1:8080/api/preflight
curl -sS http://127.0.0.1:8080/api/resumen-llm
```

La defensa correcta no es “Mayab gana más”. Es: **Mayab reduce falsos positivos y hace auditables
los supuestos que convierten un spread bruto en una operación simulada ejecutable**.
