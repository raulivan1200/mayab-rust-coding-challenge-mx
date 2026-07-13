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

La verificación SHA-256 usa lectura streaming con un buffer fijo de 256 KiB; el
tamaño del shard no se materializa en RAM. La reconstrucción mantiene como
estado únicamente los libros activos y su profundidad acotada.

Criterion reporta throughput en eventos/s. El fixture se crea fuera de la
región medida. La lectura del sistema de archivos puede beneficiarse del page
cache; por eso una cifra debe declarar si la corrida fue fría o caliente.

## Hot path del motor

```bash
cargo bench -p mayab-arbitrage --bench motor_benchmark
```

La operación medida recibe una cotización a través de `Motor`, accede al estado
y deja que el pipeline analice el evento. No mide red, TLS, WebSocket, escritura
durable ni render del navegador. Tampoco equivale a una dislocación de mercado:
es una iteración de ingreso al motor y debe publicarse con ese nombre.

### Corrida de referencia local

```text
commit: 71f88c3
dirty worktree: sí
fecha UTC: 2026-07-13T05:27:41Z
rustc: 1.96.0 (ac68faa20 2026-05-25)
OS/kernel: Darwin 25.5.0 arm64
CPU: Apple M4
modo: warm-cache, Criterion release/optimized
benchmark: motor_recibir_cotizacion
muestras: 100; ~1.8M iteraciones en ~5.0141 s durante la recolección
tiempo: [3.9809 µs, 4.0890 µs, 4.2066 µs]
throughput derivado de la mediana: ~244,559 iteraciones/s
limitaciones: worktree sucio; sin red, TLS, persistencia ni navegador; Criterion
reportó regresión contra su baseline local previo y 11 outliers
```

La cifra del dashboard sigue siendo la telemetría del proceso desplegado, no
esta referencia de laboratorio: rutas acumuladas, cómputo p50 y eventos/s se
actualizan desde `telemetriaPipeline`.

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
