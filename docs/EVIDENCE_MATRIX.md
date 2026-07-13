# Evidence Matrix

Mayab no se autocalifica. Esta matriz permite que el evaluador asigne su propia
calificación a partir de evidencia reproducible. `LIVE` significa datos públicos
observados; `SYNTHETIC` significa un escenario deterministico del simulador.

| Claim | Origen | Evidencia runtime | Codigo | Test / reproduccion | Endpoint |
|---|---|---|---|---|---|
| Dos o mas venues utilizables | LIVE | venues unicos con WebSocket fresco y libro ruteable | `src/mercado.rs`, `src/server.rs` | abrir preflight con el servidor activo | `GET /api/preflight` |
| Utilidad neta despues de costos | LIVE o SYNTHETIC | waterfall de fees, slippage, latencia, basis y retiro amortizado | `src/motor.rs`, `src/server.rs` | `cargo test -p mayab-arbitrage motor` | `GET /api/research/economics` |
| Conciliación completa de dos piernas | SYNTHETIC etiquetado | Ejecutor termina `RECONCILED` con exposición BTC cero, ledger y reservas conciliadas | `src/execution.rs` | `POST /api/demo/final` | `GET /api/estado` |
| Matriz adversa determinista | SYNTHETIC determinista etiquetado | 12 escenarios, 10 invariantes por escenario y `matrixSha256` estable | `src/execution.rs`, `src/server.rs` | `cargo test -p mayab-arbitrage execution` | `GET /api/research/execution-matrix` |
| Matriz adversa completa 12/12 | SYNTHETIC determinista etiquetado | doce escenarios, diez invariantes por escenario y `matrixSha256` estable | `src/execution.rs`, `src/server.rs` | contrato HTTP y prueba de determinismo | `GET /api/research/execution-matrix` |
| Fill parcial acotado por liquidez | SYNTHETIC etiquetado | operación `parcial=true`; cantidad llena no excede profundidad | `src/motor.rs` | `POST /api/demo/final` | `GET /api/preflight` |
| Fallo de segunda pierna y unwind | SYNTHETIC etiquetado | `LEG2_REJECTED -> RECOVERY_SELECTED -> RECONCILED`; fills, wallets, PnL y reservas pasan invariantes | `src/execution.rs` | `POST /api/demo/caos` | `GET /api/estado` |
| Rebalanceo de inventario | SYNTHETIC etiquetado | saldos antes/despues, costo y settlement | `src/motor.rs` | `POST /api/demo/final` | `GET /api/estado` |
| GA no sustituye baseline sin evidencia | REPLAY | champion y challenger se publican por separado | `src/ga.rs`, `src/server.rs` | `POST /api/ga/evolucionar` | `GET /api/ga/estado` |
| Hot path medido sin mezclar scheduling | LIVE | compute y scheduling con p50/p95/p99, muestras y throughput | `src/motor.rs`, `src/types.rs` | observar una ventana con feeds activos | `GET /api/preflight` |
| Demo no opera fondos | DESIGN | sin llaves privadas, custodia, firmas ni transferencias | `src/server.rs`, `src/motor.rs` | revisar modelo de seguridad | `GET /api/resumen-llm` |

## Interpretacion de estados

- `PASS`: evidencia presente en la corrida actual.
- `WARN`: capacidad implementada, pero la corrida limpia aun no genero evidencia.
- `FAIL`: capacidad operativa necesaria no disponible; puede bloquear readiness.

`operationalReady` mide si el motor puede evaluarse ahora. El gate público
`listo=true` es más estricto: exige esa salud operativa y exactamente 12/12
checks de la corrida visible. La reconciliación sólo pasa cuando existe el caso
adverso runtime y la matriz pura completa reporta 12/12 con todas sus
invariantes. `POST /api/demo/final` genera la parte runtime de forma sintética y
etiquetada cuando el mercado live no la produce.
