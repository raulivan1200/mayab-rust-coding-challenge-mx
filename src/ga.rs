//! Algoritmo genético para ajustar la estrategia de selección de oportunidades.
//!
//! La población evoluciona pesos de scoring, umbral mínimo, tamaño máximo de
//! operación y tolerancia de latencia usando historial simulado o replay
//! sintético controlado.

use chrono::Utc;
use rand::{rngs::StdRng, Rng, SeedableRng};
use serde::{Deserialize, Serialize};

use crate::types::{EstadoGenetico, Operacion};

/// Parámetros de evolución del algoritmo genético.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConfigGa {
    #[serde(rename = "tamanoPoblacion")]
    pub tamano_poblacion: usize,
    #[serde(rename = "tasaMutacion")]
    pub tasa_mutacion: f64,
    #[serde(rename = "tasaCruce")]
    pub tasa_cruce: f64,
}

impl Default for ConfigGa {
    fn default() -> Self {
        Self {
            tamano_poblacion: 50,
            tasa_mutacion: 0.15,
            tasa_cruce: 0.72,
        }
    }
}

/// Estrategia derivada del mejor genoma disponible.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EstrategiaGa {
    pub pesos: [f64; 5],
    pub umbral_min_spread_bps: f64,
    pub max_operacion_btc: f64,
    pub tolerancia_latencia_ms: i64,
}

#[derive(Clone, Debug, PartialEq)]
struct Genoma {
    pesos: [f64; 5],
    umbral_min_spread_bps: f64,
    max_operacion_btc: f64,
    tolerancia_latencia_ms: i64,
    fitness: f64,
}

impl Genoma {
    fn base() -> Self {
        Self {
            pesos: [0.40, 0.20, 0.20, 0.10, 0.10],
            umbral_min_spread_bps: 0.65,
            max_operacion_btc: 0.18,
            tolerancia_latencia_ms: 4500,
            fitness: 0.0,
        }
    }

    fn random(rng: &mut StdRng) -> Self {
        let mut g = Self {
            pesos: [
                rng.gen_range(0.05..0.70),
                rng.gen_range(0.05..0.45),
                rng.gen_range(0.05..0.45),
                rng.gen_range(0.03..0.35),
                rng.gen_range(0.03..0.35),
            ],
            umbral_min_spread_bps: rng.gen_range(0.20..4.50),
            max_operacion_btc: rng.gen_range(0.03..0.60),
            tolerancia_latencia_ms: rng.gen_range(900..7000),
            fitness: 0.0,
        };
        g.normalizar();
        g
    }

    fn normalizar(&mut self) {
        let total: f64 = self
            .pesos
            .iter()
            .copied()
            .filter(|v| v.is_finite() && *v > 0.0)
            .sum();
        if total <= 0.0 {
            self.pesos = Self::base().pesos;
        } else {
            for peso in &mut self.pesos {
                *peso = (*peso).max(0.01) / total;
            }
        }
        self.umbral_min_spread_bps = self.umbral_min_spread_bps.clamp(0.10, 8.0);
        self.max_operacion_btc = self.max_operacion_btc.clamp(0.01, 1.25);
        self.tolerancia_latencia_ms = self.tolerancia_latencia_ms.clamp(250, 15_000);
    }
}

#[derive(Clone, Debug)]
/// Estado mutable del algoritmo genético.
///
/// Se mantiene en memoria dentro del motor; no requiere almacenamiento externo
/// para la demo. Los campos de población y RNG permanecen privados para poder
/// cambiar la implementación sin tocar el contrato del dashboard.
pub struct EstadoGa {
    pub config: ConfigGa,
    pub generacion: i64,
    pub mejor_fitness: f64,
    pub fitness_promedio: f64,
    pub retador_fitness: f64,
    pub diversidad: f64,
    pub mejores_pesos: [f64; 5],
    pub umbral_optimizado: f64,
    pub max_operacion_optimizada_btc: f64,
    pub tolerancia_latencia_ms: i64,
    pub operaciones_evaluadas: usize,
    pub fallos_evaluados: usize,
    pub mejora_generacional: f64,
    pub temperatura_annealing: f64,
    pub inyecciones_diferenciales: usize,
    poblacion: Vec<Genoma>,
    rng: StdRng,
}

