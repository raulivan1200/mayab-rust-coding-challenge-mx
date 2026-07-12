//! Evaluación walk-forward de una cinta de mercado.
//!
//! La cinta se materializa antes de evaluar estrategias: cada método recibe los
//! mismos eventos, costos observados, liquidez y realizaciones. A entrena el GA,
//! B calibra parámetros de ejecución y C sólo se lee después de congelarlos.

use std::{
    collections::HashMap,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use rand::{rngs::StdRng, Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    config::Config,
    ga::EstadoGa,
    motor::calcular_costos_canonicos,
    types::{CostosOperacion, Cotizacion, MapaCostos, Operacion},
};

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Split {
    pub train: u32,
    pub calibration: u32,
    pub holdout: u32,
}
impl Default for Split {
    fn default() -> Self {
        Self {
            train: 50,
            calibration: 20,
            holdout: 30,
        }
    }
}
impl FromStr for Split {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        let p = s
            .split(',')
            .map(str::trim)
            .map(str::parse::<u32>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if p.len() != 3 || p.iter().sum::<u32>() != 100 || p.contains(&0) {
            bail!("--split debe contener tres porcentajes positivos que sumen 100");
        }
        Ok(Self {
            train: p[0],
            calibration: p[1],
            holdout: p[2],
        })
    }
}

pub struct EvaluationConfig {
    pub tape: PathBuf,
    pub output: PathBuf,
    pub split: Split,
    pub seed: u64,
}
pub struct OutputPaths {
    pub json: PathBuf,
    pub csv: PathBuf,
    pub markdown: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TapeEvent {
    timestamp_ms: i64,
    buy_exchange: String,
    sell_exchange: String,
    ask: f64,
    bid: f64,
    available_btc: f64,
    cost_quantity_btc: f64,
    latency_ms: i64,
    gross_bps: f64,
    base_cost_bps: f64,
    costs: CostosOperacion,
    realized_move_bps: f64,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum Sizing {
    Available,
    Fixed,
    Kelly,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Strategy {
    name: String,
    threshold_bps: f64,
    max_btc: f64,
    latency_ms: i64,
    impact_multiplier: f64,
    score_threshold: f64,
    weights: [f64; 5],
    sizing: Sizing,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    pnl_net_usd: f64,
    pnl_per_btc_usd: f64,
    pnl_per_deployed_capital: f64,
    max_drawdown_usd: f64,
    fill_rate_quantity: f64,
    fill_rate_orders: f64,
    profit_factor: f64,
    max_exposure_usd: f64,
    unwind_rate: f64,
    rejections_by_cause: HashMap<String, u64>,
    turnover_usd: f64,
    total_costs_usd: f64,
    stability_between_windows: f64,
    orders: u64,
    filled_orders: u64,
    pnl_windows: Vec<f64>,
    negative_windows: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StrategyReport {
    strategy: Strategy,
    calibration_score: f64,
    holdout: Metrics,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    schema_version: u32,
    generated_at: DateTime<Utc>,
    seed: u64,
    source: String,
    split: Split,
    event_counts: [usize; 3],
    partition_hashes: [String; 3],
    config_hash: String,
    engine_version: String,
    protocol: Protocol,
    ga_training: GaTraining,
    calibration: Calibration,
    results: Vec<StrategyReport>,
    holdout_winner: String,
    caveats: Vec<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Protocol {
    ga_sees_only_a: bool,
    calibration_uses_only_b: bool,
    holdout_runs_once: bool,
    common_events_and_costs: bool,
    holdout_seed_used_for_selection: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GaTraining {
    generations: usize,
    observations: usize,
    frozen_weights: [f64; 5],
    raw_threshold_bps: f64,
    raw_max_btc: f64,
    raw_latency_ms: i64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Calibration {
    observations: usize,
    objective: String,
    selected_before_holdout: bool,
}

pub fn evaluate_tape(cfg: &EvaluationConfig) -> Result<OutputPaths> {
    let quotes = load_quotes(&cfg.tape)?;
    let costs = Config::from_env().costos;
    let events = materialize_events(&quotes, &costs);
    if events.len() < 10 {
        bail!(
            "la cinta produjo {} eventos comparables; se requieren al menos 10",
            events.len()
        );
    }
    let n_a = events.len() * cfg.split.train as usize / 100;
    let n_b = events.len() * cfg.split.calibration as usize / 100;
    let (a, rest) = events.split_at(n_a);
    let (b, c) = rest.split_at(n_b);
    if a.is_empty() || b.is_empty() || c.is_empty() {
        bail!("la partición dejó una ventana vacía");
    }

    let ga_ops = operations_for_ga(a);
    let mut ga = EstadoGa::default();
    for _ in 0..24 {
        ga.evolucionar(&ga_ops, 0);
    }
    let evolved = ga.estrategia();
    let mut strategies = base_strategies(evolved.pesos, cfg.seed);
    for strategy in &mut strategies {
        calibrate(strategy, b);
    }
    // Freeze point: no mutation of strategies is permitted below this line.
    let calibration_scores = strategies
        .iter()
        .map(|s| objective(&run(s, b)))
        .collect::<Vec<_>>();
    let results = strategies
        .into_iter()
        .zip(calibration_scores)
        .map(|(strategy, calibration_score)| StrategyReport {
            holdout: run(&strategy, c),
            strategy,
            calibration_score,
        })
        .collect::<Vec<_>>();
    let winner = results
        .iter()
        .max_by(|x, y| x.holdout.pnl_net_usd.total_cmp(&y.holdout.pnl_net_usd))
        .map(|x| x.strategy.name.clone())
        .unwrap_or_default();
    let report = Report {
        schema_version: 1,
        generated_at: Utc::now(),
        seed: cfg.seed,
        source: cfg.tape.display().to_string(),
        split: cfg.split,
        event_counts: [a.len(), b.len(), c.len()],
        partition_hashes: [hash_events(a), hash_events(b), hash_events(c)],
        config_hash: hash_config(&costs),
        engine_version: env!("CARGO_PKG_VERSION").into(),
        protocol: Protocol {
            ga_sees_only_a: true,
            calibration_uses_only_b: true,
            holdout_runs_once: true,
            common_events_and_costs: true,
            holdout_seed_used_for_selection: false,
        },
        ga_training: GaTraining {
            generations: 24,
            observations: ga_ops.len(),
            frozen_weights: evolved.pesos,
            raw_threshold_bps: evolved.umbral_min_spread_bps,
            raw_max_btc: evolved.max_operacion_btc,
            raw_latency_ms: evolved.tolerancia_latencia_ms,
        },
        calibration: Calibration {
            observations: b.len(),
            objective: "pnl_neto - 0.35*max_drawdown; desempate por profit_factor".into(),
            selected_before_holdout: true,
        },
        results,
        holdout_winner: winner,
        caveats: vec![
            "Evaluación simulada; no demuestra rentabilidad real.".into(),
            "Se conservan todas las estrategias, ventanas negativas y derrotas del campeón GA."
                .into(),
        ],
    };
    fs::create_dir_all(&cfg.output)
        .with_context(|| format!("no se pudo crear {}", cfg.output.display()))?;
    let paths = OutputPaths {
        json: cfg.output.join("evaluation.json"),
        csv: cfg.output.join("evaluation.csv"),
        markdown: cfg.output.join("evaluation.md"),
    };
    fs::write(&paths.json, serde_json::to_vec_pretty(&report)?)?;
    fs::write(&paths.csv, csv_report(&report))?;
    fs::write(&paths.markdown, markdown_report(&report))?;
    Ok(paths)
}

fn load_quotes(path: &Path) -> Result<Vec<Cotizacion>> {
    let mut files = Vec::new();
    if path.is_file() {
        files.push(path.to_path_buf());
    } else if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let p = entry?.path();
            if p.is_file()
                && matches!(
                    p.extension().and_then(|x| x.to_str()),
                    Some("json" | "jsonl" | "ndjson")
                )
            {
                files.push(p);
            }
        }
        files.sort();
    } else {
        bail!("no existe la cinta {}", path.display());
    }
    let mut out: Vec<Cotizacion> = Vec::new();
    for file in files {
        let raw = fs::read_to_string(&file)?;
        if matches!(
            file.extension().and_then(|x| x.to_str()),
            Some("jsonl" | "ndjson")
        ) {
            for (i, line) in raw
                .lines()
                .enumerate()
                .filter(|(_, l)| !l.trim().is_empty())
            {
                out.push(
                    serde_json::from_str(line)
                        .with_context(|| format!("{}:{}", file.display(), i + 1))?,
                );
            }
        } else {
            let value: Value =
                serde_json::from_str(&raw).with_context(|| file.display().to_string())?;
            let values = value
                .as_array()
                .cloned()
                .or_else(|| value.get("cotizaciones").and_then(Value::as_array).cloned())
                .or_else(|| value.get("eventos").and_then(Value::as_array).cloned())
                .ok_or_else(|| {
                    anyhow!("{} no contiene un array de cotizaciones", file.display())
                })?;
            for v in values {
                out.push(serde_json::from_value(v).with_context(|| file.display().to_string())?);
            }
        }
    }
    out.sort_by_key(|q| (q.evento_unix_ms, q.secuencia));
    Ok(out)
}

fn materialize_events(quotes: &[Cotizacion], config: &MapaCostos) -> Vec<TapeEvent> {
    let mut latest: HashMap<String, &Cotizacion> = HashMap::new();
    let mut out = Vec::new();
    for q in quotes {
        latest.insert(q.exchange.clone(), q);
        if latest.len() < 2 {
            continue;
        }
        let buy = latest
            .values()
            .min_by(|a, b| a.ask.total_cmp(&b.ask))
            .unwrap();
        let sell = latest
            .values()
            .filter(|x| x.exchange != buy.exchange)
            .max_by(|a, b| a.bid.total_cmp(&b.bid))
            .unwrap();
        let mid = (buy.ask + sell.bid) / 2.0;
        if mid <= 0.0 {
            continue;
        }
        let gross = (sell.bid - buy.ask) / mid * 10_000.0;
        let available_btc = buy.ask_cantidad.min(sell.bid_cantidad).max(0.0);
        let quantity = available_btc.min(config.max_operacion_btc);
        let costs = calcular_costos_canonicos(
            quantity,
            buy,
            sell,
            buy.latencia_ms.max(sell.latencia_ms),
            config,
        );
        let capital = buy.ask * quantity;
        let base_cost = if capital > 0.0 {
            costs.total_usd / capital * 10_000.0
        } else {
            0.0
        };
        let realized = ((q.bid + q.ask) / 2.0 - mid) / mid * 10_000.0;
        out.push(TapeEvent {
            timestamp_ms: q.evento_unix_ms,
            buy_exchange: buy.exchange.clone(),
            sell_exchange: sell.exchange.clone(),
            ask: buy.ask,
            bid: sell.bid,
            available_btc,
            cost_quantity_btc: quantity,
            latency_ms: buy.latencia_ms.max(sell.latencia_ms),
            gross_bps: gross,
            base_cost_bps: base_cost,
            costs,
            realized_move_bps: realized,
        });
    }
    out
}

fn operations_for_ga(events: &[TapeEvent]) -> Vec<Operacion> {
    events
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let qty = e.cost_quantity_btc;
            let capital = e.ask * qty;
            let expected = capital * (e.gross_bps - e.base_cost_bps) / 10_000.0;
            let realized = expected + capital * e.realized_move_bps / 10_000.0;
            Operacion {
                id: format!("tape-a-{i}"),
                tipo: Default::default(),
                compra_en: e.buy_exchange.clone(),
                venta_en: e.sell_exchange.clone(),
                par: "BTC/USD".into(),
                piernas: vec![],
                cantidad_btc: qty,
                precio_compra: e.ask,
                precio_venta: e.bid,
                utilidad_usd: realized,
                utilidad_esperada_usd: expected,
                costos: scale_costs(
                    &e.costs,
                    if e.cost_quantity_btc > 0.0 {
                        qty / e.cost_quantity_btc
                    } else {
                        0.0
                    },
                ),
                parcial: false,
                ejecutada_en: DateTime::from_timestamp_millis(e.timestamp_ms)
                    .unwrap_or_else(Utc::now),
                latencia_max_ms: e.latency_ms,
            }
        })
        .collect()
}

fn base_strategies(weights: [f64; 5], seed: u64) -> Vec<Strategy> {
    let mut rng = StdRng::seed_from_u64(seed);
    let random_w = [rng.gen(), rng.gen(), rng.gen(), rng.gen(), rng.gen()];
    let make = |name: &str, threshold, max, sizing, weights| Strategy {
        name: name.into(),
        threshold_bps: threshold,
        max_btc: max,
        latency_ms: 5000,
        impact_multiplier: 1.0,
        score_threshold: 0.0,
        weights,
        sizing,
    };
    vec![
        make(
            "spread_neto_simple",
            0.0,
            0.25,
            Sizing::Available,
            [1.0, 0.0, 0.0, 0.0, 0.0],
        ),
        make(
            "preset_conservador",
            2.0,
            0.08,
            Sizing::Available,
            [0.45, 0.25, 0.1, 0.1, 0.1],
        ),
        make(
            "preset_balanceado",
            0.65,
            0.18,
            Sizing::Available,
            [0.4, 0.2, 0.2, 0.1, 0.1],
        ),
        make(
            "tamano_fijo",
            0.65,
            0.10,
            Sizing::Fixed,
            [0.4, 0.2, 0.2, 0.1, 0.1],
        ),
        make(
            "kelly_fraccional",
            0.65,
            0.30,
            Sizing::Kelly,
            [0.4, 0.2, 0.2, 0.1, 0.1],
        ),
        make(
            "parametros_aleatorios",
            rng.gen_range(0.1..3.0),
            rng.gen_range(0.03..0.4),
            Sizing::Available,
            random_w,
        ),
        make(
            "campeon_ga_congelado",
            0.65,
            0.18,
            Sizing::Available,
            weights,
        ),
    ]
}

fn calibrate(s: &mut Strategy, b: &[TapeEvent]) {
    let mut best = (f64::NEG_INFINITY, s.clone());
    for threshold in [0.0, 0.5, 1.0, 2.0, 4.0] {
        for impact in [0.75, 1.0, 1.25, 1.5] {
            for score in [0.0, 0.25, 0.5] {
                let mut x = s.clone();
                x.threshold_bps = threshold;
                x.impact_multiplier = impact;
                x.score_threshold = score;
                let m = run(&x, b);
                let o = objective(&m);
                if o > best.0 || (o == best.0 && m.profit_factor > run(&best.1, b).profit_factor) {
                    best = (o, x);
                }
            }
        }
    }
    *s = best.1;
}
fn objective(m: &Metrics) -> f64 {
    m.pnl_net_usd - 0.35 * m.max_drawdown_usd
}

fn run(s: &Strategy, events: &[TapeEvent]) -> Metrics {
    let mut m = Metrics::default();
    let mut pnl: f64 = 0.0;
    let mut peak: f64 = 0.0;
    let mut gross_win = 0.;
    let mut gross_loss = 0.;
    let chunks = events.len().clamp(1, 8);
    let window = events.len().div_ceil(chunks);
    m.pnl_windows = vec![0.; chunks];
    for (i, e) in events.iter().enumerate() {
        m.orders += 1;
        let net = e.gross_bps - e.base_cost_bps * s.impact_multiplier;
        let score = score(s, e, net);
        if e.available_btc <= 0. {
            reject(&mut m, "sin_liquidez");
            continue;
        }
        if e.latency_ms > s.latency_ms {
            reject(&mut m, "latencia");
            continue;
        }
        if net < s.threshold_bps {
            reject(&mut m, "umbral_neto");
            continue;
        }
        if score < s.score_threshold {
            reject(&mut m, "score");
            continue;
        }
        let requested = s.max_btc;
        let qty = match s.sizing {
            Sizing::Available => requested.min(e.available_btc),
            Sizing::Fixed => requested.min(e.available_btc),
            Sizing::Kelly => (requested * ((net / 20.0).clamp(0.0, 0.5))).min(e.available_btc),
        };
        if qty <= 0.0 {
            reject(&mut m, "tamano_cero");
            continue;
        }
        m.filled_orders += 1;
        let capital = e.ask * qty;
        let cost = capital * e.base_cost_bps * s.impact_multiplier / 10_000.0;
        let trade = capital * (e.gross_bps + e.realized_move_bps) / 10_000.0 - cost;
        pnl += trade;
        peak = peak.max(pnl);
        m.max_drawdown_usd = m.max_drawdown_usd.max(peak - pnl);
        m.turnover_usd += 2.0 * capital;
        m.total_costs_usd += cost;
        m.max_exposure_usd = m.max_exposure_usd.max(capital);
        m.fill_rate_quantity += qty / requested.max(1e-9);
        m.pnl_per_btc_usd += qty;
        m.pnl_per_deployed_capital += capital;
        m.pnl_windows[(i / window).min(chunks - 1)] += trade;
        if trade >= 0.0 {
            gross_win += trade
        } else {
            gross_loss -= trade;
            m.unwind_rate += 1.0;
        }
    }
    m.pnl_net_usd = pnl;
    m.fill_rate_orders = m.filled_orders as f64 / m.orders.max(1) as f64;
    m.fill_rate_quantity /= m.orders.max(1) as f64;
    m.unwind_rate /= m.filled_orders.max(1) as f64;
    let btc = m.pnl_per_btc_usd;
    let capital = m.pnl_per_deployed_capital;
    m.pnl_per_btc_usd = pnl / btc.max(1e-9);
    m.pnl_per_deployed_capital = pnl / capital.max(1e-9);
    m.profit_factor = if gross_loss > 0. {
        gross_win / gross_loss
    } else if gross_win > 0. {
        f64::INFINITY
    } else {
        0.
    };
    m.negative_windows = m.pnl_windows.iter().filter(|x| **x < 0.).count() as u64;
    let mean = m.pnl_windows.iter().sum::<f64>() / chunks as f64;
    let sd = (m
        .pnl_windows
        .iter()
        .map(|x| (x - mean).powi(2))
        .sum::<f64>()
        / chunks as f64)
        .sqrt();
    m.stability_between_windows = if sd == 0. {
        if mean >= 0. {
            1.
        } else {
            0.
        }
    } else {
        (1. - sd / (mean.abs() + sd)).clamp(0., 1.)
    };
    m
}
fn score(s: &Strategy, e: &TapeEvent, net: f64) -> f64 {
    let sum = s.weights.iter().sum::<f64>().max(1e-9);
    let f = [
        (net / 10.0).clamp(-1.0, 1.0),
        (1.0 - e.latency_ms as f64 / 10_000.0).clamp(0.0, 1.0),
        (e.available_btc / 0.5).clamp(0.0, 1.0),
        if e.realized_move_bps >= 0.0 { 1.0 } else { 0.0 },
        (e.gross_bps.abs() / 20.0).clamp(0.0, 1.0),
    ];
    s.weights.iter().zip(f).map(|(w, x)| w * x).sum::<f64>() / sum
}
fn reject(m: &mut Metrics, cause: &str) {
    *m.rejections_by_cause.entry(cause.into()).or_default() += 1;
}

fn hash_events(events: &[TapeEvent]) -> String {
    let bytes = serde_json::to_vec(events).unwrap_or_default();
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{h:016x}")
}
fn hash_config(config: &MapaCostos) -> String {
    let mut value = serde_json::to_value(config).unwrap_or(Value::Null);
    canonicalize_json(&mut value);
    let bytes = serde_json::to_vec(&value).unwrap_or_default();
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// Ordena recursivamente las llaves para que la huella no dependa del orden
/// aleatorio de iteración de `HashMap` entre procesos.
fn canonicalize_json(value: &mut Value) {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<_> = std::mem::take(object).into_iter().collect();
            entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
            for (_, child) in &mut entries {
                canonicalize_json(child);
            }
            object.extend(entries);
        }
        Value::Array(values) => values.iter_mut().for_each(canonicalize_json),
        _ => {}
    }
}

fn scale_costs(costs: &CostosOperacion, factor: f64) -> CostosOperacion {
    CostosOperacion {
        fee_compra_usd: costs.fee_compra_usd * factor,
        fee_venta_usd: costs.fee_venta_usd * factor,
        deslizamiento_usd: costs.deslizamiento_usd * factor,
        retiro_amort_usd: costs.retiro_amort_usd * factor,
        latencia_riesgo_usd: costs.latencia_riesgo_usd * factor,
        seleccion_adversa_usd: costs.seleccion_adversa_usd * factor,
        total_usd: costs.total_usd * factor,
    }
}
fn finite(x: f64) -> String {
    if x.is_finite() {
        format!("{x:.8}")
    } else {
        "inf".into()
    }
}
fn csv_report(r: &Report) -> String {
    let mut s="strategy,pnl_net_usd,pnl_per_btc_usd,pnl_per_deployed_capital,max_drawdown_usd,fill_rate_quantity,fill_rate_orders,profit_factor,max_exposure_usd,unwind_rate,turnover_usd,total_costs_usd,stability_between_windows,negative_windows,rejections_by_cause\n".to_string();
    for x in &r.results {
        let rejects = serde_json::to_string(&x.holdout.rejections_by_cause)
            .unwrap()
            .replace('"', "\"\"");
        let m = &x.holdout;
        let _ = writeln!(
            s,
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},\"{}\"",
            x.strategy.name,
            finite(m.pnl_net_usd),
            finite(m.pnl_per_btc_usd),
            finite(m.pnl_per_deployed_capital),
            finite(m.max_drawdown_usd),
            finite(m.fill_rate_quantity),
            finite(m.fill_rate_orders),
            finite(m.profit_factor),
            finite(m.max_exposure_usd),
            finite(m.unwind_rate),
            finite(m.turnover_usd),
            finite(m.total_costs_usd),
            finite(m.stability_between_windows),
            m.negative_windows,
            rejects
        );
    }
    s
}
fn markdown_report(r: &Report) -> String {
    let mut s=format!("# Evaluación cronológica de tape\n\n- Split: **{}/{}/{}** (A/B/C)\n- Eventos: **{}/{}/{}**\n- Seed de optimización: **{}** (no se usa para seleccionar con C)\n- Ganador observado en holdout: **{}**\n\n| Estrategia | P&L neto | P&L/BTC | Max DD | Fill órdenes | Profit factor | Costos | Ventanas negativas |\n|---|---:|---:|---:|---:|---:|---:|---:|\n",r.split.train,r.split.calibration,r.split.holdout,r.event_counts[0],r.event_counts[1],r.event_counts[2],r.seed,r.holdout_winner);
    for x in &r.results {
        let m = &x.holdout;
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {:.2}% | {} | {} | {} |",
            x.strategy.name,
            finite(m.pnl_net_usd),
            finite(m.pnl_per_btc_usd),
            finite(m.max_drawdown_usd),
            m.fill_rate_orders * 100.,
            finite(m.profit_factor),
            finite(m.total_costs_usd),
            m.negative_windows
        );
    }
    s.push_str("\nTodas las derrotas y ventanas negativas se conservan. C se ejecutó una vez después de congelar GA y calibración.\n");
    s
}

#[cfg(test)]
mod tests {
    use crate::types::ExchangeConfig;

    use super::*;
    #[test]
    fn split_rejects_invalid() {
        assert!("50,20,20".parse::<Split>().is_err());
        assert!("50,20,30".parse::<Split>().is_ok());
    }
    #[test]
    fn negative_runs_are_preserved() {
        let s = base_strategies([0.4, 0.2, 0.2, 0.1, 0.1], 1).remove(0);
        let e = TapeEvent {
            timestamp_ms: 1,
            buy_exchange: "a".into(),
            sell_exchange: "b".into(),
            ask: 100.0,
            bid: 100.0,
            available_btc: 1.0,
            cost_quantity_btc: 1.0,
            latency_ms: 1,
            gross_bps: 30.0,
            base_cost_bps: 20.0,
            costs: CostosOperacion {
                total_usd: 0.2,
                ..Default::default()
            },
            realized_move_bps: -50.0,
        };
        let m = run(&s, &[e]);
        assert!(m.pnl_net_usd < 0.0);
        assert_eq!(m.negative_windows, 1);
    }

    #[test]
    fn config_hash_is_independent_from_exchange_insertion_order() {
        let mut first = MapaCostos::default();
        first.exchanges.insert(
            "Kraken".into(),
            ExchangeConfig {
                nombre: "Kraken".into(),
                fee_taker: 0.0026,
                retiro_btc: 0.0002,
                confiabilidad: 0.97,
            },
        );
        first.exchanges.insert(
            "Binance".into(),
            ExchangeConfig {
                nombre: "Binance".into(),
                fee_taker: 0.001,
                retiro_btc: 0.0001,
                confiabilidad: 0.98,
            },
        );

        let mut second = MapaCostos::default();
        for name in ["Binance", "Kraken"] {
            second.exchanges.insert(
                name.into(),
                first.exchanges.get(name).expect("exchange fixture").clone(),
            );
        }

        assert_eq!(hash_config(&first), hash_config(&second));
    }
}
