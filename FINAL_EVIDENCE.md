# Evidencia Final (v1.0.0-jury)

Este archivo sirve como artefacto de evidencia de la entrega final para el jurado, correspondiente a la versión `v1.0.0-jury`.

## Cumplimiento de Criterios

Para verificar la alineación con la rúbrica oficial, el sistema proporciona varios mecanismos en tiempo real:

1. **Jury Mode (Rúbrica y Scorecard)**: 
   Visita `GET /api/jurado` en la aplicación desplegada para ver un mapeo directo de los 5 criterios contra el estado del sistema, evidencias en código y enlaces para comprobar cada punto.

2. **Paquete de Evaluación**:
   Visita `GET /api/paquete-evaluacion` para obtener el resumen de la ejecución actual, un backtest reproducible usando los algoritmos genéticos y evidencia forense en formato JSON, listo para ser consumido por un revisor técnico o automatizado.

3. **Endpoints de Auditoría**:
   Toda la operación simulada genera una huella conservada en memoria/SQLite durante el ciclo de vida de la instancia. Se puede acceder vía:
   - `GET /api/estado`
   - `GET /api/export/json`
   - `GET /api/export/csv`

4. **Resumen LLM**:
   Visita `GET /api/resumen-llm` para una lectura rápida del estado general de la aplicación, latencias, motor de decisión y algoritmo genético, en formato simplificado para LLMs.

### Despliegue Público
El proyecto está desplegado y verificado en Cloud Run, ofreciendo soporte real de WebSockets para el monitoreo en tiempo real de los 10 exchanges. Todos los controles de la interfaz gráfica y la API están operables.

Para más detalles, consulta la sección "Cómo cumple cada criterio" en el `README.md`.
