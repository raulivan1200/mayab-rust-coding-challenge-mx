# Checklist Top 1–2

Estado auditado contra las dos revisiones comparativas recibidas el 12 de julio de 2026. Una casilla marcada significa que existe evidencia local verificable; no equivale a una calificación oficial.

## P0 — Lo que cambia la evaluación

- [x] Judge Mode concentrado en `GET /api/jurado`, con checks, scorecard, links y límites seguros.
- [x] Demo final reproducible mediante `POST /api/demo/final` sin depender de oportunidades reales.
- [x] Reporte GA contra baseline, holdout sin reentrenamiento, 24 semillas e intervalos en `/api/backtest` y `/api/research/*`.
- [x] Fallo de segunda pierna visible, unwind y exposición residual comprobable en `/api/demo/caos`.
- [x] Invariantes financieras y escenarios adversos cubiertos por tests del motor y auditoría del ledger.
- [x] Latencia separada en transporte, quote→decision y compute con p50/p95/p99 en `/api/latencias`.
- [x] Dashboard separa resumen, riesgo, auditoría, Evidence Lab y GA.
- [ ] Publicar el tag/release `v1.0.0-finalist` (requiere autorización y push del propietario).

## P1 — Confianza de repositorio

- [x] `SECURITY.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md` y `CHANGELOG.md`.
- [x] Plantillas estructuradas para bugs, features y pull requests.
- [x] CI de fmt, clippy, tests, release build, smoke, contenedor non-root y seguridad.
- [x] Cobertura LCOV reproducible y publicación opcional a Codecov.
- [x] Release artifacts con checksum SHA-256 y SBOM CycloneDX.
- [x] Benchmarks, Prometheus, documentación operativa y ADRs.
- [ ] Configurar `CODECOV_TOKEN` y hacer visible el badge después de la primera corrida válida.
- [ ] Crear release pública y adjuntar artefactos; no se hace automáticamente desde este checklist.

## P2 — Presentación final

- [x] Recorrido y defensa técnica documentados.
- [x] Primera pantalla enlaza evidencia completa y declara capital real igual a cero.
- [x] Consola `/operator` separada del dashboard de presentación.
- [x] Guion de video disponible; producir el video queda explícitamente fuera de alcance.
- [ ] Ejecutar smoke contra la URL pública después del siguiente deploy.
- [ ] Congelar el SHA entregable en `FINAL_SUBMISSION.md` al momento de publicar el release.

## Orden de cierre recomendado

1. Dejar CI y cobertura verdes en el SHA final.
2. Desplegar y ejecutar `/healthz`, `/api/preflight`, `/api/resumen-llm`, `/api/ga/estado` y el dashboard.
3. Crear `v1.0.0-finalist`, publicar release notes y registrar el SHA.
4. Recorrer Judge Mode, demo rentable y segunda pierna fallida en menos de tres minutos.
5. No agregar features nuevas después del tag; solo corregir defectos bloqueantes con evidencia.
