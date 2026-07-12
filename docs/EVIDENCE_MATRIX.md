# Evidence Matrix

Mayab no se autocalifica. Esta matriz permite que el evaluador asigne su propia
calificacion a partir de evidencia reproducible. `LIVE` significa datos publicos
observados; `SYNTHETIC` significa un escenario deterministico del simulador.

| Claim | Origen | Evidencia runtime | Codigo | Test / reproduccion | Endpoint |
|---|---|---|---|---|---|
| Dos o mas venues utilizables | LIVE | venues unicos con WebSocket fresco y libro ruteable | `src/mercado.rs`, `src/server.rs` | abrir preflight con el servidor activo | `GET /api/preflight` |
| Utilidad neta despues de costos | LIVE o SYNTHETIC | waterfall de fees, slippage, latencia, basis y retiro amortizado | `src/motor.rs` | `cargo test -p mayab-arbitrage motor` | `GET /api/estado` |
| Conciliacion completa de dos piernas | SYNTHETIC etiquetado | FSM termina `COMMITTED` con exposicion BTC cero | `src/motor.rs` | `POST /api/demo/final` | `GET /api/estado` |
| Fill parcial acotado por liquidez | SYNTHETIC etiquetado | operacion `parcial=true`; cantidad llena no excede profundidad | `src/motor.rs` | `POST /api/demo/final` | `GET /api/preflight` |
| Fallo de segunda pierna y unwind | SYNTHETIC etiquetado | `LEG_B_REJECTED -> UNWIND_FILLED -> RECONCILED_LOSS` | `src/motor.rs` | `POST /api/demo/caos` | `GET /api/estado` |
| Rebalanceo de inventario | SYNTHETIC etiquetado | saldos antes/despues, costo y settlement | `src/motor.rs` | `POST /api/demo/final` | `GET /api/estado` |
| GA no sustituye baseline sin evidencia | REPLAY | champion y challenger se publican por separado | `src/ga.rs`, `src/server.rs` | `POST /api/ga/evolucionar` | `GET /api/ga/estado` |
| Hot path medido sin mezclar scheduling | LIVE | compute y scheduling con p50/p95/p99, muestras y throughput | `src/motor.rs`, `src/types.rs` | observar una ventana con feeds activos | `GET /api/preflight` |
| Demo no opera fondos | DESIGN | sin llaves privadas, custodia, firmas ni transferencias | `src/http/auth.rs`, `src/motor.rs` | revisar modelo de seguridad | `GET /api/resumen-llm` |

## Interpretacion de estados

- `PASS`: evidencia presente en la corrida actual.
- `WARN`: capacidad implementada, pero la corrida limpia aun no genero evidencia.
- `FAIL`: capacidad operativa necesaria no disponible; puede bloquear readiness.

El readiness operativo solo depende de que el motor pueda evaluarse ahora. No
depende de tener PnL positivo, operaciones historicas, GA evolucionado o eventos
adversos precargados. Esos elementos se reportan separadamente como evidencia.