impl Default for EstadoGa {
    fn default() -> Self {
        let mut rng = StdRng::seed_from_u64(0x4d41594142);
        let config = ConfigGa::default();
        let mut poblacion = Vec::with_capacity(config.tamano_poblacion);
        poblacion.push(Genoma::base());
        while poblacion.len() < config.tamano_poblacion {
            poblacion.push(Genoma::random(&mut rng));
        }
        Self {
            config,
            generacion: 0,
            mejor_fitness: 0.0,
            fitness_promedio: 0.0,
            retador_fitness: 0.0,
            diversidad: 1.0,
            mejores_pesos: [0.40, 0.20, 0.20, 0.10, 0.10],
            umbral_optimizado: 0.65,
            max_operacion_optimizada_btc: 0.18,
            tolerancia_latencia_ms: 4500,
            operaciones_evaluadas: 0,
            fallos_evaluados: 0,
            mejora_generacional: 0.0,
            temperatura_annealing: 1.0,
            inyecciones_diferenciales: 0,
            poblacion,
            rng,
        }
    }
}

impl EstadoGa {
    /// Devuelve la estrategia que el motor debe usar en el siguiente ciclo.
    pub fn estrategia(&self) -> EstrategiaGa {
        EstrategiaGa {
            pesos: self.mejores_pesos,
            umbral_min_spread_bps: self.umbral_optimizado,
            max_operacion_btc: self.max_operacion_optimizada_btc,
            tolerancia_latencia_ms: self.tolerancia_latencia_ms,
        }
    }

    /// Actualiza límites de población y tasas, aplicando rangos seguros.
    pub fn actualizar_config(&mut self, mut cfg: ConfigGa) {
        cfg.tamano_poblacion = cfg.tamano_poblacion.clamp(10, 300);
        cfg.tasa_mutacion = cfg.tasa_mutacion.clamp(0.0, 0.8);
        cfg.tasa_cruce = cfg.tasa_cruce.clamp(0.0, 1.0);
        self.config = cfg;
        self.ajustar_poblacion();
    }

    /// Evoluciona la población usando operaciones simuladas y conteo de fallos.
    pub fn evolucionar(&mut self, operaciones: &[Operacion], fallos: usize) {
        self.ajustar_poblacion();
        self.generacion += 1;
        self.operaciones_evaluadas = operaciones.len();
        self.fallos_evaluados = fallos;

        if operaciones.is_empty() {
            self.diversidad = (self.diversidad * 0.97).max(0.01);
            return;
        }

        let base = evaluar_ventana(operaciones, fallos);
        for genoma in &mut self.poblacion {
            genoma.fitness = fitness_genoma(genoma, &base);
        }
        self.poblacion
            .sort_by(|a, b| b.fitness.total_cmp(&a.fitness));

        let fitness_anterior = self.mejor_fitness;
        let mejor = self.poblacion[0].clone();
        self.mejor_fitness = mejor.fitness;
        self.retador_fitness = self
            .poblacion
            .get(1)
            .map(|g| g.fitness)
            .unwrap_or(mejor.fitness);
        self.mejora_generacional = (self.mejor_fitness - fitness_anterior).max(0.0);
        self.fitness_promedio =
            self.poblacion.iter().map(|g| g.fitness).sum::<f64>() / self.poblacion.len() as f64;
        self.diversidad = diversidad(&self.poblacion);
        self.mejores_pesos = mejor.pesos;
        self.umbral_optimizado = mejor.umbral_min_spread_bps;
        self.max_operacion_optimizada_btc = mejor.max_operacion_btc;
        self.tolerancia_latencia_ms = mejor.tolerancia_latencia_ms;

        let elite = self.poblacion.len().min(4);
        let mut siguiente = self
            .poblacion
            .iter()
            .take(elite)
            .cloned()
            .collect::<Vec<_>>();
        while siguiente.len() < self.config.tamano_poblacion {
            let padre = self.torneo(3);
            let madre = self.torneo(3);
            let mut hijo = if self.rng.gen_bool(self.config.tasa_cruce) {
                self.cruzar(&padre, &madre)
            } else {
                padre
            };
            self.mutar(&mut hijo);
            siguiente.push(hijo);
        }

        self.aplicar_recocido_simulado(&base, &mut siguiente, elite);
        self.inyectar_evolucion_diferencial(&base, &mut siguiente, elite);

        if self.diversidad < 0.04 {
            for genoma in siguiente.iter_mut().skip(elite).step_by(3) {
                *genoma = Genoma::random(&mut self.rng);
            }
        }

        for genoma in &mut siguiente {
            genoma.fitness = fitness_genoma(genoma, &base);
        }
        siguiente.sort_by(|a, b| b.fitness.total_cmp(&a.fitness));
        let mejor_hibrido = siguiente[0].clone();
        let fitness_antes_hibrido = self.mejor_fitness;
        self.mejor_fitness = mejor_hibrido.fitness;
        self.retador_fitness = siguiente
            .get(1)
            .map(|g| g.fitness)
            .unwrap_or(mejor_hibrido.fitness);
        self.mejora_generacional = (self.mejor_fitness - fitness_anterior).max(0.0);
        self.fitness_promedio =
            siguiente.iter().map(|g| g.fitness).sum::<f64>() / siguiente.len() as f64;
        self.diversidad = diversidad(&siguiente);
        self.mejores_pesos = mejor_hibrido.pesos;
        self.umbral_optimizado = mejor_hibrido.umbral_min_spread_bps;
        self.max_operacion_optimizada_btc = mejor_hibrido.max_operacion_btc;
        self.tolerancia_latencia_ms = mejor_hibrido.tolerancia_latencia_ms;
        if self.mejor_fitness > fitness_antes_hibrido {
            self.mejora_generacional = (self.mejor_fitness - fitness_antes_hibrido).max(0.0);
        }
        self.poblacion = siguiente;
    }

