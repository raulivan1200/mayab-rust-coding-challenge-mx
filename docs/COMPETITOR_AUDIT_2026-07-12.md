# Auditoría competitiva — corte 2026-07-12

Esta revisión usa repositorios públicos observables, no nombres de proyecto
aislados. El universo se obtuvo buscando entregas públicas de Coding Challenge
México; el orden final puede cambiar y sólo el jurado conoce sus ponderaciones.

## Veredicto de juez exigente

Mayab ya compite por profundidad técnica y tiene captura pública sellada,
verificación de corpus y evaluación A/B/C funcionando end-to-end. Todavía no
puede reclamar el primer lugar por evidencia pública: falta ejecutar y publicar
un corpus largo que supere los gates de escala. La barrera ya no es código ni
otro algoritmo; es tiempo de observación real sin seleccionar sólo ventanas
favorables.

| Rival verificable | Ventaja que amenaza a Mayab | Respuesta de Mayab | Brecha restante |
|---|---|---|---|
| [ArbitrAI](https://github.com/JoahanMorales/CODING_CHALLENGE_MEXICO) | Declara 60,716 rondas, 23 dislocaciones net-positive y un estudio de 3.6M de dislocaciones reales | Captura real multi-venue, corpus deduplicado, scan streaming, hash de costos y holdout A/B/C ya funcionan | Acumular y publicar ≥1M eventos, ≥10 shards y ≥24 h capturadas; hoy el smoke declara `insufficient_scale` |
| [Aurelion](https://github.com/rvvictor/Challenge-CODING-CHALLENGE-MEXICO) | 142 pruebas declaradas, 50 parámetros, ciclos de cuatro pasos, research lab y narrativa experimental honesta | Release gate verde, contratos HTTP, invariantes, corpus/ledger y benchmarks reproducibles | Simplificar la primera impresión y publicar resultados del corpus largo |
| [Atalaya](https://github.com/HumbertoBernal/atalaya-arb) | Explicación inmediata, laboratorio paralelo, 46 parámetros y demo Vercel fácil de abrir | Backend stateful, auditoría durable, telemetría e integridad por feed | La landing de Mayab aún presenta más conceptos de los que un juez retiene |
| [Filobot](https://github.com/ImanolD/btc_arbitrage_cchallenge) | UX pulida, tour, copiloto y WhatsApp; tesis EV muy fácil de defender | Sin dependencia de LLM, paquete de evaluación estructurado y un solo binario | Grabar un video impecable y reducir fricción de la demo pública |

Los conteos anteriores son afirmaciones de los README rivales observados en la
fecha de corte. No equivalen por sí solos a cobertura, calidad o reproducibilidad.

## Universo público localizado

Además de los cuatro rivales anteriores, la búsqueda pública encontró entregas
de JoahanMorales, UzielTzab (dos repos), JaavRJ, Seebaastiaan, crazy-valter,
omarbramirez, aarmentah, AlanPidal, abiside, TacosyHorchata, aliyatdev,
kryptomarireal, ChrisMoCa, ManuelCanulDev y Humol-e. “17 finalistas” no implica
que cada resultado de búsqueda sea finalista: remakes, repos duplicados o
entregas fuera de corte deben confirmarse contra la lista oficial.

## Ranking por evidencia pública, no por promesas

1. **ArbitrAI** mientras su dataset y scripts de millones de observaciones sean
   descargables y reproducibles. Si sólo existe la cifra en README, baja.
2. **Mayab** por release gate verde, captura/corpus verificable, integridad de
   libro, evaluación cronológica y el caso memorable de pierna B rechazada con
   unwind y cero BTC residual. Sube al 1 cuando publique escala real suficiente.
3. **Aurelion** por amplitud cuantitativa, pruebas y honestidad experimental.
4. **Atalaya / Filobot**, con posibilidad real de superar a Mayab si el jurado
   pondera claridad y UX más que profundidad del backend.

No hay base honesta para garantizar top 1–3. Sí hay una ruta verificable para
maximizar la probabilidad.

## Lo que falta para ser el rival más fuerte

### P0 — antes de entregar

- CI verde sobre el SHA exacto: fmt, clippy, tests Rust, contratos HTTP y E2E.
- Deploy público sin cold start durante la evaluación y recorrido cerrado sin
  token para `demo/final` y `demo/caos`.
- Paquete de evidencia sellado con commit, dirty flag, timestamps y SHA-256.
- Video de 90–120 segundos: mercado público → decisión neta → fallo de segunda
  pierna → unwind → `0 BTC` → evidencia descargable.
- Corregir todos los números contradictorios de pruebas (129/152/156) y citar
  únicamente el conteo producido por la corrida CI entregable.

### P1 — ejecución pendiente capaz de destronar la evidencia de ArbitrAI

- Ejecutar `capture-corpus` hasta superar los gates públicos: ≥1M eventos,
  ≥10 shards, ≥2 venues y ≥24 horas realmente capturadas.
- Publicar JSONL con procedencia, manifiestos, corpus index, scan, hashes y
  script de reproducción; el pipeline ya existe y el smoke real está verde.
- Reportar eventos, ventanas, rutas, frescura, gaps, costos, edge bruto/neto y
  tasa que sobrevive; separar observación de ejecución paper.
- Ejecutar baseline y GA sobre el mismo holdout temporal y publicar intervalos,
  no sólo el mejor resultado.

### P2 — defensa de arquitectura

- Ejecutar benchmarks pareados antes de afirmar multiplicadores de lenguaje.
- Mantener la frase defendible: **Rust en el camino crítico; vanilla JS en la
  última milla: sin GC en el motor, sin hidratación en el dashboard y sin una
  cadena de servicios entre la señal y la evidencia.**
- Explicar que Rust reduce fuentes de jitter y overhead arquitectónico, pero no
  elimina latencia de red ni vuelve lento por definición a Node/Python/Java.

## Preguntas con las que un juez intentará tumbar Mayab

1. ¿Dónde está el tape real y cómo pruebo que no fue fabricado?
2. ¿Por qué 10 exchanges importan si sólo dos libros están frescos ahora?
3. ¿El GA mejora out-of-sample o sólo optimiza el simulador que ustedes crearon?
4. ¿Qué ocurre entre `LEG_A_FILLED` y el unwind, y cuánto cuesta quedar plano?
5. ¿La latencia publicada es red, quote-to-decision o cómputo puro?
6. ¿Por qué debo creer “Rust más rápido” sin una implementación equivalente?
7. ¿Qué queda después de reiniciar Cloud Run si SQLite vive en `/tmp`?

Mayab debe responder cada una con endpoint, archivo, comando o artefacto; una
respuesta narrativa sin evidencia cuenta como fallo.
