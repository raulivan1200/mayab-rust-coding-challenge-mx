# Evidencia de Validación Final (Mayab Arbitraje BTC)

Este documento indica cómo obtener evidencia vigente del binario que se está evaluando. Separa *Real-market paper result* (feeds públicos y ejecución simulada) de *Synthetic demo result* (escenarios artificiales etiquetados). Las métricas dinámicas no se copian aquí como constantes porque latencias, feeds y contadores cambian entre revisiones e instancias.

## Scorecard recordable

- 10 exchanges públicos y 90 rutas lineales dirigidas.
- Suite Rust multi-capa; el conteo aprobado proviene del CI del SHA entregable.
- 24 semillas comunes para comparar baseline y campeón GA.
- 10,000 remuestras moving-block en la validación bootstrap.
- 0 BTC de exposición residual al terminar el escenario reproducible de segunda pierna rechazada.

La última cifra se verifica con `POST /api/demo/caos`: la FSM publica
`PENDING → LEG_A_FILLED → LEG_B_REJECTED → UNWIND_FILLED → RECONCILED_LOSS`,
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

La evidencia vigente se genera en tiempo de ejecución mediante `GET /api/ga/sensibilidad`. El reporte usa siete configuraciones reproducibles y 24 semillas holdout comunes; no se conservan cifras estáticas que puedan quedar desalineadas del código desplegado.

```bash
curl -sS http://127.0.0.1:8080/api/ga/sensibilidad | jq '{metodologia, resultados}'
```

> **Lectura correcta:** El endpoint compara configuraciones reproducibles de población, mutación y cruce sobre 24 semillas holdout comunes. Cada estrategia aplica su propio umbral, tolerancia de latencia y tamaño máximo. Es análisis de sensibilidad del GA híbrido, no una prueba causal aislada de cada operador interno.

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
- `cargo audit`
- `docker build --tag mayab-btc-arbitrage:ci .`

No debe marcarse esta sección como aprobada si la revisión publicada no tiene todos los checks verdes.

### Inventario de pruebas y regla anti-inflación

La suite separa pruebas de librería, auditor de ledger, integraciones y contratos
independientes de rutas públicas en `tests/public_contract_test.rs`. Playwright
se reporta por separado con cuatro recorridos, incluido el selector superior de
procedencia y escala. Como el árbol sigue cambiando, ningún conteo se presenta
como aprobado hasta que CI publique la corrida verde del SHA entregable.

Después de esa corrida se añadieron veintiséis casos pendientes de validación final:
cinco cubren backpressure, fallos y flush acotado de auditoría; cuatro cubren
atomicidad, límites y contrato cerrado de configuración; uno exige al menos 50
controles únicos con categoría, restricción y origen; dos impiden seleccionar
retrospectivamente al campeón usando el holdout; dos evitan doble conteo por
ventanas de mercado solapadas; uno exige replay determinista con la misma huella
de entrada aun si la configuración live activa adversidad aleatoria; uno prueba
compatibilidad de epochs/reconexiones con tapes previos; uno verifica que un
corpus nativo completo entra a A/B/C sin conversión manual; uno valida el índice
SQLite transaccional e idempotente; dos validan memoria acotada y hash canónico
del escáner streaming; dos impiden publicar un scan malformado o perteneciente a
otro corpus; uno garantiza que un benchmark sintético jamás supera el gate de
publicación; uno valida publicación JSON atómica sin temporales filtrados; y dos
validan los intervalos Wilson 95% incluso en extremos. El objetivo del árbol actual es 186, pero la UI
conserva 160 como última cifra verificada hasta una
nueva corrida verde.

Cada contrato HTTP corresponde a una URI distinta y falla con el nombre exacto
de la superficie rota. Un loop con muchas aserciones no se contabiliza como
muchas pruebas. La comparación con proyectos externos debe reportar por separado
conteo, cobertura de ramas, propiedades/invariantes y escenarios end-to-end;
ninguna de esas métricas sustituye a las demás.

## 5. Paquete sellado y procedencia

`OUT_DIR=artifacts/evidence/final ./scripts/generar-evidencia.sh` captura el
paquete de evaluación, auditoría, latencias, backtest, bootstrap, holdout,
microestructura, sensibilidad GA y preflight. El directorio incluye manifiesto
con commit/dirty-worktree y `SHA256SUMS` para detectar cambios posteriores.

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
