# ADR 3: Rust en el hot path y JavaScript nativo en la interfaz

## Contexto

Mayab mantiene feeds WebSocket de larga vida, normaliza libros, evalúa rutas,
simula riesgo y publica estado en un solo proceso. La prioridad no es afirmar que
un lenguaje siempre vence a otro, sino reducir fuentes de jitter y piezas
operativas en el camino que va de una cotización a una decisión.

## Decisión

Se usa **Rust + Tokio + Axum** para el proceso y **HTML/CSS/JavaScript nativo**
para el dashboard.

Rust aporta al diseño del hot path:

- memoria gestionada sin recolector de basura, evitando pausas de GC;
- concurrencia multihilo y tareas asíncronas sin limitar el motor a un único
  event loop;
- tipos, ownership y aritmética decimal que hacen explícitos estado compartido,
  errores y cálculos monetarios;
- un binario que integra feeds, motor, API y archivos estáticos.

Node.js y Python pueden ofrecer rendimiento suficiente para muchos sistemas de
mercado y tienen ecosistemas excelentes. En cargas CPU-bound, sus runtimes
gestionados pueden introducir GC, GIL en CPython o la necesidad de workers y
procesos adicionales. PHP-FPM está optimizado principalmente para ciclos HTTP
request/response; sostener feeds y estado en memoria suele requerir otro modelo
de ejecución. Estas son diferencias arquitectónicas, no una prueba de que todo
programa Rust sea automáticamente más rápido.

JavaScript nativo evita en este dashboard el runtime, la hidratación y el bundle
de un framework. El navegador recibe archivos estáticos desde el mismo binario y
abre directamente el WebSocket. Esta elección reduce dependencias y superficie
de despliegue; no implica que React, Vue o Svelte sean intrínsecamente lentos.

## Power phrase defendible

> **Rust en el camino crítico; vanilla JS en la última milla: sin GC en el motor,
> sin hidratación en el dashboard y sin una cadena de servicios entre la señal y
> la evidencia.**

## Evidencia y regla de comparación

La ventaja se valida con métricas del entregable, no con cifras genéricas:

- `GET /api/latencias` publica p50/p95/p99 y throughput del pipeline observado.
- `scripts/benchmark-cloud-run-regions.sh` conserva región, revisión y
  condiciones de red.
- El tamaño de assets puede medirse con `du` y la carga con las pruebas
  Playwright.

Mayab todavía no incluye implementaciones funcionalmente equivalentes en
Node.js, Python o PHP ejecutadas sobre el mismo hardware y dataset. Por ello no
publica multiplicadores como “Nx más rápido”. Si se añade esa comparación debe
fijar SHA, hardware, runtime, warm-up, carga, dataset, concurrencia, percentiles,
memoria y código equivalente.

## Consecuencias

- Mayor control del costo y del estado del hot path.
- Menos dependencias y un despliegue de una sola unidad.
- Curva de aprendizaje y tiempos de compilación mayores que en stacks dinámicos.
- La UI asume responsabilidad directa por accesibilidad, estado y manipulación
  segura del DOM que un framework podría estructurar.
- Ninguna elección de lenguaje elimina la latencia de red, la calidad de los
  feeds ni el riesgo de ejecución.
