# API Reference

## `GET /healthz`
Health check. Devuelve `{"ok": true}`.

## `GET /api/estado`
Snapshot completo del motor: cotizaciones, carteras, operaciones, GA, métricas.

## `GET /api/preflight`
Reporte de preparación para jurado. Scorecard, evidencia forense, cobertura.

## `GET /api/resumen-llm`
Resumen narrativo del estado actual (texto plano para LLM).

## `GET /api/backtest`
Backtest comparativo: baseline conservador vs optimizado GA. 24 semillas de validación.

## `GET /api/lab/sweep`
Sweep de 4 presets (conservador, balanceado, agresivo, GA edge) con análisis de sensibilidad de umbral y slippage.

## `GET /api/metrics`
Métricas Prometheus: contadores HTTP, gauges del motor (PnL, drawdown, Sharpe, GA).

## `GET /api/ga/estado`
Estado del GA: población, generación, campeón, fitness.

## `GET /api/ga/sensibilidad`
Compara configuraciones de población, mutación y cruce sobre 24 semillas holdout comunes. El alias histórico `/api/ga/ablacion` conserva compatibilidad.

## `POST /api/ga/evolucionar`
Ejecuta N generaciones del GA.

**Body:**
```json
{"usarReplaySiVacio": true, "muestras": 96}
```

## `POST /api/ga/config`
Actualiza configuración del GA.

## `POST /api/demo`
Activa escenario demo.

**Body:**
```json
{"escenario": "mercado_rentable"}
```
Escenarios: `mercado_rentable`, `fill_parcial`, `rebalanceo`, `caos`.

## `POST /api/demo/caos`
Prueba de caos: activa adversidad extrema.

## `POST /api/demo/reset`
Reinicia simulación (conserva configuración).

## `POST /api/demo/final`
Deja demo lista para jurado.

## `POST /api/exchanges`
Toggle de exchange activo/inactivo.

**Body:**
```json
{"exchange": "Binance", "activo": true}
```

## `GET /api/export/json`
Exporta toda la auditoría como JSON.

## `GET /api/export/csv?tabla=operaciones`
Exporta tabla como CSV. Tablas: `operaciones`, `oportunidades`, `eventos`, `auditorias`, `rebalanceos`.

## `GET /api/paquete-evaluacion`
Paquete completo para jurado: preflight + scorecard + enlaces.

## `WS /tiempo-real`
WebSocket push de `EstadoPublico` a ~1 Hz.

## `GET /`
Dashboard estático (index.html).