    /// Convierte el estado interno al contrato público del dashboard.
    pub fn public(&self) -> Option<EstadoGenetico> {
        let activo = self.generacion > 0 && self.operaciones_evaluadas > 0;
        Some(EstadoGenetico {
            activo,
            generacion: self.generacion,
            mejor_fitness: self.mejor_fitness,
            fitness_promedio: self.fitness_promedio,
            retador_fitness: self.retador_fitness,
            diversidad: self.diversidad,
            tasa_mutacion: self.config.tasa_mutacion,
            tasa_cruce: self.config.tasa_cruce,
            poblacion: self.config.tamano_poblacion,
            convergente: self.diversidad < 0.04,
            mejores_pesos: self.mejores_pesos.to_vec(),
            umbral_optimizado: self.umbral_optimizado,
            max_operacion_optimizada_btc: self.max_operacion_optimizada_btc,
            tolerancia_latencia_ms: self.tolerancia_latencia_ms,
            operaciones_evaluadas: self.operaciones_evaluadas,
            fallos_evaluados: self.fallos_evaluados,
            mejora_generacional: self.mejora_generacional,
            temperatura_annealing: self.temperatura_annealing,
            inyecciones_diferenciales: self.inyecciones_diferenciales,
            metaheuristicas: vec![
                "GA + recocido simulado".into(),
                "GA + evolucion diferencial".into(),
            ],
        })
    }

    /// Representación JSON compacta para `/api/ga/estado`.
    pub fn api_estado(&self) -> serde_json::Value {
        let activo = self.generacion > 0 && self.operaciones_evaluadas > 0;
        serde_json::json!({
            "activo": activo,
            "generacion": self.generacion,
            "mejorFitness": self.mejor_fitness,
            "fitnessPromedio": self.fitness_promedio,
            "retadorFitness": self.retador_fitness,
            "diversidad": self.diversidad,
            "tasaMutacion": self.config.tasa_mutacion,
            "tasaCruce": self.config.tasa_cruce,
            "convergencia": 1.0 - self.diversidad,
            "poblacion": self.config.tamano_poblacion,
            "operacionesEvaluadas": self.operaciones_evaluadas,
            "fallosEvaluados": self.fallos_evaluados,
            "convergente": self.diversidad < 0.04,
            "mejoresPesos": self.mejores_pesos,
            "umbralOptimizado": self.umbral_optimizado,
            "maxOperacionOptimizadaBtc": self.max_operacion_optimizada_btc,
            "toleranciaLatenciaMs": self.tolerancia_latencia_ms,
            "mejoraGeneracional": self.mejora_generacional,
            "temperaturaAnnealing": self.temperatura_annealing,
            "inyeccionesDiferenciales": self.inyecciones_diferenciales,
            "metaheuristicas": [
                "GA + recocido simulado",
                "GA + evolucion diferencial"
            ],
            "mejorGenoma": {
                "ponderacionUtilidad": self.mejores_pesos[0],
                "ponderacionFrescura": self.mejores_pesos[1],
                "ponderacionLiquidez": self.mejores_pesos[2],
                "ponderacionConfiabilidad": self.mejores_pesos[3],
                "ponderacionZScore": self.mejores_pesos[4],
                "umbralMinSpreadBps": self.umbral_optimizado,
                "maxOperacionBtc": self.max_operacion_optimizada_btc,
                "toleranciaLatenciaMs": self.tolerancia_latencia_ms
            },
            "timestamp": Utc::now(),
        })
    }

