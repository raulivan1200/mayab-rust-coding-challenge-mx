# Evidencia de Validación Final (Mayab Arbitraje BTC)

Este documento indica cómo obtener evidencia vigente del binario que se está evaluando. Separa *Real-market paper result* (feeds públicos y ejecución simulada) de *Synthetic demo result* (escenarios artificiales etiquetados). Las métricas dinámicas no se copian aquí como constantes porque latencias, feeds y contadores cambian entre revisiones e instancias.

## 1. Rúbrica de Criterios (Evaluación Automática `/api/jurado`)

`/api/jurado` y `/api/preflight` calculan sus checks desde el estado actual. Para preparar una corrida reproducible y consultar la evidencia:

```bash
curl -sS -X POST http://127.0.0.1:8080/api/demo/final
curl -sS http://127.0.0.1:8080/api/jurado | jq '{estado, checks, rubricaOficial}'
curl -sS http://127.0.0.1:8080/api/preflight | jq '{listo, judgeReadiness, checks}'
```

El gate automatizado completo es `./scripts/release-check.sh`; además de compilar y probar, exige PnL demo positivo, GA activo, fill parcial, rebalanceo, auditoría y reconciliación de segunda pierna sin exposición residual.

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

## 5. Decision Inspector

La auditoría real de la corrida se consulta sin ejemplos inventados:

```bash
curl -sS http://127.0.0.1:8080/api/paquete-evaluacion \
  | jq '.evidencia.ultimaAuditoria'
```

Todas las órdenes, wallets y resultados son simulados. El sistema no usa llaves privadas, no firma órdenes y no arriesga capital.
