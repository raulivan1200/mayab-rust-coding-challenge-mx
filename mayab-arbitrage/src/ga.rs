//! Algoritmo genético para ajustar la estrategia de selección de oportunidades.
//!
//! La población evoluciona pesos de scoring, umbral mínimo, tamaño máximo de
//! operación y tolerancia de latencia usando historial simulado o replay
//! sintético controlado.

use std::collections::HashMap;

use chrono::Utc;
use rand::{rngs::StdRng, Rng, SeedableRng};
use serde::{Deserialize, Serialize};

use crate::types::{EstadoGenetico, MoneyUnits, Operacion, PuntoPareto, QtyUnits};

/// Score canónico compartido por ejecución, explicación y entrenamiento GA.
/// Mantener esta transformación en un solo sitio evita que el campeón se
/// evalúe con features distintas de las que gobiernan el motor.
pub struct FeaturesScore {
    pub utilidad_usd: MoneyUnits,
    pub latencia_ms: i64,
    pub tolerancia_latencia_ms: i64,
    pub cantidad_btc: QtyUnits,
    pub max_operacion_btc: QtyUnits,
    pub confiabilidad: f64,
    pub z_score: f64,
}

pub fn score_canonico(pesos: &[f64], f: FeaturesScore) -> f64 {
    let mut w = [0.40, 0.20, 0.20, 0.10, 0.10];
    if pesos.len() >= 5 {
        let total = pesos.iter().take(5).copied().sum::<f64>();
        if total > 0.0 {
            for i in 0..5 {
                w[i] = pesos[i].max(0.0) / total;
            }
        }
    }
    let features = [
        (f.utilidad_usd / 100.0).clamp(0.0, 1.0),
        (1.0 - f.latencia_ms as f64 / f.tolerancia_latencia_ms.max(1) as f64).clamp(0.0, 1.0),
        (f.cantidad_btc / f.max_operacion_btc.max(0.00000001)).clamp(0.0, 1.0),
        f.confiabilidad.clamp(0.0, 1.0),
        (f.z_score / 3.0).clamp(0.0, 1.0),
    ];
    w.iter()
        .zip(features)
        .map(|(peso, valor)| peso * valor)
        .sum()
}

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
    pub max_operacion_btc: QtyUnits,
    pub tolerancia_latencia_ms: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Genoma {
    pesos: [f64; 5],
    umbral_min_spread_bps: f64,
    max_operacion_btc: QtyUnits,
    tolerancia_latencia_ms: i64,
    fitness: f64,
    objetivos: [f64; 4],
    rank: usize,
    crowding_distance: f64,
}

