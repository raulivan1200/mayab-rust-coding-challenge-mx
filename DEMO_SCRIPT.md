# Guía de Pruebas (Demo Script) para Jurado

Este documento detalla el paso a paso para probar todas las funcionalidades principales, mecanismos de robustez y algoritmos del motor.

## 1. Validación de Readiness Inicial
- **Acción:** Al cargar el dashboard (http://127.0.0.1:8080), observe la sección central **"Readiness"** (Modo Jurado).
- **Expectativa:** 5 tarjetas deben mostrar "Ok" validando la parametrización, robustez, soporte multi-wallet, métricas de latencia y documentación.

## 2. Inyectar Oportunidad (Demo Rentable)
- **Acción:** En la sección "Demo controlada", presione **"Demo rentable + GA"**.
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
  3. Las siguientes inyecciones de "Demo rentable" serán rechazadas con `RECHAZADA_CIRCUIT_BREAKER`.

## 5. Escenario: Rebalanceo de Carteras
- **Acción:** Presione **"Forzar rebalanceo"**.
- **Expectativa:** 
  1. En la tabla "Wallets -> Rebalanceos" aparecerá un nuevo registro.
  2. En el panel "Carteras", el renglón de "Total Costos Reb." aumentará en color rojo.

## 6. Generación y Exportación de Evidencia
- **Acción:** En la sección "Qué está pasando ahora", haga click en **"Exportar CSV"**.
- **Expectativa:** Se descargará un CSV completo que contiene en la parte superior el volcado de la configuración usada, y abajo la sábana completa de transacciones y decisiones algorítmicas que explican el comportamiento del sistema.
