# Benchmarking reproducible

Los benchmarks miden capacidad técnica local; no son evidencia de mercado ni
rentabilidad. Toda cifra publicada debe incluir commit, hardware, sistema
operativo, versión de Rust, fecha y comando.

## Scan streaming del corpus

```bash
cargo bench -p mayab-arbitrage --bench corpus_scan_benchmark
```

El fixture contiene 100,000 snapshots generados localmente y se clasifica como
`synthetic_benchmark`. El gate de corpus devuelve siempre `synthetic_only`, aun
si el fixture alcanzara un millón de eventos o 24 horas simuladas.

La medición incluye:

- verificación SHA-256 y reconstrucción del shard;
- parsing JSONL;
- reconstrucción de libros top-50;
- alineación de venues por frescura;
- cálculo de costos canónicos;
- embudo bruto/neto/con liquidez.

Criterion reporta throughput en eventos/s. El fixture se crea fuera de la
región medida. La lectura del sistema de archivos puede beneficiarse del page
cache; por eso una cifra debe declarar si la corrida fue fría o caliente.

## Hot path del motor

```bash
cargo bench -p mayab-arbitrage --bench motor_benchmark
```

## Plantilla de publicación

```text
commit:
dirty worktree:
fecha UTC:
rustc:
OS/kernel:
CPU:
RAM:
disco/filesystem:
modo: cold-cache | warm-cache
eventos:
tiempo mediano:
throughput:
p95 (si aplica):
limitaciones:
```

No se comparan cifras contra Node, Python u otros proyectos si no usan el mismo
dataset, hardware, definición y región medida.