impl Genoma {
    fn base() -> Self {
        Self {
            pesos: [0.40, 0.20, 0.20, 0.10, 0.10],
            umbral_min_spread_bps: 0.65,
            max_operacion_btc: (0.18),
            tolerancia_latencia_ms: 4500,
            fitness: 0.0,
            objetivos: [0.0; 4],
            rank: 0,
            crowding_distance: 0.0,
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
            max_operacion_btc: (rng.gen_range(0.03..0.60)),
            tolerancia_latencia_ms: rng.gen_range(900..7000),
            fitness: 0.0,
            objetivos: [0.0; 4],
            rank: 0,
            crowding_distance: 0.0,
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

    fn domina(&self, otro: &Genoma) -> bool {
        let mut estricto = false;
        for i in 0..4 {
            if self.objetivos[i] < otro.objetivos[i] {
                return false;
            }
            if self.objetivos[i] > otro.objetivos[i] {
                estricto = true;
            }
        }
        estricto
    }
}

fn fast_non_dominated_sort(poblacion: &mut [Genoma]) -> Vec<Vec<usize>> {
    let n = poblacion.len();
    if n == 0 {
        return vec![];
    }
    let mut fronts: Vec<Vec<usize>> = vec![vec![]];
    let mut domination_count = vec![0; n];
    let mut dominated_list: Vec<Vec<usize>> = vec![vec![]; n];

    for p in 0..n {
        for q in 0..n {
            if p == q {
                continue;
            }
            if poblacion[p].domina(&poblacion[q]) {
                dominated_list[p].push(q);
            } else if poblacion[q].domina(&poblacion[p]) {
                domination_count[p] += 1;
            }
        }
        if domination_count[p] == 0 {
            poblacion[p].rank = 0;
            fronts[0].push(p);
        }
    }

    let mut i = 0;
    while !fronts[i].is_empty() {
        let mut next_front = vec![];
        for &p in &fronts[i] {
            for &q in &dominated_list[p] {
                domination_count[q] -= 1;
                if domination_count[q] == 0 {
                    poblacion[q].rank = i + 1;
                    next_front.push(q);
                }
            }
        }
        i += 1;
        if next_front.is_empty() {
            break;
        }
        fronts.push(next_front);
    }

    if fronts.last().map(|f| f.is_empty()).unwrap_or(false) {
        fronts.pop();
    }
    fronts
}

fn crowding_distance_assignment(poblacion: &mut [Genoma], front: &[usize]) {
    let l = front.len();
    if l == 0 {
        return;
    }
    for &idx in front {
        poblacion[idx].crowding_distance = 0.0;
    }
    for m in 0..4 {
        let mut front_sorted = front.to_vec();
        front_sorted.sort_by(|&a, &b| {
            poblacion[a].objetivos[m]
                .partial_cmp(&poblacion[b].objetivos[m])
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        poblacion[front_sorted[0]].crowding_distance = f64::INFINITY;
        poblacion[front_sorted[l - 1]].crowding_distance = f64::INFINITY;

        let obj_min = poblacion[front_sorted[0]].objetivos[m];
        let obj_max = poblacion[front_sorted[l - 1]].objetivos[m];
        let diff = (obj_max - obj_min).max(1e-9);

        for i in 1..(l.saturating_sub(1)) {
            let prev = front_sorted[i - 1];
            let next = front_sorted[i + 1];
            let current = front_sorted[i];
            if poblacion[current].crowding_distance != f64::INFINITY {
                poblacion[current].crowding_distance +=
                    (poblacion[next].objetivos[m] - poblacion[prev].objetivos[m]) / diff;
            }
        }
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
    pub max_operacion_optimizada_btc: QtyUnits,
    pub tolerancia_latencia_ms: i64,
    pub operaciones_evaluadas: usize,
    pub fallos_evaluados: usize,
    pub mejora_generacional: f64,
    pub temperatura_annealing: f64,
    pub inyecciones_diferenciales: usize,
    islas: [Vec<Genoma>; 4],
    config_islas: [ConfigGa; 4],
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

        // Inicializar 4 islas
        let mut islas: [Vec<Genoma>; 4] = core::array::from_fn(|_| Vec::new());
        for (i, genoma) in poblacion.iter().enumerate() {
            islas[i % 4].push(genoma.clone());
        }

        // Configuraciones base (Meta-GA mutará esto)
        let config_islas = [
            ConfigGa {
                tasa_mutacion: 0.1,
                tasa_cruce: 0.8,
                ..config
            }, // Nicho 0: Tendencia
            ConfigGa {
                tasa_mutacion: 0.2,
                tasa_cruce: 0.7,
                ..config
            }, // Nicho 1: Rango
            ConfigGa {
                tasa_mutacion: 0.3,
                tasa_cruce: 0.6,
                ..config
            }, // Nicho 2: Volátil
            ConfigGa {
                tasa_mutacion: 0.05,
                tasa_cruce: 0.9,
                ..config
            }, // Nicho 3: Calmo
        ];
        Self {
            config,
            generacion: 0,
            mejor_fitness: 0.0,
            fitness_promedio: 0.0,
            retador_fitness: 0.0,
            diversidad: 1.0,
            mejores_pesos: [0.40, 0.20, 0.20, 0.10, 0.10],
            umbral_optimizado: 0.65,
            max_operacion_optimizada_btc: (0.18),
            tolerancia_latencia_ms: 4500,
            operaciones_evaluadas: 0,
            fallos_evaluados: 0,
            mejora_generacional: 0.0,
            temperatura_annealing: 1.0,
            inyecciones_diferenciales: 0,
            islas,
            config_islas,
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

        for i in 0..4 {
            for genoma in &mut self.islas[i] {
                let ventana = evaluar_genoma(genoma, operaciones, fallos);
                // Nichos: Alteramos el fitness escalar según el nicho
                let mut fitness_modificado = fitness_genoma(genoma, &ventana);
                match i {
                    0 => fitness_modificado += ventana.pnl_total * 0.5, // Tendencia (busca utilidad)
                    1 => fitness_modificado += ventana.sharpe * 10.0,   // Rango (busca sharpe)
                    2 => fitness_modificado += ventana.win_rate * 50.0, // Volátil (busca win rate)
                    3 => fitness_modificado -= ventana.max_drawdown * 0.1, // Calmo (minimiza riesgo)
                    _ => {}
                }
                genoma.fitness = fitness_modificado;
                genoma.objetivos[0] = ventana.pnl_total;
                genoma.objetivos[1] = ventana.sharpe;
                genoma.objetivos[2] = -ventana.max_drawdown;
                genoma.objetivos[3] = ventana.win_rate;
            }
            let fronts = fast_non_dominated_sort(&mut self.islas[i]);
            for front in &fronts {
                crowding_distance_assignment(&mut self.islas[i], front);
            }
            self.islas[i].sort_by(|a, b| {
                if a.rank != b.rank {
                    a.rank.cmp(&b.rank)
                } else {
                    b.crowding_distance
                        .partial_cmp(&a.crowding_distance)
                        .unwrap_or(std::cmp::Ordering::Equal)
                }
            });
        }

        // Reconstruir la población aplanada (para mantener compatibilidad con otras funciones)
        self.poblacion.clear();
        for isla in &self.islas {
            self.poblacion.extend(isla.iter().cloned());
        }

        let fronts = fast_non_dominated_sort(&mut self.poblacion);
        for front in &fronts {
            crowding_distance_assignment(&mut self.poblacion, front);
        }
        self.poblacion.sort_by(|a, b| {
            if a.rank != b.rank {
                a.rank.cmp(&b.rank)
            } else {
                b.crowding_distance
                    .partial_cmp(&a.crowding_distance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }
        });

        let fitness_anterior = self.mejor_fitness;
        // Política operativa explícita: dentro del primer frente no dominado,
        // el campeón es la mayor utilidad escalar ajustada por riesgo. Pareto
        // conserva alternativas; crowding sólo preserva diversidad.
        let mejor = self
            .poblacion
            .iter()
            .filter(|g| g.rank == 0)
            .max_by(|a, b| a.fitness.total_cmp(&b.fitness))
            .cloned()
            .unwrap_or_else(|| self.poblacion[0].clone());
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

        self.aplicar_recocido_simulado(operaciones, fallos, &mut siguiente, elite);
        self.inyectar_evolucion_diferencial(operaciones, fallos, &mut siguiente, elite);

        // Migración de islas y meta-GA
        if self.generacion > 0 && self.generacion % 5 == 0 {
            // Migrar élite entre islas (0->1, 1->2, 2->3, 3->0)
            for i in 0..4 {
                if !self.islas[i].is_empty() {
                    let emigrante = self.islas[i][0].clone();
                    // Meta-GA: mutar ligeramente las tasas de cruce y mutación
                    self.config_islas[i].tasa_mutacion = (self.config_islas[i].tasa_mutacion
                        + self.rng.gen_range(-0.02..0.02))
                    .clamp(0.01, 0.4);
                    self.config_islas[i].tasa_cruce = (self.config_islas[i].tasa_cruce
                        + self.rng.gen_range(-0.05..0.05))
                    .clamp(0.5, 0.95);
                    let dest = (i + 1) % 4;
                    if let Some(last) = self.islas[dest].last_mut() {
                        *last = emigrante;
                    }
                }
            }
        }

        if self.diversidad < 0.04 {
            for genoma in siguiente.iter_mut().skip(elite).step_by(3) {
                *genoma = Genoma::random(&mut self.rng);
            }
        }

        for genoma in &mut siguiente {
            let ventana = evaluar_genoma(genoma, operaciones, fallos);
            genoma.fitness = fitness_genoma(genoma, &ventana);
            genoma.objetivos[0] = ventana.pnl_total;
            genoma.objetivos[1] = ventana.sharpe;
            genoma.objetivos[2] = -ventana.max_drawdown;
            genoma.objetivos[3] = ventana.win_rate;
        }
        let fronts = fast_non_dominated_sort(&mut siguiente);
        for front in &fronts {
            crowding_distance_assignment(&mut siguiente, front);
        }
        siguiente.sort_by(|a, b| {
            if a.rank != b.rank {
                a.rank.cmp(&b.rank)
            } else {
                b.crowding_distance
                    .partial_cmp(&a.crowding_distance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }
        });
        let mejor_hibrido = siguiente
            .iter()
            .filter(|g| g.rank == 0)
            .max_by(|a, b| a.fitness.total_cmp(&b.fitness))
            .cloned()
            .unwrap_or_else(|| siguiente[0].clone());
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
            frontera_pareto: self.frontera_pareto(),
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
            "fronteraPareto": self.frontera_pareto(),
            "politicaCampeon": "max_fitness_ajustado_riesgo_en_primer_frente",
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

    fn frontera_pareto(&self) -> Vec<PuntoPareto> {
        if self.generacion == 0 || self.operaciones_evaluadas == 0 {
            return vec![];
        }
        self.poblacion
            .iter()
            .filter(|g| g.rank == 0)
            .map(|g| PuntoPareto {
                x: g.objetivos[1],
                y: g.objetivos[0],
                umbral: g.umbral_min_spread_bps,
            })
            .collect()
    }

    /// Compara reglas operativas fijas sobre las mismas operaciones.
    ///
    /// No es una ablación de operadores evolutivos: no vuelve a entrenar el
    /// GA ni atribuye causalidad a recocido o evolución diferencial.
    pub fn sensibilidad_reglas(&self, operaciones: &[Operacion]) -> serde_json::Value {
        let calcular_metricas = |g: &Genoma| -> serde_json::Value {
            let mut filtradas = Vec::new();
            for op in operaciones {
                let capital = op.precio_compra * op.cantidad_btc;
                if capital <= 0.0 || op.cantidad_btc <= 0.0 {
                    continue;
                }
                let neto_bps = utilidad_esperada(op) / capital * 10_000.0;
                if neto_bps < g.umbral_min_spread_bps
                    || op.latencia_max_ms > g.tolerancia_latencia_ms
                {
                    continue;
                }
                let cantidad = op.cantidad_btc.min(g.max_operacion_btc);
                if cantidad <= 0.0 {
                    continue;
                }
                let escala = cantidad / op.cantidad_btc;
                filtradas.push(op.utilidad_usd * escala);
            }

            let trades = filtradas.len();
            if trades == 0 {
                return serde_json::json!({
                    "mediana": 0.0, "p05": 0.0, "p95": 0.0, "drawdown": 0.0, "trades": 0, "profit_factor": 0.0
                });
            }

            let mut profit = 0.0;
            let mut loss = 0.0;
            let mut acumulado = 0.0;
            let mut pico = 0.0;
            let mut max_drawdown = 0.0;

            let mut ordenadas = filtradas.clone();
            ordenadas.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

            let mut ganadoras = 0;
            for &u in &filtradas {
                if u > 0.0 {
                    profit += u;
                    ganadoras += 1;
                } else {
                    loss -= u;
                }
                acumulado += u;
                if acumulado > pico {
                    pico = acumulado;
                }
                let dd = pico - acumulado;
                if dd > max_drawdown {
                    max_drawdown = dd;
                }
            }

            let profit_factor = if loss == 0.0 {
                profit.max(1.0) // fallback
            } else {
                profit / loss
            };

            let p = |pct: f64| -> f64 {
                let idx = ((trades as f64 - 1.0) * pct).round() as usize;
                ordenadas[idx]
            };

            serde_json::json!({
                "mediana": p(0.50),
                "p05": p(0.05),
                "p95": p(0.95),
                "drawdown": max_drawdown,
                "trades": trades,
                "profit_factor": profit_factor,
                "win_rate": if trades > 0 { ganadoras as f64 / trades as f64 } else { 0.0 }
            })
        };

        let base = if !self.poblacion.is_empty() {
            self.poblacion[0].clone()
        } else {
            Genoma::base()
        };

        let mut g_spread = base.clone();
        g_spread.pesos = [0.0; 5];
        g_spread.umbral_min_spread_bps = 5.0; // Alto para no filtrar ruido si no hay GA

        let mut g_conservador = base.clone();
        g_conservador.umbral_min_spread_bps = 15.0;
        g_conservador.max_operacion_btc = 0.05;

        let mut g_ev_fijo = base.clone();
        g_ev_fijo.pesos = [1.0, 1.0, 1.0, 1.0, 1.0];

        serde_json::json!({
            "tipoAnalisis": "sensibilidad_reglas",
            "esAblacionOperadores": false,
            "solo_spread": calcular_metricas(&g_spread),
            "conservador": calcular_metricas(&g_conservador),
            "ev_fijo": calcular_metricas(&g_ev_fijo),
            "configuracion_activa": calcular_metricas(&base)
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

    fn torneo(&mut self, k: usize) -> Genoma {
        let mut mejor = &self.poblacion[self.rng.gen_range(0..self.poblacion.len())];
        for _ in 1..k {
            let retador = &self.poblacion[self.rng.gen_range(0..self.poblacion.len())];
            let gana = if retador.rank != mejor.rank {
                retador.rank < mejor.rank
            } else {
                retador.crowding_distance > mejor.crowding_distance
            };
            if gana {
                mejor = retador;
            }
        }
        mejor.clone()
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
        operaciones: &[Operacion],
        fallos: usize,
        poblacion: &mut [Genoma],
        elite: usize,
    ) {
        if poblacion.is_empty() {
            return;
        }
        self.temperatura_annealing = (1.25 / (1.0 + self.generacion as f64 * 0.045)).max(0.05);
        let limite = poblacion.len().min((elite + 8).max(8));
        for genoma in poblacion.iter_mut().take(limite).skip(1) {
            let ventana = evaluar_genoma(genoma, operaciones, fallos);
            genoma.fitness = fitness_genoma(genoma, &ventana);
            let mut vecino = genoma.clone();
            self.mutar_fino(&mut vecino, self.temperatura_annealing);
            let ventana_vecino = evaluar_genoma(&vecino, operaciones, fallos);
            vecino.fitness = fitness_genoma(&vecino, &ventana_vecino);
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
        operaciones: &[Operacion],
        fallos: usize,
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
            candidato.normalizar();
            let ventana = evaluar_genoma(&candidato, operaciones, fallos);
            candidato.fitness = fitness_genoma(&candidato, &ventana);
            candidato.objetivos[0] = ventana.pnl_total;
            candidato.objetivos[1] = ventana.sharpe;
            candidato.objetivos[2] = -ventana.max_drawdown;
            candidato.objetivos[3] = ventana.win_rate;
            if candidato.domina(&poblacion[destino])
                || (!poblacion[destino].domina(&candidato)
                    && candidato.fitness > poblacion[destino].fitness)
            {
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
    calibracion_pesos: f64,
    edge_ponderado: f64,
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
        calibracion_pesos: 0.0,
        edge_ponderado: 0.0,
        n,
        fallos,
    }
}

fn evaluar_genoma(g: &Genoma, operaciones: &[Operacion], fallos: usize) -> VentanaFitness {
    let seleccionadas = operaciones
        .iter()
        .filter_map(|op| {
            let capital = op.precio_compra * op.cantidad_btc;
            if capital <= 0.0 || op.cantidad_btc <= 0.0 {
                return None;
            }
            let neto_bps = utilidad_esperada(op) / capital * 10_000.0;
            if neto_bps < g.umbral_min_spread_bps || op.latencia_max_ms > g.tolerancia_latencia_ms {
                return None;
            }
            let cantidad = op.cantidad_btc.min(g.max_operacion_btc);
            if cantidad <= 0.0 {
                return None;
            }
            let escala = cantidad / op.cantidad_btc;
            let mut candidato = op.clone();
            candidato.cantidad_btc = cantidad;
            candidato.utilidad_usd *= escala;
            candidato.utilidad_esperada_usd *= escala;
            candidato.costos.fee_compra_usd *= escala;
            candidato.costos.fee_venta_usd *= escala;
            candidato.costos.deslizamiento_usd *= escala;
            candidato.costos.retiro_amort_usd *= escala;
            candidato.costos.latencia_riesgo_usd *= escala;
            candidato.costos.seleccion_adversa_usd *= escala;
            candidato.costos.total_usd *= escala;
            Some(candidato)
        })
        .collect::<Vec<_>>();
    let mut ventana = evaluar_ventana(&seleccionadas, fallos);
    (ventana.calibracion_pesos, ventana.edge_ponderado) = evaluar_pesos(g, &seleccionadas);
    ventana
}

fn fitness_genoma(g: &Genoma, base: &VentanaFitness) -> f64 {
    let penalizacion_latencia = if g.tolerancia_latencia_ms as f64 >= base.latencia_media {
        0.0
    } else {
        (base.latencia_media - g.tolerancia_latencia_ms as f64) / 500.0
    };
    let penalizacion_fallos = base.fallos as f64 / (base.n + base.fallos).max(1) as f64;
    let penalizacion_parciales = base.fill_parcial_rate * 5.0;

    (base.utilidad_media.max(0.0).sqrt() * 6.0).min(35.0)
        + base.sharpe * 8.0
        + base.win_rate * 22.0
        + (base.pnl_total.max(0.0).sqrt() * 0.8).min(40.0)
        + base.calibracion_pesos * 16.0
        + base.edge_ponderado * 14.0
        - (base.max_drawdown / 250.0).min(30.0)
        - penalizacion_fallos * 40.0
        - penalizacion_latencia.min(20.0)
        - penalizacion_parciales
}

fn utilidad_esperada(op: &Operacion) -> f64 {
    if op.utilidad_esperada_usd.abs() > 0.0 {
        op.utilidad_esperada_usd
    } else {
        let spread = (op.precio_venta - op.precio_compra) * op.cantidad_btc;
        let costos = op.costos.total_usd - op.costos.seleccion_adversa_usd;
        spread - costos.max(0.0)
    }
}

fn evaluar_pesos(g: &Genoma, operaciones: &[Operacion]) -> (f64, f64) {
    if operaciones.is_empty() {
        return (0.0, 0.0);
    }
    let mut rutas: HashMap<String, (usize, usize)> = HashMap::new();
    let mut netos_bps = Vec::with_capacity(operaciones.len());
    let max_cantidad = operaciones
        .iter()
        .map(|op| op.cantidad_btc)
        .fold(0.0_f64, f64::max)
        .max(0.00000001);
    for op in operaciones {
        let ruta = format!("{}->{}", op.compra_en, op.venta_en);
        let entry = rutas.entry(ruta).or_insert((0, 0));
        entry.0 += usize::from(op.utilidad_usd > 0.0);
        entry.1 += 1;
        let capital = op.precio_compra * op.cantidad_btc;
        netos_bps.push(utilidad_esperada(op) / capital.max(0.00000001) * 10_000.0);
    }
    let media = netos_bps.iter().sum::<f64>() / netos_bps.len() as f64;
    let desv = if netos_bps.len() > 1 {
        (netos_bps
            .iter()
            .map(|valor| (valor - media).powi(2))
            .sum::<f64>()
            / (netos_bps.len() - 1) as f64)
            .sqrt()
            .max(0.00000001)
    } else {
        1.0
    };

    let mut calibracion = 0.0;
    let mut edge = 0.0;
    for (op, neto_bps) in operaciones.iter().zip(netos_bps) {
        let ruta = format!("{}->{}", op.compra_en, op.venta_en);
        let (wins, total) = rutas.get(&ruta).copied().unwrap_or((0, 1));
        let z_score = (neto_bps - media) / desv;
        let prediccion = score_canonico(
            &g.pesos,
            FeaturesScore {
                utilidad_usd: (utilidad_esperada(op)),
                latencia_ms: op.latencia_max_ms,
                tolerancia_latencia_ms: g.tolerancia_latencia_ms,
                cantidad_btc: op.cantidad_btc,
                max_operacion_btc: (max_cantidad),
                confiabilidad: wins as f64 / total.max(1) as f64,
                z_score,
            },
        )
        .clamp(0.0, 1.0);
        let outcome = if op.utilidad_usd > 0.0 { 1.0 } else { 0.0 };
        calibracion += 1.0 - (prediccion - outcome).powi(2);
        edge += (prediccion - 0.5) * (op.utilidad_usd / 25.0).tanh();
    }
    let n = operaciones.len() as f64;
    (
        (calibracion / n).clamp(0.0, 1.0),
        (0.5 + edge / n).clamp(0.0, 1.0),
    )
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
            piernas: vec![],
            tipo: crate::types::TipoOportunidad::Lineal,
            id: format!("{utilidad}"),
            compra_en: "A".into(),
            venta_en: "B".into(),
            par: "BTC/USDT".into(),
            cantidad_btc: 0.1,
            precio_compra: 100_000.0,
            precio_venta: 100_100.0,
            utilidad_usd: utilidad,
            utilidad_esperada_usd: utilidad,
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
        assert!(!estado.frontera_pareto.is_empty());
        assert_eq!(
            ga.api_estado()["politicaCampeon"],
            "max_fitness_ajustado_riesgo_en_primer_frente"
        );
    }

    #[test]
    fn fitness_evalua_limites_propios_de_cada_genoma() {
        let operaciones = vec![op(20.0, false), op(8.0, false)];
        let amplio = Genoma::base();
        let mut estricto = Genoma::base();
        estricto.umbral_min_spread_bps = 50.0;
        let mut pequeno = Genoma::base();
        pequeno.max_operacion_btc = 0.05;

        let ventana_amplia = evaluar_genoma(&amplio, &operaciones, 0);
        let ventana_estricta = evaluar_genoma(&estricto, &operaciones, 0);
        let ventana_pequena = evaluar_genoma(&pequeno, &operaciones, 0);

        assert_eq!(ventana_amplia.n, 2);
        assert_eq!(ventana_estricta.pnl_total, 0.0);
        assert!(ventana_pequena.pnl_total < ventana_amplia.pnl_total);
    }

    #[test]
    fn fitness_de_pesos_depende_de_outcomes_y_no_de_vector_ideal_fijo() {
        let mut ganadora = op(22.0, false);
        ganadora.utilidad_esperada_usd = 22.0;
        ganadora.latencia_max_ms = 4_000;
        let mut adversa = op(-12.0, false);
        adversa.utilidad_esperada_usd = 1.0;
        adversa.latencia_max_ms = 10;
        let operaciones = vec![ganadora, adversa];

        let mut utilidad = Genoma::base();
        utilidad.pesos = [0.96, 0.01, 0.01, 0.01, 0.01];
        utilidad.normalizar();
        let mut frescura = Genoma::base();
        frescura.pesos = [0.01, 0.96, 0.01, 0.01, 0.01];
        frescura.normalizar();

        let ventana_utilidad = evaluar_genoma(&utilidad, &operaciones, 0);
        let ventana_frescura = evaluar_genoma(&frescura, &operaciones, 0);

        assert!(ventana_utilidad.calibracion_pesos > ventana_frescura.calibracion_pesos);
        assert!(
            fitness_genoma(&utilidad, &ventana_utilidad)
                > fitness_genoma(&frescura, &ventana_frescura)
        );
    }
}
