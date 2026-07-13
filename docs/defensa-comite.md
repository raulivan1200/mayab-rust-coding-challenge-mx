# Defensa para comité final

Este guion resume cómo presentar Mayab Arbitraje BTC sin depender de una oportunidad real de mercado y sin prometer trading real.

## Apertura de 60 segundos

Mayab es un binario Rust seguro que consume order books públicos de BTC, calcula arbitraje neto con costos, simula fills y wallets prefundeadas, y expone una UI en tiempo real. La diferencia no es imprimir spreads brutos: cada decisión queda auditada con fees, slippage, retiro amortizado, latencia, profundidad, inventario, Z-Score, pesos del algoritmo genético y razón estable de aceptación o rechazo.

La demo es segura: no usa llaves API, no firma órdenes, no custodia fondos y todos los POST solo cambian estado simulado en memoria.

## Demo recomendada

1. Abrir dashboard.
   - Mostrar badge LIVE/DEMO/REST.
   - Mostrar mapa de rutas, wallets, P&L, latencia, eventos y panel GA.

2. Abrir `/api/preflight`.
   - Confirmar `judgeReadiness.status=ready`.
   - Confirmar `passed=12`, `total=12` y todos los checks en verde.
   - Mostrar `rubricaOficial` con los 5 criterios del correo.

3. Ejecutar demo rentable.
   - Botón: **Ejecutar prueba completa**.
   - Evidencia esperada: operaciones, PnL positivo, eventos `demo_rentable`, auditoría y GA activo.

4. Ejecutar rebalanceo.
   - Botón: **Forzar rebalanceo**.
   - Evidencia esperada: movimiento interno de wallet, costo explícito y rebalanceo en timeline.

5. Abrir `/api/paquete-evaluacion`.
   - Mostrar `puntajeTotal`, `huellaAuditoria`, `rubricaOficialComite`, `radarCompetitivo` y `recomendacionesParaGanar`.

6. Cerrar con export.
   - Descargar `/api/export/json` o `/api/export/csv`.
   - Explicar que el export sella la corrida y que producción conserva operaciones, ejecuciones, oportunidades, eventos, auditorías y rebalanceos en TimescaleDB.

## Smoke verificable

Con servidor local activo:

```bash
make check
make smoke
```

Con binario release temporal:

```bash
make release-check
```

Contra URL pública después de deploy:

```bash
BASE_URL=https://tu-url-publica ./scripts/smoke-demo.sh
```

El smoke falla si no hay salud, preflight, rúbrica oficial, GA, PnL positivo, eventos demo, rebalanceo, paquete de evaluación o persistencia activa.

## Mapa contra rúbrica

| Criterio del comité | Evidencia defendible |
| --- | --- |
| Profundidad y parametrización | `/api/config`, presets UI, costos por exchange, toggles de exchanges, GA configurable, umbrales, stale guard, adversidad y rebalanceo. |
| Robustez ante adversidad | Escenarios `fallo_orden`, `mercado_movido`, `fill_parcial`, `liquidez_insuficiente`, `circuit_breaker`, `rebalanceo` y `mercado_rentable`. |
| Wallets y rebalanceo | Balances USD/BTC por exchange, protección contra saldos negativos, rebalanceos automáticos y demo manual auditada. |
| Interfaz y visualización | WebSocket `/tiempo-real`, mapa de rutas, decision inspector, P&L, drawdown, win rate, timeline, GA, backtest y exports. |
| Documentación y claridad | README, este guion, `AGENTS.md`, `make check`, CI, smoke y endpoints compactos para jueces/LLMs. |

## Preguntas difíciles

### ¿Por qué no ejecuta órdenes reales?

Porque el reto evalúa un sistema de arbitraje demostrable, no custodia ni ejecución financiera real. Conectar órdenes privadas sin autenticación fuerte, límites de exposición, permisos por exchange, auditoría regulatoria y manejo seguro de secretos sería irresponsable. El proyecto delimita explícitamente que es simulación segura.

### ¿Cómo evitan PnL falso?

El motor no usa solo best bid/ask. Evalúa profundidad acumulada, fees taker por exchange, slippage, retiro amortizado, haircut de latencia, inventario disponible, stale books, USD/USDT basis y revalidación antes de mover balances simulados. Si no sobrevive esos filtros, registra rechazo con razón auditable.

### ¿Qué pasa si no hay arbitraje real durante la demo?

BTC líquido normalmente no regala edge neto. Por eso existe `mercado_rentable`: inyecta una dislocación sintética y etiquetada para demostrar el flujo end-to-end sin fingir que fue una oportunidad live. El estado y eventos la marcan como demo.

### ¿Qué aporta el algoritmo genético?

Optimiza pesos de scoring, umbral mínimo, tamaño máximo y tolerancia de latencia. Usa población, elitismo, torneo, cruce, mutación gaussiana, annealing e inyección diferencial. Puede entrenar con historial real o replay sintético reproducible cuando no hay trades suficientes.

### ¿Cómo se audita la decisión?

`auditoriaDecisiones` registra ruta, par, decisión, `decisionCode`, `decisionReason`, score, pesos GA, utilidad, net bps, costo total, latencia, Z-Score y balances previos. También se exporta a JSON/CSV y se persiste en SQLite local o TimescaleDB productivo.

### ¿Qué diferencia a Mayab de una demo web común?

Tiene un camino completo de evaluación: motor Rust, WebSocket-first con REST fallback, aritmética decimal en cálculos críticos, adversidad controlada, wallets, rebalanceo, GA, backtest, auditoría durable, UI en tiempo real, preflight, paquete de evaluación, CI y smoke reproducible.

## Secuencia final antes de entregar

1. `make check`
2. `cargo build --release --locked`
3. `make release-check`
4. Deploy Cloud Run.
5. `BASE_URL=https://url-publica ./scripts/smoke-demo.sh`
6. Abrir dashboard público y revisar mobile/desktop.
7. Actualizar envío con repo y URL pública.
