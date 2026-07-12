# Protocolo de evidencia cuantitativa

Este protocolo define qué debe existir antes de afirmar que Mayab observó una
cantidad determinada de eventos o dislocaciones. Su objetivo es permitir una
historia cuantitativa grande sin confundir volumen de mensajes, oportunidades,
operaciones simuladas y resultados sintéticos.

## Unidades que no deben mezclarse

- **Evento de mercado:** snapshot o delta recibido desde un WebSocket público,
  o snapshot REST explícitamente marcado como fallback.
- **Libro reconstruido:** estado válido por exchange y par después de aplicar un
  evento y superar orden, secuencia, checksum y lados no vacíos.
- **Candidato:** comparación temporalmente alineada entre dos libros ruteables.
- **Dislocación bruta:** candidato cuyo mejor bid supera el mejor ask.
- **Dislocación neta:** candidato que sigue positivo después de fee taker,
  profundidad, slippage, latencia, retiro amortizado y basis USD/USDT.
- **Operación paper:** ejecución simulada; nunca equivale a una orden real.

Un único cambio de libro puede generar varios candidatos. Por eso el reporte
debe publicar los seis conteos anteriores y no llamar “dislocaciones” a todos
los mensajes recibidos.

`evaluate-tape` publica estos conteos bajo `quantitativeFunnel`, incluyendo
quotes inválidos por causa y tasas brutas/netas por millón de quotes. Cada
estrategia añade `holdoutFunnel`, donde los fills paper permanecen separados de
las dislocaciones observadas. El Markdown generado conserva el mismo embudo.

La estrategia llamada campeón se selecciona exclusivamente con la calibración
B y queda registrada como `preregisteredChampion` antes de ejecutar C. El mejor
resultado retrospectivo de C se publica aparte como `exPostHoldoutWinner`,
incluso cuando derrota al campeón. El campo `championWonHoldout` hace visible
esa derrota y evita reetiquetar al ganador ex post como si hubiera sido elegido
de antemano.

## Identidad de un shard

Cada tape contiene `events.jsonl`, `capture-config.json` y `manifest.json`. El
manifiesto incluye `datasetId`, clasificación, commit, ventana temporal,
eventos por exchange, bytes y SHA-256. `verify-tape` reconstruye los libros y
recalcula hashes y conteos.

## Identidad de un corpus

Un corpus es un directorio cuyos hijos son tapes verificados. `verify-corpus`:

1. acepta únicamente `public_market_capture`;
2. rechaza dos shards con el mismo SHA-256;
3. rechaza ventanas solapadas sobre el mismo exchange/par aunque sus hashes sean distintos;
4. usa suma con detección de overflow;
5. agrega exchanges, pares, bytes y duración realmente capturada;
6. genera `corpusSha256` desde la lista ordenada de hashes.

```bash
cargo run -p mayab-cli --bin verify-corpus -- \
  --root artifacts/tapes \
  --output artifacts/evidence/corpus.json
```

## Gate para publicar cifras

Una cifra pública debe acompañarse de:

- SHA del commit y `corpusSha256`;
- número de shards únicos;
- ventana UTC y exchanges/pares;
- eventos y bytes totales;
- gaps, resyncs y fallback REST;
- definición exacta de dislocación;
- costos usados y split cronológico;
- resultados negativos y shards rechazados.

La meta “un millón” se considera alcanzada únicamente cuando
`totalEvents >= 1_000_000` en un corpus verde. Superar un millón de eventos no
implica haber observado un millón de dislocaciones. Esta última afirmación
requiere el contador específico de dislocaciones netas y su metodología.

`evidenceGates.status` permanece `insufficient_scale` hasta cumplir a la vez:
dos venues, diez shards únicos, un millón de eventos y 24 horas realmente
capturadas, política de entrega sin drops de aplicación y una tasa de gaps de
secuencia menor o igual a 1%. La diferencia entre `observedSpanMs` y
`totalCaptureDurationMs` evita presentar huecos entre sesiones como horas de
observación efectiva.

Los productores esperan (`await`) cuando el canal acotado está lleno en lugar
de descartar eventos dentro de la aplicación. Una desconexión causada por
backpressure o red no se oculta: el siguiente evento útil incrementa
`reconnectEvents` y conserva su `connectionEpoch`. Al cerrar un shard, los
productores detectan el receptor cerrado y terminan; no permanecen conectores
huérfanos entre shards rotatorios.