    fn ajustar_poblacion(&mut self) {
        if self.poblacion.len() > self.config.tamano_poblacion {
            self.poblacion.truncate(self.config.tamano_poblacion);
        }
        while self.poblacion.len() < self.config.tamano_poblacion {
            self.poblacion.push(Genoma::random(&mut self.rng));
        }
    }

    fn torneo(&mut self, n: usize) -> Genoma {
        let mut mejor = self.poblacion[self.rng.gen_range(0..self.poblacion.len())].clone();
        for _ in 1..n {
            let candidato = self.poblacion[self.rng.gen_range(0..self.poblacion.len())].clone();
            if candidato.fitness > mejor.fitness {
                mejor = candidato;
            }
        }
        mejor
    }

    fn cruzar(&mut self, a: &Genoma, b: &Genoma) -> Genoma {
        let mut hijo = a.clone();
        for i in 0..hijo.pesos.len() {
            if self.rng.gen_bool(0.5) {
                hijo.pesos[i] = b.pesos[i];
            }
        }
        if self.rng.gen_bool(0.5) {
            hijo.umbral_min_spread_bps = b.umbral_min_spread_bps;
        }
        if self.rng.gen_bool(0.5) {
            hijo.max_operacion_btc = b.max_operacion_btc;
        }
        if self.rng.gen_bool(0.5) {
            hijo.tolerancia_latencia_ms = b.tolerancia_latencia_ms;
        }
        hijo.fitness = 0.0;
        hijo.normalizar();
        hijo
    }

    fn mutar(&mut self, genoma: &mut Genoma) {
        if self.config.tasa_mutacion <= 0.0 {
            return;
        }
        for peso in &mut genoma.pesos {
            if self.rng.gen_bool(self.config.tasa_mutacion) {
                *peso += gauss(&mut self.rng, 0.0, 0.08);
            }
        }
        if self.rng.gen_bool(self.config.tasa_mutacion) {
            genoma.umbral_min_spread_bps += gauss(&mut self.rng, 0.0, 0.45);
        }
        if self.rng.gen_bool(self.config.tasa_mutacion) {
            genoma.max_operacion_btc += gauss(&mut self.rng, 0.0, 0.08);
        }
        if self.rng.gen_bool(self.config.tasa_mutacion) {
            genoma.tolerancia_latencia_ms += gauss(&mut self.rng, 0.0, 700.0) as i64;
        }
        genoma.normalizar();
    }

    fn aplicar_recocido_simulado(
        &mut self,
        base: &VentanaFitness,
        poblacion: &mut [Genoma],
        elite: usize,
    ) {
        if poblacion.is_empty() {
            return;
        }
        self.temperatura_annealing = (1.25 / (1.0 + self.generacion as f64 * 0.045)).max(0.05);
        let limite = poblacion.len().min((elite + 8).max(8));
        for genoma in poblacion.iter_mut().take(limite).skip(1) {
            genoma.fitness = fitness_genoma(genoma, base);
            let mut vecino = genoma.clone();
            self.mutar_fino(&mut vecino, self.temperatura_annealing);
            vecino.fitness = fitness_genoma(&vecino, base);
            let delta = vecino.fitness - genoma.fitness;
            let prob = if delta >= 0.0 {
                1.0
            } else if delta.is_nan() || self.temperatura_annealing.is_nan() {
                0.0
            } else {
                (delta / self.temperatura_annealing.max(0.01)).exp()
            };
            let aceptar = delta >= 0.0 || self.rng.gen_bool(prob.clamp(0.0, 1.0));
            if aceptar {
                *genoma = vecino;
            }
        }
    }

