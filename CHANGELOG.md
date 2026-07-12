# Changelog

This project follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and semantic versioning for tagged releases.

## [Unreleased]

### Added
- Prometheus histograms and counters for bounded pipeline stages and HTTP latency.
- Read-only `/operator` console backed by live engine state.
- Architecture, security, exchange-extension and operations documentation.
- Community health files and multi-platform release automation.
- Structured issue and pull-request templates with explicit simulation safety checks.
- Reproducible LCOV coverage workflow with artifact retention and optional Codecov publishing.
- SHA-256 checksums and CycloneDX SBOMs for release artifacts.
- Audited `TOP2_CHECKLIST.md` mapping finalist recommendations to verifiable evidence.
- Open Graph and social preview metadata using repository screenshots.

## [0.1.0] - 2026-07-12

### Added
- **Modo Jurado (Readiness)**: Nuevo panel en el frontend con 5 tarjetas interactivas (Parametrización, Robustez, Wallets, UI/UX, Documentación) indicando el cumplimiento de la rúbrica de evaluación de manera inmediata.
- **Decision Inspector**: Desglose detallado de las oportunidades evaluadas con insignias legibles (badges) que muestran códigos como `ACEPTADA`, `RECHAZADA_STALE`, `RECHAZADA_WALLET`, etc.
- **Percentiles de Latencia (p50/p99)**: Cálculo en el motor Rust y visualización en el ranking de latencias en el Dashboard para evaluar estabilidad estadística más allá de picos y promedios.
- **Costos de Rebalanceo**: Acumulación y renderizado en la sección de carteras, visibilizando el desgaste de P&L debido al mantenimiento de liquidez entre exchanges.
- **Documentación Técnica**: Se agregaron `ARCHITECTURE.md` y `DEMO_SCRIPT.md` para facilitar las pruebas controladas por parte de los jueces.

### Changed
- **Exportación CSV**: Se inyectaron configuraciones serializadas y parámetros de decisión, junto con `decision_code` y `decision_reason` en la bitácora de auditoría.
- **UI/UX Refinada**: Los layouts del frontend se adaptaron para albergar nuevas métricas preservando los 60 fps e integrando CSS animado para métricas de latencia.