## Escalado recomendado

- Capturas rotatorias de 30–60 minutos para limitar el radio de corrupción.
- Al menos dos venues simultáneos y un mismo instrumento normalizado.
- Publicar primero 100k eventos, después 1M y finalmente varias sesiones/días.
- Congelar el corpus antes de calibrar; entrenamiento A, calibración B y
  holdout C permanecen cronológicos.
- Nunca seleccionar únicamente sesiones rentables.

La captura rotatoria automatiza este patrón y verifica cada shard antes de
continuar. Los shards fallidos se renombran con prefijo `failed-` y quedan en
cuarentena; no entran al corpus ni se borran silenciosamente:

```bash
cargo run -p mayab-cli --bin capture-corpus -- \
  --root artifacts/tapes/btc-usd-july \
  --total 24h \
  --shard 30m \
  --pair BTC/USD \
  --exchanges Binance,Kraken,Coinbase,OKX \
  --depth 10
```

La numeración continúa desde el último shard presente, por lo que una nueva
corrida puede ampliar el corpus sin sobrescribir capturas anteriores. Al
terminar se generan `corpus.json` y `corpus.sqlite`. SQLite indexa corpus,
shards y exchanges; los eventos permanecen en JSONL y fuera del hot path. La
decisión se documenta en
[`ADRs/0004-corpus-storage.md`](ADRs/0004-corpus-storage.md).

Para contar el embudo sobre millones de eventos sin materializar el corpus en
RAM, `scan-corpus` hace una pasada secuencial y conserva solo los 50 mejores
niveles de cada libro activo:

```bash
cargo run -p mayab-cli --bin scan-corpus -- \
  --root artifacts/tapes/btc-usd-july \
  --output artifacts/evidence/corpus-scan.json
```

El reporte enlaza `corpusSha256` y `costModelSha256`, documenta el algoritmo y
publica eventos, libros válidos, candidatos y dislocaciones brutas/netas/con
liquidez. Cada tasa incluye el estimado por millón y un intervalo Wilson 95%
(`grossRate95`, `netRate95`, `liquidNetRate95`), de modo que una captura pequeña
expone explícitamente su incertidumbre en vez de aparentar precisión por la
normalización. El denominador es `rawEvents` y el caso sin eventos se publica
como intervalo completo `[0, 1]`. Su memoria es `O(venues × pairs × depth)`; no entrena el GA ni elige
un ganador. La evaluación A/B/C sigue siendo un paso separado y más pesado.

También registra `processingDurationMs`, `eventsPerSecond`,
`maxActiveBooks` y `maxLevelsInMemory`. Los máximos describen estructuras del
algoritmo, no RSS exacto del proceso; cualquier cifra pública de memoria debe
medirse además con una herramienta del sistema operativo.

`capture-corpus` genera este scan al finalizar. `/api/research/tapes` lo expone
como `quantitativeScan` únicamente cuando su `corpusSha256` coincide con el
reporte visible; un JSON malformado o de otro corpus falla cerrado. El menú
superior puede entonces mostrar eventos y dislocaciones netas sin cruzar
artefactos incompatibles.

Al cerrar la captura también se genera `evidence-seal.json`, que encadena los
SHA-256 de `corpus.json`, `corpus-scan.json` y `corpus.sqlite`. Antes de hashear
SQLite se fuerza un checkpoint WAL para obtener un archivo autocontenido. La API
oculta el scan si el sello falta o cualquiera de las tres huellas cambió.

```bash
cargo run -p mayab-cli --bin verify-corpus-seal -- \
  artifacts/tapes/btc-usd-july
```

Antes de iniciar una captura larga, el smoke de 40 segundos comprueba TLS, dos
venues, captura, cuarentena, verificación del corpus y evaluación A/B/C:

```bash
./scripts/smoke-research-corpus.sh
```

Debe terminar en `insufficient_scale`; declararse publicable con una sola
ventana corta sería un fallo del gate, no un éxito.

El replay interactivo usa los timestamps capturados, monotoniza únicamente los
empates o retrocesos de reloj y desactiva la adversidad aleatoria dentro del
sandbox. Su respuesta incluye `inputSha256`, `determinista: true` y la política
de reloj. Repetir el mismo tape debe producir exactamente el mismo resumen sin
modificar wallets, PnL, GA ni libros live.