    fn inyectar_evolucion_diferencial(
        &mut self,
        base: &VentanaFitness,
        poblacion: &mut [Genoma],
        elite: usize,
    ) {
        if poblacion.len() < 8 {
            self.inyecciones_diferenciales = 0;
            return;
        }
        let inyecciones = ((poblacion.len() as f64 * 0.12).ceil() as usize).clamp(1, 18);
        self.inyecciones_diferenciales = inyecciones;
        let inicio = elite.min(poblacion.len().saturating_sub(1));
        for _ in 0..inyecciones {
            let destino = self.rng.gen_range(inicio..poblacion.len());
            let a = self.rng.gen_range(0..poblacion.len());
            let b = self.rng.gen_range(0..poblacion.len());
            let c = self.rng.gen_range(0..poblacion.len());
            if a == b || a == c || b == c {
                continue;
            }
            let mut candidato = poblacion[a].clone();
            for i in 0..candidato.pesos.len() {
                candidato.pesos[i] =
                    poblacion[a].pesos[i] + 0.58 * (poblacion[b].pesos[i] - poblacion[c].pesos[i]);
            }
            candidato.umbral_min_spread_bps = poblacion[a].umbral_min_spread_bps
                + 0.58 * (poblacion[b].umbral_min_spread_bps - poblacion[c].umbral_min_spread_bps);
            candidato.max_operacion_btc = poblacion[a].max_operacion_btc
                + 0.58 * (poblacion[b].max_operacion_btc - poblacion[c].max_operacion_btc);
            candidato.tolerancia_latencia_ms = (poblacion[a].tolerancia_latencia_ms as f64
                + 0.58
                    * (poblacion[b].tolerancia_latencia_ms - poblacion[c].tolerancia_latencia_ms)
                        as f64) as i64;
            candidato.normalizar();
            candidato.fitness = fitness_genoma(&candidato, base);
            if candidato.fitness > poblacion[destino].fitness {
                poblacion[destino] = candidato;
            }
        }
    }

    fn mutar_fino(&mut self, genoma: &mut Genoma, temperatura: f64) {
        let escala = temperatura.clamp(0.05, 1.25);
        for peso in &mut genoma.pesos {
            *peso += gauss(&mut self.rng, 0.0, 0.025 * escala);
        }
        genoma.umbral_min_spread_bps += gauss(&mut self.rng, 0.0, 0.16 * escala);
        genoma.max_operacion_btc += gauss(&mut self.rng, 0.0, 0.03 * escala);
        genoma.tolerancia_latencia_ms += gauss(&mut self.rng, 0.0, 260.0 * escala) as i64;
        genoma.normalizar();
    }
}

#[derive(Clone, Debug)]
struct VentanaFitness {
    pnl_total: f64,
    utilidad_media: f64,
    sharpe: f64,
    win_rate: f64,
    max_drawdown: f64,
    latencia_media: f64,
    fill_parcial_rate: f64,
    n: usize,
    fallos: usize,
}

fn evaluar_ventana(operaciones: &[Operacion], fallos: usize) -> VentanaFitness {
    let n = operaciones.len().max(1);
    let pnl_total = operaciones.iter().map(|op| op.utilidad_usd).sum::<f64>();
    let utilidad_media = pnl_total / n as f64;
    let win_rate = operaciones
        .iter()
        .filter(|op| op.utilidad_usd > 0.0)
        .count() as f64
        / n as f64;
    let latencia_media = operaciones
        .iter()
        .map(|op| op.latencia_max_ms as f64)
        .sum::<f64>()
        / n as f64;
    let fill_parcial_rate = operaciones.iter().filter(|op| op.parcial).count() as f64 / n as f64;
    let retornos = operaciones
        .iter()
        .filter_map(|op| {
            let capital = op.precio_compra * op.cantidad_btc;
            (capital > 0.0).then_some(op.utilidad_usd / capital)
        })
        .collect::<Vec<_>>();
    let sharpe = if retornos.len() < 2 {
        0.0
    } else {
        let media = retornos.iter().sum::<f64>() / retornos.len() as f64;
        let var =
            retornos.iter().map(|r| (r - media).powi(2)).sum::<f64>() / (retornos.len() - 1) as f64;
        let desv = var.sqrt();
        if desv == 0.0 {
            0.0
        } else {
            (media / desv).clamp(-5.0, 5.0)
        }
    };
    let mut pico = 0.0;
    let mut acumulado = 0.0;
    let mut max_drawdown = 0.0;
    for op in operaciones.iter().rev() {
        acumulado += op.utilidad_usd;
        if acumulado > pico {
            pico = acumulado;
        }
        let dd = pico - acumulado;
        if dd > max_drawdown {
            max_drawdown = dd;
        }
    }
    VentanaFitness {
        pnl_total,
        utilidad_media,
        sharpe,
        win_rate,
        max_drawdown,
        latencia_media,
        fill_parcial_rate,
        n,
        fallos,
    }
}

