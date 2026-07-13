# Referencia de API

Mayab expone una API Axum para observar y modificar únicamente una simulación.
No hay rutas para enviar órdenes reales, custodiar fondos, firmar transacciones o
administrar llaves privadas de exchanges.

La especificación legible por herramientas está en
[`docs/openapi.yaml`](docs/openapi.yaml). Los contratos Rust compartidos con el
dashboard viven en `mayab-arbitrage/src/types.rs`.

## Convenciones

- Base local: `http://127.0.0.1:8080`.
- Las lecturas devuelven JSON, salvo Prometheus y descargas CSV.
- Los campos del contrato público usan `camelCase`.
- Los POST mutables aceptan `Content-Type: application/json` cuando tienen body.
- En `MAYAB_ENV=production`, los POST mutables requieren
  `Authorization: Bearer <ADMIN_TOKEN>` o `X-Admin-Token`.
- `POST /api/discord/interactions` no usa `ADMIN_TOKEN`: exige la firma Ed25519
  de Discord.
- En desarrollo local, el token administrativo es opcional.
- `MAYAB_JUDGE_MODE=true` sólo hace públicas `/api/demo/reset`,
  `/api/demo/final` y `/api/demo/caos`; no abre el resto de las mutaciones.

Una mutación correcta suele devolver `{"ok":true}` o un objeto de resultado.
Los errores validados usan:

```json
{
  "ok": false,
  "error": {
    "code": "codigo_estable",
    "message": "Descripción legible"
  }
}
```

## Salud y versión

| Método | Ruta | Descripción |
|---|---|---|
| `GET` | `/healthz` | Liveness mínimo; alias `/api/healthz` |
| `GET` | `/readyz` | Readiness del proceso; alias `/api/readyz` |
| `GET` | `/api/version` | Build, schema, sesión y hashes canónicos de dataset/configuración |

## Estado y evidencia pública

| Método | Ruta | Descripción |
|---|---|---|
| `GET` | `/api/estado` | Snapshot completo de `EstadoPublico` |
| `GET` | `/api/jurado` | Rúbrica, checks, cobertura y enlaces de auditoría |
| `GET` | `/api/preflight` | Gate 12/12 de operación, evidencia, conciliación y persistencia |
| `GET` | `/api/resumen-llm` | Resumen narrativo y métricas para revisores automáticos |
| `GET` | `/api/latencias` | EWMA y percentiles del pipeline y los exchanges |
| `GET` | `/api/backtest` | Comparación reproducible baseline frente a GA |
| `GET` | `/api/lab/sweep` | Sweep pareado de presets sobre el mismo replay |
| `GET` | `/api/paquete-evaluacion` | Scorecard, evidencia, guion y huella de auditoría |
| `GET` | `/api/readiness/live` | Limitaciones declaradas para evidencia live |
| `GET` | `/operator` | Consola operativa estática |

## Investigación reproducible

| Método | Ruta | Descripción |
|---|---|---|
| `GET` | `/api/research/tapes` | Inventario y procedencia de tapes |
| `GET` | `/api/research/walk-forward` | Split train/calibration/holdout y baselines |
| `GET` | `/api/research/impact` | Comparación de modelos de impacto |
| `GET` | `/api/research/economics` | Waterfall, break-even, capacidad y embudo |
| `GET` | `/api/research/execution-matrix` | Matriz determinista de escenarios de ejecución |
| `GET` | `/api/research/bootstrap` | Bootstrap temporal pareado |
| `GET` | `/api/research/microstructure` | Calibración y métricas de microestructura |
| `GET` | `/api/research/ou` | Laboratorio Ornstein-Uhlenbeck fuera de muestra |
| `GET` | `/api/research/ledger-audit` | Conciliación y huella del ledger |

## Motor genético

| Método | Ruta | Descripción |
|---|---|---|
| `GET` | `/api/ga/estado` | Población, generación, campeón y fitness |
| `GET` | `/api/ga/config` | Configuración vigente |
| `GET` | `/api/ga/sensibilidad` | Siete configuraciones sobre holdout común |
| `GET` | `/api/ga/ablacion` | Alias histórico de sensibilidad |
| `POST` | `/api/ga/config` | Actualiza la configuración validada |
| `POST` | `/api/ga/evolucionar` | Fuerza evolución; alias `/api/admin/ga/evolucionar` |

Body de evolución:

