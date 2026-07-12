# ADR 0002: GA multiobjetivo con selección NSGA-II

## Contexto

El motor debe equilibrar utilidad, Sharpe, drawdown y tasa de acierto sin esconder esos compromisos dentro de una sola cifra.

## Decisión

Se mantiene una población con cuatro objetivos: PnL, Sharpe, drawdown negado y win rate. La selección usa dominancia, non-dominated sorting, rank y crowding distance. El contrato público expone el primer frente en `frontera_pareto`.

Para convertir el frente en una decisión operativa determinista, el campeón es el genoma con mayor fitness escalar ajustado por riesgo dentro del primer frente no dominado. La frontera conserva alternativas; el fitness decide el punto que usa el motor.

## Consecuencias

- El dashboard y `/api/ga/estado` muestran trade-offs reales y la política de selección del campeón.
- Las pruebas cubren dominancia, crowding y publicación de una frontera no vacía tras evolucionar.
- El resultado sigue siendo una simulación en memoria; no habilita trading real.
