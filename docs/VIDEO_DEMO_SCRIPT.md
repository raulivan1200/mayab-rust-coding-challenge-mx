# Demo en Video (3 Minutos) - Mayab Arbitraje BTC

Este guion está diseñado para grabar una demostración de 3 minutos del dashboard de Mayab Arbitraje BTC para una auditoría o presentación ejecutiva.

## 0:00 - 0:30 | Introducción y Readiness
- **Voz/Texto:** "Bienvenidos a la demo de Mayab Arbitraje BTC. Esta es una simulación de alta frecuencia basada en Rust, diseñada para evaluar algoritmos sin exposición a riesgos reales."
- **Acción:** Muestra la pantalla principal del Dashboard.
- **Acción:** Haz un scroll ligero hacia la sección de "Readiness". Destaca cómo los indicadores principales están en verde y muestran el estado actual del sistema (parametrización, latencias, estado del GA).

## 0:30 - 1:15 | Demostración de Escenarios de Mercado Rentable
- **Voz/Texto:** "Vamos a inyectar oportunidades simuladas y ver cómo reacciona el motor en tiempo real."
- **Acción:** Presiona **"Ejecutar prueba completa"** en la portada. El mismo recorrido también está disponible como **"Preparar demo auditada"** o **"Preparar recorrido completo"** dentro del dashboard.
- **Acción:** Muestra cómo el "PnL Acumulado" (Profit and Loss) sube inmediatamente en la tarjeta superior.
- **Acción:** Haz clic en una operación en verde en la tabla inferior y muestra el **Decision Inspector** (Forense) que explica por qué la operación fue aceptada (e.g. `ACEPTADA_RENTABILIDAD_RUTA`).

## 1:15 - 2:00 | Simulación de Caos y Resiliencia (Circuit Breaker)
- **Voz/Texto:** "El motor de arbitraje está diseñado para ser resiliente. Vamos a probar un escenario donde el mercado se vuelve caótico o perdemos conexión."
- **Acción:** Selecciona la pestaña "Prueba de Caos". Presiona el botón de **"Prueba de caos completa"**.
- **Acción:** Muestra el banner rojo de "Circuit Breaker Activo" y el "Modo de Operación" en estado de Alerta.
- **Acción:** Muestra que el panel de "Evidencia" confirma que el sistema abortó las ejecuciones en riesgo y que la exposición residual es exactamente 0 BTC.

## 2:00 - 2:30 | Evolución y Algoritmos Genéticos
- **Voz/Texto:** "Para adaptarnos, el sistema utiliza un Algoritmo Genético de objetivo único que corre en segundo plano y mejora los umbrales de decisión."
- **Acción:** Abre la pestaña de "Laboratorio y Genético".
- **Acción:** Muestra la población y las generaciones. Presiona "Evolucionar Ahora". Muestra cómo los hiperparámetros cambian instantáneamente basándose en la historia reciente.

## 2:30 - 3:00 | Auditoría y Exportación de Datos
- **Voz/Texto:** "Finalmente, todos estos eventos son deterministas y pueden ser auditados."
- **Acción:** Presiona el botón "Exportar CSV".
- **Acción:** Muestra brevemente el archivo descargado o explica que contiene una hoja forense que detalla el costo de latencia, slippage y decision code para cada trade.
- **Voz/Texto:** "Con esto concluimos la demostración de resiliencia y ejecución del Motor Mayab. Gracias."
