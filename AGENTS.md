# AGENTS.md

## Proyecto

Mayab Arbitraje BTC es un binario Rust que sirve:

- Feeds WebSocket publicos de mercado.
- Motor de arbitraje simulado.
- API Axum.
- Dashboard estatico desde `internal/webui/web`.

No hay ordenes reales, llaves API, custodia ni transferencias on-chain. Los POST solo cambian estado simulado en memoria.

## Comandos base

```bash
cargo fmt -- --check
cargo test --workspace
cargo run
```

Debug local:

```bash
RUST_LOG=debug cargo run
```

Frontend debug:

```text
http://127.0.0.1:8080/?debug=1
```

Sin `?debug=1` o `localStorage.mayabDebug=1`, el dashboard no debe emitir logs de consola ni instalar observers de performance.

Con el servidor local activo:

```bash
curl -sS http://127.0.0.1:8080/healthz
curl -sS http://127.0.0.1:8080/api/preflight
curl -sS http://127.0.0.1:8080/api/resumen-llm
curl -sS http://127.0.0.1:8080/api/backtest
```

## Archivos clave

- `src/motor.rs`: decisiones, simulacion, carteras, adversidad, demo rentable y metricas.
- `src/ga.rs`: poblacion, fitness, seleccion, cruce, mutacion, recocido e inyeccion diferencial.
- `src/mercado.rs`: adaptadores WebSocket y parsers por exchange.
- `src/server.rs`: rutas HTTP, WebSocket, preflight, resumen LLM, backtest y exports.
- `src/types.rs`: contrato JSON del dominio.
- `internal/webui/web/index.html`: estructura del dashboard.
- `internal/webui/web/app.js`: interacciones, WebSocket, render y controles POST.
- `internal/webui/web/styles.css`: layout responsive y estilo visual.

## Reglas de cambio

- Mantener el sistema como demo segura: no agregar trading real ni manejo de secretos sin una capa explicita de seguridad.
- En controles interactivos, usar como hover por defecto el relleno vertical de abajo hacia arriba de los CTA del hero (`Ver a Mayab decidir` e `Inspeccionar la evidencia completa`), con la misma duracion y curva al entrar y salir. No sustituirlo por cambios instantaneos de color, saltos, escalado o sombras. Se exceptuan cierres, estados deshabilitados, `prefers-reduced-motion` y superficies meramente informativas.
- Si se agrega una promesa al README, exponerla en API/UI o quitarla.
- Si se toca `EstadoPublico` o contratos JSON, actualizar UI y exports.
- Si se cambia el motor, agregar o ajustar tests unitarios en `src/motor.rs`.
- Si se cambia GA, validar que `/api/ga/evolucionar` funcione con historial real y con replay sintetico.
- No depender de oportunidades reales para demostrar valor: `POST /api/demo {"escenario":"mercado_rentable"}` debe mantener el dashboard vivo.

## Smoke de demo

```bash
curl -sS -X POST http://127.0.0.1:8080/api/ga/evolucionar \
  -H 'Content-Type: application/json' \
  -d '{"usarReplaySiVacio":true,"muestras":96}'

curl -sS -X POST http://127.0.0.1:8080/api/demo \
  -H 'Content-Type: application/json' \
  -d '{"escenario":"mercado_rentable"}'

curl -sS http://127.0.0.1:8080/api/estado
```

Despues de `mercado_rentable`, debe haber operaciones, PnL positivo, eventos `demo_rentable` y GA activo.

## Deploy

- Plataforma principal: Cloud Run.
- Script: `./scripts/deploy-cloud-run.sh`.
- Para evaluación final, considerar `MIN_INSTANCES=1` temporalmente para evitar cold start.
- Render/Fly existen como alternativas, pero no son la ruta principal de entrega.
- Despues de deploy, validar la URL pública con `/healthz`, `/api/preflight`, `/api/resumen-llm`, `/api/ga/estado` y el dashboard.
