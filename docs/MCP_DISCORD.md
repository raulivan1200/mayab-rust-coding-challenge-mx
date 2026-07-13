# Integraciones para agentes: MCP-lite y Discord

Este documento describe las dos superficies que permiten consultar Mayab desde
un agente: el contrato HTTP/JSON MCP-lite y el bot opcional de Discord. Ambas
operan exclusivamente sobre mercado público y estado simulado. Ninguna coloca
órdenes reales, custodia fondos o recibe llaves privadas de exchanges.

## Límites de seguridad

- Los datos de lectura son públicos.
- En producción, toda herramienta MCP-lite mutable requiere `ADMIN_TOKEN`.
- En desarrollo local, el token administrativo es opcional.
- `MAYAB_JUDGE_MODE=true` no abre las mutaciones MCP-lite; sólo permite los tres
  recorridos HTTP cerrados de jurado.
- Discord verifica la firma Ed25519 antes de deserializar cada interacción.
- La herramienta de IA que cambia parámetros sólo se ofrece a miembros con
  `Manage Server` o `Administrator`.
- `/demo-rentable` y `prepare_demo` pueden modificar la simulación, pero no
  parámetros arbitrarios ni recursos externos.
- Los secretos se cargan desde variables de entorno o Secret Manager y no se
  incluyen en respuestas, logs, imágenes Docker ni ejemplos versionados.

## MCP-lite por HTTP/JSON

### Compatibilidad

MCP-lite es un contrato propio y deliberadamente pequeño. No implementa el
transporte ni el ciclo de sesión del Model Context Protocol estándar. Un cliente
MCP estándar necesita un adaptador que traduzca sus llamadas a:

- `GET /api/mcp/manifest`: catálogo y metadatos.
- `POST /api/mcp/call`: invocación de una herramienta.
- `GET /api/mcp`: alias del manifiesto.

El manifiesto declara `protocol: "mayab-mcp-lite-v1"`,
`mcpStandardCompatible: false` y `transport: "http-json"` para evitar que un
integrador confunda ambas interfaces.

### Forma de una llamada

```json
{
  "tool": "summarize_for_llm",
  "arguments": {}
}
```

`arguments` es opcional. Las herramientas sin parámetros sólo aceptan un objeto
vacío. Los DTO de `evolve_ga` y `demo_scenario` rechazan campos desconocidos.

Una respuesta correcta conserva esta envoltura:

```json
{
  "ok": true,
  "tool": "summarize_for_llm",
  "result": {}
}
```

Los errores de validación usan el contrato general de la API. Una herramienta
desconocida devuelve HTTP 400, `ok: false` y la ruta del manifiesto.

### Herramientas de lectura

| Herramienta | Argumentos | Resultado |
|---|---|---|
| `get_state` | Ninguno | Contrato completo de `/api/estado` |
| `preflight` | Ninguno | Readiness, checks y evidencia operativa |
| `jury_mode` | Ninguno | Rúbrica, scorecard, cobertura y enlaces |
| `summarize_for_llm` | Ninguno | Resumen narrativo y métricas clave |
| `evaluation_package` | Ninguno | Paquete de evaluación reproducible |
| `latency_ranking` | Ninguno | Ranking y percentiles por exchange |
| `backtest` | Ninguno | Backtest con la configuración vigente |
| `research_lab_sweep` | Ninguno | Comparación pareada de presets |

### Herramientas mutables

| Herramienta | Argumentos | Efecto simulado |
|---|---|---|
| `prepare_demo_final` | Ninguno | Prepara GA, rentabilidad, fallos, conciliación y rebalanceo |
| `evolve_ga` | `usarReplaySiVacio?: boolean`, `muestras?: integer` | Evoluciona el GA con historial o replay sintético |
| `demo_scenario` | `escenario: string` | Ejecuta un escenario controlado |

Los escenarios válidos son `fallo_orden`, `fallo_segunda_pierna`,
`mercado_movido`, `liquidez_insuficiente`, `fill_parcial`, `circuit_breaker`,
`rebalanceo` y `mercado_rentable`.

En `evolve_ga`, `usarReplaySiVacio` vale `true` por defecto y `muestras` vale
`96`; cuando se envía, `muestras` debe estar entre 12 y 240.

Ejemplo de lectura:

```bash
curl -fsS -X POST http://127.0.0.1:8080/api/mcp/call \
  -H 'Content-Type: application/json' \
  -d '{"tool":"get_state"}'
```

Ejemplo mutable en un entorno protegido:

```bash
curl -fsS -X POST http://127.0.0.1:8080/api/mcp/call \
  -H 'Content-Type: application/json' \
  -H "Authorization: Bearer ${ADMIN_TOKEN}" \
  -d '{"tool":"demo_scenario","arguments":{"escenario":"fill_parcial"}}'
```

