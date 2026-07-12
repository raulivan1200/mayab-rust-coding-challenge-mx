# Guía de Pruebas (Demo Script) para Jurado

Este documento detalla el paso a paso para probar todas las funcionalidades principales, mecanismos de robustez y algoritmos del motor.

La aplicación pública deja la evidencia GET disponible para el jurado, pero reserva los controles mutables al operador. Para ejecutar botones o POST en producción, configura `localStorage.mayabAdminToken` desde una sesión controlada o envía `Authorization: Bearer <ADMIN_TOKEN>`; nunca compartas el token en una URL o captura. En desarrollo local el token es opcional.

## Recorrido opcional de 2 minutos

- **Acción:** En el encabezado del dashboard, pulse el botón flotante **Recorrido 2 min** (`#tutorialToggle`).
- **Expectativa:** La guía interactiva iniciará y cambiará automáticamente de pestaña, aplicando la clase `.tutorial-highlight` a los siguientes contenedores DOM específicos:
  
  1. **Lectura ejecutiva en 15 segundos:** Resalta la franja superior de resumen (`.llm-strip`, `#resumenLlm`), mostrando el estado simulado, PnL, y mejor ruta.
  2. **Order books públicos en vivo:** Cambia a la pestaña "Mercado y rutas" (`#tab-mercado`) y resalta la cinta de precios (`.mercado`, `#exchangeLista`), donde se mide la latencia de los WebSockets.
  3. **De spread bruto a utilidad neta:** Resalta el mapa de arbitraje (`.mapa`, `#canvasMapa`), demostrando cómo se calculan las rutas restando fees y slippage.
  4. **Robustez que se puede provocar:** Cambia a la pestaña "Riesgo y escenarios" (`#tab-riesgo`) y destaca la sala de pruebas (`.demo-panel`), exponiendo controles manuales para inyectar adversidad (`#btnDemoCaos`, `#btnResetDemo`).
  5. **Baseline vs campeón GA:** Navega a la pestaña "Auditoría y backtest" (`#tab-logs`) y resalta el validador multisemilla (`.replay-panel`), que verifica al motor con intervalos de confianza P05-P95.
  6. **Optimización evolutiva explicable:** Termina en la pestaña "Optimización GA" (`#tab-galab`) destacando el laboratorio genético (`.ga-panel`), donde los umbrales (tamaño, spread, latencia permitida) se ajustan a la vista.

- **Atajo reproducible:** En la sección "Demo controlada" (`.demo-panel`), use **Reiniciar corrida de jurado** (`#btnResetDemo`) antes de inyectar oportunidades con **Preparar recorrido completo** (`#btnDemoFinal`). El reset conserva feeds públicos, pero limpia carteras (`#balances`), PnL (`#pnlLiveTitle`), riesgo y el estado GA para una evaluación limpia.

## 1. Validación de Readiness Inicial
- **Acción:** Al cargar el dashboard (http://127.0.0.1:8080), observe la sección central **"Readiness"** (Modo Jurado).
- **Expectativa:** 5 tarjetas deben mostrar "Ok" validando la parametrización, robustez, soporte multi-wallet, métricas de latencia y documentación.

## 2. Inyectar Oportunidad (Demo Rentable)
- **Acción:** Pulse **"Preparar demo auditada"** en el resumen o desplácese a "Demo controlada" y pulse **"Preparar recorrido completo"**. La acción explícita reinicia primero la corrida simulada para que visitas previas no acumulen PnL.
- **Expectativa:** 
  1. El PnL debe incrementar.
  2. En "Ejecución -> Operaciones" debe aparecer una transacción rentable en verde.
  3. El panel "GA Lab" debe reportar que la generación avanzó y ajustó parámetros.

## 3. Revisar el "Decision Inspector"
- **Acción:** En la tabla "Oportunidades", haga click sobre alguna fila.
- **Expectativa:** El recuadro inferior "Forense" mostrará los desglose de costos (Slippage, Fees, Riesgo latencia) y un Badge colorido. Podrá ver el código `ACEPTADA` o `RECHAZADA_` junto con el razonamiento.

## 4. Escenario: Circuit Breaker
- **Acción:** En "Demo controlada", presione **"Circuit breaker"**.
- **Expectativa:** 
  1. Aparecerá un banner superior rojo alertando la detención de las ejecuciones.
  2. El "Modo de Operación" en la esquina superior izquierda se tornará Ámbar/Rojo.
  3. Las siguientes inyecciones de **"Repetir escenario rentable"** serán rechazadas con `RECHAZADA_CIRCUIT_BREAKER`.

## 4.1 Prueba de caos completa
- **Acción:** Presione **"Prueba de caos completa"**.
- **Expectativa:** El motor encadena fill parcial, baja liquidez, fallo de segunda pierna con unwind, circuit breaker, rebalanceo y recuperación. El resultado debe mostrar `8/8 checks`, exposición residual `0 BTC` y circuit breaker restaurado.
- **API equivalente:** `curl -X POST http://127.0.0.1:8080/api/demo/caos -H "Authorization: Bearer ${ADMIN_TOKEN}"`.

## 5. Escenario: Rebalanceo de Carteras
- **Acción:** Presione **"Forzar rebalanceo"**.
- **Expectativa:** 
  1. En la tabla "Wallets -> Rebalanceos" aparecerá un nuevo registro.
  2. En el panel "Carteras", el renglón de "Total Costos Reb." aumentará en color rojo.

## 6. Generación y Exportación de Evidencia
- **Acción:** En la sección "Qué está pasando ahora", haga click en **"Exportar CSV"**.
- **Expectativa:** Se descargará un CSV completo que contiene en la parte superior el volcado de la configuración usada, y abajo la sábana completa de transacciones y decisiones algorítmicas que explican el comportamiento del sistema.
