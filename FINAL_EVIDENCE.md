# Evidencia de Validación Final (Mayab Arbitraje BTC)

Este documento indica cómo obtener evidencia vigente del binario que se está evaluando. Separa *Real-market paper result* (feeds públicos y ejecución simulada) de *Synthetic demo result* (escenarios artificiales etiquetados). Las métricas dinámicas no se copian aquí como constantes porque latencias, feeds y contadores cambian entre revisiones e instancias.

## Scorecard recordable

- 10 exchanges públicos y 90 rutas lineales dirigidas.
- Suite Rust multi-capa; el conteo aprobado proviene del CI del SHA entregable.
- 24 semillas comunes para comparar baseline y campeón GA.
- 10,000 remuestras moving-block en la validación bootstrap.
- 0 BTC de exposición residual al terminar el escenario reproducible de segunda pierna rechazada.

La última cifra se verifica con `POST /api/demo/caos`: la FSM publica
`DETECTED → RESERVED → LEG1_SUBMITTED → LEG1_FILLED → LEG2_SUBMITTED → LEG2_REJECTED → RECOVERY_SELECTED → RECONCILED`,
incluye la pérdida realizada del unwind y termina conciliada. `/api/jurado`
expone esta afirmación bajo `evidenciaClave.resultadoMemorable`.

## 1. Rúbrica de Criterios (Evaluación Automática `/api/jurado`)

`/api/jurado` y `/api/preflight` calculan sus checks desde el estado actual. Para preparar una corrida reproducible y consultar la evidencia:

```bash
curl -sS -X POST http://127.0.0.1:8080/api/demo/final \
  -H "Authorization: Bearer ${ADMIN_TOKEN}"
curl -sS http://127.0.0.1:8080/api/jurado | jq '{estado, checks, rubricaOficial}'
curl -sS http://127.0.0.1:8080/api/preflight | jq '{listo, judgeReadiness, checks}'
```

El gate automatizado completo es `./scripts/release-check.sh`; crea un token efímero para su servidor local y, además de compilar y probar, exige PnL demo positivo, GA activo, fill parcial, rebalanceo, auditoría y reconciliación de segunda pierna sin exposición residual. En el deploy de evaluación, `MAYAB_JUDGE_MODE=true` permite ejecutar sin credenciales únicamente `/api/demo/reset`, `/api/demo/final` y `/api/demo/caos`; configuración, exchanges, wallets arbitrarios, GA libre, captura y MCP siguen protegidos por `ADMIN_TOKEN`. `/api/jurado.accesoDemo` declara la política efectiva del proceso.

La respuesta de `/api/demo/final` incluye `evidencia.huellaAuditoria`, un SHA-256 del estado de la corrida. Es una huella de integridad reproducible, no una firma criptográfica ni evidencia de rentabilidad real.

## 2. Análisis de sensibilidad (Genetic Algorithm)

La evidencia vigente se genera en tiempo de ejecución mediante `GET /api/ga/sensibilidad`. El reporte usa siete configuraciones reproducibles, 24 semillas comunes de entrenamiento y 24 semillas holdout distintas; no se conservan cifras estáticas que puedan quedar desalineadas del código desplegado.

```bash
curl -sS http://127.0.0.1:8080/api/ga/sensibilidad | jq '{metodologia, resultados}'
```

> **Lectura correcta:** El endpoint ajusta cada configuración con semillas 101..124, congela su estrategia y sólo entonces la evalúa sobre los holdouts pareados 401..424. Todas reciben los mismos corpus train/holdout. Es análisis de sensibilidad del GA híbrido, no una prueba causal aislada de cada operador interno.

## 3. Telemetría de Latencia (Pipeline)

La telemetría separa latencia de red, scheduling quote→decisión y cómputo interno. La fuente autoritativa es:

```bash
curl -sS http://127.0.0.1:8080/api/latencias | jq
```

Los percentiles deben citarse junto con la región, revisión y hora de la medición; el proyecto no declara un SLA universal de los exchanges.

## 4. Cobertura Estática

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --all-targets --locked`
- `cargo deny check`
- `docker build --tag mayab-btc-arbitrage:ci .`

No debe marcarse esta sección como aprobada si la revisión publicada no tiene todos los checks verdes.

### Inventario de pruebas y regla anti-inflación

La suite separa pruebas de librería, auditor de ledger, integraciones y contratos
independientes de rutas públicas en `tests/public_contract_test.rs`. Playwright
se reporta por separado, incluido el selector superior de
procedencia y escala. Como el árbol sigue cambiando, ningún conteo se presenta
como aprobado hasta que CI publique la corrida verde del SHA entregable.

La suite cubre backpressure, fallos y flush acotado de auditoría; atomicidad,
límites y configuración cerrada; catálogo de controles; selección de campeón
sin fuga de holdout; replay determinista; corpus, índices e idempotencia;
memoria acotada, hashes canónicos, publicación atómica e intervalos Wilson 95%.
Playwright valida por separado el dashboard principal, las
superficies de replay y operación, los contratos HTTP, la procedencia del corpus
y la demo rentable; el conteo autoritativo sigue siendo el publicado por CI para
el SHA entregable.

Cada contrato HTTP corresponde a una URI distinta y falla con el nombre exacto
de la superficie rota. Un loop con muchas aserciones no se contabiliza como
muchas pruebas. La comparación con proyectos externos debe reportar por separado
conteo, cobertura de ramas, propiedades/invariantes y escenarios end-to-end;
ninguna de esas métricas sustituye a las demás.

## 5. Paquete sellado y procedencia

`OUT_DIR=artifacts/evidence/final ./scripts/generar-evidencia.sh` ejecuta primero
una corrida limpia de `/api/demo/final` y captura el
paquete de evaluación, auditoría, latencias, backtest, bootstrap, holdout,
microestructura, sensibilidad GA, matriz forense, ledger y preflight. Antes de
publicar, falla si no obtiene preflight exacto 12/12, matriz 12/12, todas las
invariantes runtime, ledger conciliado o una cola de persistencia sin pérdidas.
El directorio se publica atómicamente e incluye `assertions.json`, manifiesto
con commit/dirty-worktree/sesión/hashes, `packageSha256`, `resultSha256` y un
`SHA256SUMS` verificado.

El sello prueba integridad del artefacto, no que el origen sea real. Toda cifra
debe conservar una de estas etiquetas: `mercado_publico`,
`replay_sintetico_o_historial_publico` o `sintetica_etiquetada`. Hasta contar con
un tape público verificable de millones de eventos, Mayab no declara evidencia
equivalente a “millones de dislocaciones reales”. Esa limitación es parte de la
honestidad experimental, no se oculta en el scorecard.

## 6. Decision Inspector

La auditoría real de la corrida se consulta sin ejemplos inventados:

```bash
curl -sS http://127.0.0.1:8080/api/paquete-evaluacion \
  | jq '.evidencia.ultimaAuditoria'
```

Todas las órdenes, wallets y resultados son simulados. El sistema no usa llaves privadas, no firma órdenes y no arriesga capital.