También se admite `X-Admin-Token`. El token nunca debe viajar en la URL.

## Bot de Discord

### Flujo de una interacción

1. Discord envía el cuerpo crudo y los headers `X-Signature-Ed25519` y
   `X-Signature-Timestamp` a `POST /api/discord/interactions`.
2. Mayab verifica la firma con `DISCORD_PUBLIC_KEY` antes de leer el JSON.
3. Los comandos directos responden en la misma interacción.
4. Las consultas a NVIDIA devuelven primero una respuesta diferida.
5. El resultado final reemplaza el mensaje original mediante el webhook de la
   interacción. El contenido desactiva menciones para evitar pings accidentales.

### Configuración

| Variable | Secreta | Obligatoria | Uso |
|---|---:|---:|---|
| `DISCORD_APPLICATION_ID` | No | Para registrar comandos | Identificador de la aplicación |
| `DISCORD_PUBLIC_KEY` | No | Para aceptar interacciones | Verificación Ed25519 |
| `DISCORD_BOT_TOKEN` | Sí | Para registrar comandos | Autorización contra Discord API |
| `DISCORD_GUILD_ID` | No | No | Registro inmediato en un servidor de pruebas |
| `NVIDIA_API_KEY` | Sí | Para `/mayab` y `/ask` | Chat Completions de NVIDIA NIM |
| `NVIDIA_MODELS` | No | No | Lista ordenada de modelos de fallback |

Si falta `DISCORD_PUBLIC_KEY`, el webhook devuelve HTTP 503. Si la firma es
inválida, devuelve HTTP 401. Si faltan Application ID o Bot Token, el servidor
continúa funcionando, pero omite el registro automático de slash commands.

Para una instalación de prueba:

```bash
cp .env.example .env
# Sustituye los placeholders secretos dentro de .env.
cargo run
```

Configura como Interactions Endpoint URL:

```text
https://TU_SERVICIO/api/discord/interactions
```

### Slash commands

| Comando | Efecto |
|---|---|
| `/estado` | PnL, retorno, operaciones, riesgo, GA y feeds |
| `/resumen` | Alias compacto de estado |
| `/demo-rentable` | Evoluciona el GA y prepara una demo rentable simulada |
| `/mayab pregunta:<texto>` | Consulta datos o solicita cambios permitidos a Mayab IA |
| `/ask pregunta:<texto>` | Pregunta general o sobre el estado de Mayab |

Los comandos globales pueden tardar en propagarse. Con `DISCORD_GUILD_ID` se
registran en el servidor indicado y suelen quedar disponibles inmediatamente.

### Herramientas del agente NVIDIA

| Herramienta | Lectura | Disponible para | Descripción |
|---|---:|---|---|
| `get_state` | Sí | Todos | Métricas, riesgo, operaciones y GA |
| `get_config` | Sí | Todos | Parámetros vigentes del simulador |
| `get_audit_history` | Sí | Todos | Resumen SQLite y últimas 20 operaciones |
| `prepare_demo` | No | Todos | Prepara `mercado_rentable` en la simulación |
| `update_parameters` | No | Administradores | Cambia únicamente cinco límites validados |

`update_parameters` acepta `maxOperacionBtc`, `minDiferencialNetoBps`,
`deslizamientoBps`, `minUtilidadUsd` y `enfriamientoMs`. El backend vuelve a
validar rangos; que el modelo proponga un valor no evita esa validación.

El agente limita cada turno a cuatro llamadas de herramienta y trunca la
respuesta final para respetar el límite de contenido de Discord. Si un modelo de
`NVIDIA_MODELS` falla, prueba el siguiente y reporta el error si todos fallan.

## Verificación local

```bash
cargo test -p mayab-arbitrage discord::tests

curl -fsS http://127.0.0.1:8080/api/mcp/manifest

curl -fsS -X POST http://127.0.0.1:8080/api/mcp/call \
  -H 'Content-Type: application/json' \
  -d '{"tool":"summarize_for_llm"}'
```

El webhook de Discord no debe probarse con un `curl` sin firma: el rechazo 401
es el comportamiento esperado. La prueba unitaria genera una llave efímera y
comprueba tanto una firma válida como la alteración del cuerpo.

## Código fuente relacionado

- `mayab-arbitrage/src/discord.rs`: firma, comandos, agente y herramientas.
- `mayab-arbitrage/src/server.rs`: manifiesto e invocación MCP-lite.
- `mayab-arbitrage/src/http/routes/health.rs`: rutas de integración.
- `.env.example`: variables mínimas sin secretos reales.
- `scripts/smoke-demo.sh`: verificación del manifiesto y una llamada de lectura.
