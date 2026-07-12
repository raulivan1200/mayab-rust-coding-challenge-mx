# ADR 0004: almacenamiento híbrido para corpus cuantitativo

- Estado: aceptado
- Fecha: 2026-07-12

## Contexto

Mayab necesita acumular y evaluar millones de eventos públicos sin degradar la
captura, inflar conteos ni depender de una base central. Guardar cada delta como
fila SQLite durante el hot path introduce transacciones, crecimiento del WAL y
contención. Mantener únicamente JSONL permite auditoría, pero buscar cientos de
shards requiere abrir muchos manifiestos.

## Decisión

Se usa una arquitectura de tres capas:

1. **JSONL append-only por shard** como evidencia autoritativa y portable.
2. **SQLite transaccional de metadatos** como índice local reconstruible.
3. **Parquet derivado** como formato analítico opcional futuro; nunca será la
   única fuente ni reemplazará hashes/manifiestos.

SQLite almacena corpus, shards, hashes, ventanas y conteos por exchange. No
recibe eventos desde los productores WebSocket. El índice se genera después de
verificar el corpus y puede reconstruirse desde los manifiestos.

## Propiedades

- Captura sin escrituras SQLite en el camino crítico.
- Shards pequeños y cuarentena individual.
- SHA-256 por shard y corpus.
- Transacción atómica con foreign keys.
- WAL y `synchronous=FULL`.
- Reinserción idempotente del mismo corpus.
- Consultas rápidas por tiempo, dataset y exchange.

## Consecuencias

El índice duplica metadatos, no eventos. Para scans columnares se añadirá un
export Parquet derivado cuando la evaluación deje de materializar todo en RAM.

## Alternativas descartadas

- **SQLite para cada evento:** añade contención al hot path.
- **Un JSONL gigante:** difícil de reanudar y poner en cuarentena.
- **Solo Parquet:** menos apropiado como log incremental autoritativo.
- **Postgres obligatorio:** reduce reproducibilidad local.