fn fitness_genoma(g: &Genoma, base: &VentanaFitness) -> f64 {
    let pesos_balanceados = 1.0 - desviacion_pesos(&g.pesos);
    let ajuste_umbral = 1.0 - ((g.umbral_min_spread_bps - 0.9).abs() / 8.0).clamp(0.0, 0.9);
    let ajuste_tamano = 1.0 - ((g.max_operacion_btc - 0.18).abs() / 1.25).clamp(0.0, 0.7);
    let penalizacion_latencia = if g.tolerancia_latencia_ms as f64 >= base.latencia_media {
        0.0
    } else {
        (base.latencia_media - g.tolerancia_latencia_ms as f64) / 500.0
    };
    let penalizacion_fallos = base.fallos as f64 / (base.n + base.fallos).max(1) as f64;
    let penalizacion_parciales = base.fill_parcial_rate * 5.0;

    (base.utilidad_media.tanh() * 28.0)
        + base.sharpe * 8.0
        + base.win_rate * 22.0
        + (base.pnl_total / 500.0).tanh() * 18.0
        + pesos_balanceados * 8.0
        + ajuste_umbral * 8.0
        + ajuste_tamano * 5.0
        - (base.max_drawdown / 250.0).min(30.0)
        - penalizacion_fallos * 40.0
        - penalizacion_latencia.min(20.0)
        - penalizacion_parciales
}

fn desviacion_pesos(pesos: &[f64; 5]) -> f64 {
    let ideal = [0.42, 0.18, 0.18, 0.12, 0.10];
    pesos
        .iter()
        .zip(ideal)
        .map(|(a, b)| (a - b).abs())
        .sum::<f64>()
        .clamp(0.0, 1.0)
}

fn diversidad(poblacion: &[Genoma]) -> f64 {
    if poblacion.len() < 2 {
        return 0.0;
    }
    let media = (0..5)
        .map(|i| poblacion.iter().map(|g| g.pesos[i]).sum::<f64>() / poblacion.len() as f64)
        .collect::<Vec<_>>();
    let var = poblacion
        .iter()
        .flat_map(|g| {
            (0..5)
                .map(|i| (g.pesos[i] - media[i]).powi(2))
                .collect::<Vec<_>>()
        })
        .sum::<f64>()
        / (poblacion.len() * 5) as f64;
    (var.sqrt() * 5.0).clamp(0.0, 1.0)
}

fn gauss(rng: &mut StdRng, media: f64, sigma: f64) -> f64 {
    let u1 = rng.gen_range(f64::EPSILON..1.0);
    let u2 = rng.gen_range(0.0..1.0);
    let z0 = (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos();
    media + z0 * sigma
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CostosOperacion;

    fn op(utilidad: f64, parcial: bool) -> Operacion {
        Operacion {
            id: format!("{utilidad}"),
            compra_en: "A".into(),
            venta_en: "B".into(),
            par: "BTC/USDT".into(),
            cantidad_btc: 0.1,
            precio_compra: 100_000.0,
            precio_venta: 100_100.0,
            utilidad_usd: utilidad,
            costos: CostosOperacion::default(),
            parcial,
            ejecutada_en: Utc::now(),
            latencia_max_ms: 120,
        }
    }

    #[test]
    fn ga_evoluciona_poblacion_y_publica_estrategia() {
        let mut ga = EstadoGa::default();
        let operaciones = vec![
            op(18.0, false),
            op(9.0, false),
            op(-4.0, true),
            op(14.0, false),
        ];
        ga.evolucionar(&operaciones, 1);
        let estado = ga.public().unwrap();
        assert_eq!(estado.generacion, 1);
        assert_eq!(estado.operaciones_evaluadas, 4);
        assert_eq!(estado.fallos_evaluados, 1);
        assert_eq!(estado.mejores_pesos.len(), 5);
        assert!(estado.umbral_optimizado >= 0.1);
    }
}