```json
{
  "usarReplaySiVacio": true,
  "muestras": 96
}
```

El mismo DTO estricto se usa en HTTP y MCP-lite.

## Demo y adversidad

| Método | Ruta | Descripción |
|---|---|---|
| `POST` | `/api/demo` | Ejecuta un escenario individual |
| `POST` | `/api/adverso` | Alias para escenario adverso |
| `POST` | `/api/admin/adverso` | Alias administrativo |
| `POST` | `/api/demo/reset` | Reinicia balances, PnL, riesgo y GA |
| `POST` | `/api/demo/final` | Prepara la corrida completa de jurado |
| `POST` | `/api/demo/caos` | Encadena degradación, protección y recuperación |

Body de escenario individual:

```json
{
  "escenario": "mercado_rentable"
}
```

Valores permitidos: `fallo_orden`, `fallo_segunda_pierna`, `mercado_movido`,
`liquidez_insuficiente`, `fill_parcial`, `circuit_breaker`, `rebalanceo` y
`mercado_rentable`. La prueba `caos` tiene su propia ruta; no es un valor del
campo `escenario`.

## Configuración, exchanges y rebalanceo

| Método | Ruta | Descripción |
|---|---|---|
| `POST` | `/api/config` | Aplica un parche de configuración del motor |
| `POST` | `/api/admin/config` | Alias administrativo |
| `POST` | `/api/admin/ga/config` | Alias administrativo para GA |
| `POST` | `/api/exchanges` | Habilita o deshabilita un exchange |
| `POST` | `/api/rebalance/rules` | Actualiza reglas de rebalanceo |
| `POST` | `/api/admin/kill-switch` | Activa o libera el kill switch simulado |

Ejemplo de exchange:

```json
{
  "exchange": "Binance",
  "activo": true
}
```

Los DTO rechazan campos desconocidos y valores fuera de rango.

## Captura y replay

| Método | Ruta | Descripción |
|---|---|---|
| `POST` | `/api/demo/capturar/iniciar` | Inicia captura en memoria |
| `POST` | `/api/demo/capturar/detener` | Detiene captura |
| `GET` | `/api/demo/capturar/estado` | Estado y tamaño de captura |
| `POST` | `/api/demo/capturar/replay` | Ejecuta replay de la captura |
| `POST` | `/api/replay/captura/ventana` | Selecciona una ventana de replay |
| `POST` | `/api/replay/ejecutar` | Ejecuta replay |

Existen aliases `/api/replay/captura/{iniciar,detener,estado}` y
`/api/admin/captura/{iniciar,detener,estado,replay}` para clientes anteriores.
Las capturas siguen siendo datos públicos normalizados; no contienen secretos.

## Exportaciones y métricas

| Método | Ruta | Descripción |
|---|---|---|
| `GET` | `/api/export/json` | Estado y auditoría completa en JSON |
| `GET` | `/api/export/csv` | Bitácora CSV |
| `GET` | `/api/export/evidence` | Paquete de evidencia sellable |
| `GET` | `/metrics` | Métricas Prometheus; alias `/api/metrics` |

El CSV es una bitácora unificada y tipada: incluye operaciones, oportunidades,
transiciones, ejecuciones de dos piernas, auditorías y rebalanceos en un solo
archivo, conservando el orden de cada colección.

## MCP-lite y Discord

| Método | Ruta | Descripción |
|---|---|---|
| `GET` | `/api/mcp/manifest` | Catálogo MCP-lite; alias `/api/mcp` |
| `POST` | `/api/mcp/call` | Invoca una herramienta HTTP/JSON |
| `POST` | `/api/discord/interactions` | Webhook firmado para slash commands |

MCP-lite no implementa el transporte MCP estándar. La lista exacta de
herramientas, argumentos, autorización, slash commands y flujo de firma está en
[`docs/MCP_DISCORD.md`](docs/MCP_DISCORD.md).

## WebSocket

`WS /tiempo-real` transmite un `EstadoPublico` compacto cada 450 ms, cerca de
2.2 actualizaciones por segundo. El canal es acotado: un cliente lento puede
omitir snapshots y recuperarse con el siguiente. No se usa como ledger ni como
fuente de persistencia.

## Dashboard

`GET /` sirve el dashboard embebido. El navegador consume `/tiempo-real` y las
rutas anteriores desde el mismo origen. El modo de diagnóstico sólo se activa
con `?debug=1` o `localStorage.mayabDebug=1`.
