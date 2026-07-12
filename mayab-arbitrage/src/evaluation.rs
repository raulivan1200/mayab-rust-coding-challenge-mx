//! Evaluación walk-forward de una cinta de mercado.
//!
//! La cinta se materializa antes de evaluar estrategias: cada método recibe los
//! mismos eventos, costos observados, liquidez y realizaciones. A entrena el GA,
//! B calibra parámetros de ejecución y C sólo se lee después de congelarlos.

use std::{
    collections::{BTreeMap, HashMap},
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
    tape::{EventKind, TapeEvent as PublicTapeEvent, TapeSource, EVENTS_FILE, MANIFEST_FILE},
    types::{CostosOperacion, Cotizacion, MapaCostos, NivelOrden, Operacion},
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
    holdout_funnel: StrategyFunnel,
    holdout: Metrics,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct QuantitativeFunnel {
    raw_quotes: u64,
    valid_quotes: u64,
    comparable_candidates: u64,
    gross_dislocations: u64,
    net_dislocations: u64,
    liquid_net_dislocations: u64,
    invalid_quotes_by_cause: HashMap<String, u64>,
    gross_per_million_quotes: f64,
    net_per_million_quotes: f64,
    liquid_net_per_million_quotes: f64,
    definition: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StrategyFunnel {
    holdout_candidates: u64,
    gross_dislocations: u64,
    net_dislocations: u64,
    paper_orders_filled: u64,
    conversion_from_net: f64,
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
    quantitative_funnel: QuantitativeFunnel,
    partition_hashes: [String; 3],
    config_hash: String,
    engine_version: String,
    protocol: Protocol,
    ga_training: GaTraining,
    calibration: Calibration,
    results: Vec<StrategyReport>,
    preregistered_champion: String,
    selection_policy: String,
    champion_won_holdout: bool,
    ex_post_holdout_winner: String,
    #[serde(rename = "holdoutWinner", skip_serializing_if = "String::is_empty")]
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
    let quantitative_funnel = quantitative_funnel(&quotes, &events);
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
    let preregistered_champion = select_by_calibration(&strategies, &calibration_scores)
        .context("no se pudo seleccionar campeón con calibración B")?;
    let results = strategies
        .into_iter()
        .zip(calibration_scores)
        .map(|(strategy, calibration_score)| {
            let holdout = run(&strategy, c);
            let holdout_funnel = strategy_funnel(c, &holdout);
            StrategyReport {
                holdout,
                holdout_funnel,
                strategy,
                calibration_score,
            }
        })
        .collect::<Vec<_>>();
    let ex_post_winner = results
        .iter()
        .max_by(|x, y| x.holdout.pnl_net_usd.total_cmp(&y.holdout.pnl_net_usd))
        .map(|x| x.strategy.name.clone())
        .unwrap_or_default();
    let champion_won_holdout = preregistered_champion == ex_post_winner;
    let report = Report {
        schema_version: 1,
        generated_at: Utc::now(),
        seed: cfg.seed,
        source: cfg.tape.display().to_string(),
        split: cfg.split,
        event_counts: [a.len(), b.len(), c.len()],
        quantitative_funnel,
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
        preregistered_champion,
        selection_policy: "máximo objetivo en calibración B; congelado antes de ejecutar C".into(),
        champion_won_holdout,
        ex_post_holdout_winner: ex_post_winner.clone(),
        holdout_winner: ex_post_winner,
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

fn select_by_calibration(strategies: &[Strategy], scores: &[f64]) -> Option<String> {
    strategies
        .iter()
        .zip(scores)
        .filter(|(_, score)| score.is_finite())
        .max_by(
            |(left_strategy, left_score), (right_strategy, right_score)| {
                left_score
                    .total_cmp(right_score)
                    .then_with(|| right_strategy.name.cmp(&left_strategy.name))
            },
        )
        .map(|(strategy, _)| strategy.name.clone())
}

fn quantitative_funnel(quotes: &[Cotizacion], events: &[TapeEvent]) -> QuantitativeFunnel {
    let mut invalid_quotes_by_cause = HashMap::<String, u64>::new();
    let mut valid_quotes = 0_u64;
    for quote in quotes {
        let cause = if quote.exchange.trim().is_empty() || quote.par.trim().is_empty() {
            Some("identidad_vacia")
        } else if !quote.bid.is_finite() || !quote.ask.is_finite() {
            Some("precio_no_finito")
        } else if quote.bid <= 0.0 || quote.ask <= 0.0 || quote.bid >= quote.ask {
            Some("bbo_invalido")
        } else if !quote.bid_cantidad.is_finite()
            || !quote.ask_cantidad.is_finite()
            || quote.bid_cantidad < 0.0
            || quote.ask_cantidad < 0.0
        {
            Some("cantidad_invalida")
        } else {
            None
        };
        if let Some(cause) = cause {
            *invalid_quotes_by_cause.entry(cause.into()).or_default() += 1;
        } else {
            valid_quotes += 1;
        }
    }
    let gross_dislocations = events.iter().filter(|event| event.gross_bps > 0.0).count() as u64;
    let net_dislocations = events
        .iter()
        .filter(|event| event.gross_bps - event.base_cost_bps > 0.0)
        .count() as u64;
    let liquid_net_dislocations = events
        .iter()
        .filter(|event| event.gross_bps - event.base_cost_bps > 0.0 && event.available_btc > 0.0)
        .count() as u64;
    let raw_quotes = quotes.len() as u64;
    let per_million = |count: u64| count as f64 / raw_quotes.max(1) as f64 * 1_000_000.0;
    QuantitativeFunnel {
        raw_quotes,
        valid_quotes,
        comparable_candidates: events.len() as u64,
        gross_dislocations,
        net_dislocations,
        liquid_net_dislocations,
        invalid_quotes_by_cause,
        gross_per_million_quotes: per_million(gross_dislocations),
        net_per_million_quotes: per_million(net_dislocations),
        liquid_net_per_million_quotes: per_million(liquid_net_dislocations),
        definition: "por actualización se conserva el mejor candidato cross-venue; gross: bid>ask; net: gross supera costos canónicos; liquid_net: net y cantidad disponible positiva",
    }
}

fn strategy_funnel(events: &[TapeEvent], metrics: &Metrics) -> StrategyFunnel {
    let gross_dislocations = events.iter().filter(|event| event.gross_bps > 0.0).count() as u64;
    let net_dislocations = events
        .iter()
        .filter(|event| event.gross_bps - event.base_cost_bps > 0.0)
        .count() as u64;
    StrategyFunnel {
        holdout_candidates: events.len() as u64,
        gross_dislocations,
        net_dislocations,
        paper_orders_filled: metrics.filled_orders,
        conversion_from_net: metrics.filled_orders as f64 / net_dislocations.max(1) as f64,
    }
}

fn load_quotes(path: &Path) -> Result<Vec<Cotizacion>> {
    if path.is_dir() && path.join(MANIFEST_FILE).is_file() {
        crate::tape::verify(path).context("el tape público no supera verify-tape")?;
        return load_public_tape_quotes(&path.join(EVENTS_FILE));
    }
    if path.is_dir()
        && fs::read_dir(path)?
            .filter_map(std::result::Result::ok)
            .any(|entry| crate::tape::is_corpus_shard(&entry.path()))
    {
        let corpus = crate::tape::verify_corpus(path)
            .context("el corpus público no supera verify-corpus")?;
        let mut quotes = Vec::new();
        for tape in corpus.tapes {
            quotes.extend(load_public_tape_quotes(
                &path.join(tape.relative_path).join(EVENTS_FILE),
            )?);
        }
        quotes.sort_by_key(|quote| (quote.evento_unix_ms, quote.secuencia));
        return Ok(quotes);
    }
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

fn load_public_tape_quotes(events_path: &Path) -> Result<Vec<Cotizacion>> {
    type Sides = (BTreeMap<i64, f64>, BTreeMap<i64, f64>);
    let mut books = HashMap::<(String, String), Sides>::new();
    let mut quotes = Vec::new();
    for (line_no, line) in fs::read_to_string(events_path)?.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event: PublicTapeEvent = serde_json::from_str(line)
            .with_context(|| format!("{}:{}", events_path.display(), line_no + 1))?;
        let key = (event.exchange.clone(), event.pair.clone());
        let book = books.entry(key).or_default();
        if event.kind == EventKind::Snapshot {
            book.0.clear();
            book.1.clear();
        }
        apply_tape_levels(&mut book.0, &event.bids);
        apply_tape_levels(&mut book.1, &event.asks);
        let bids = book
            .0
            .iter()
            .rev()
            .take(50)
            .map(|(price, quantity)| NivelOrden {
                precio: *price as f64 / 100_000_000.0,
                cantidad: *quantity,
            })
            .collect::<Vec<_>>();
        let asks = book
            .1
            .iter()
            .take(50)
            .map(|(price, quantity)| NivelOrden {
                precio: *price as f64 / 100_000_000.0,
                cantidad: *quantity,
            })
            .collect::<Vec<_>>();
        let (Some(bid), Some(ask)) = (bids.first(), asks.first()) else {
            continue;
        };
        quotes.push(Cotizacion {
            exchange: event.exchange,
            par: event.pair,
            bid: bid.precio,
            bid_cantidad: bid.cantidad,
            ask: ask.precio,
            ask_cantidad: ask.cantidad,
            bids: bids.into(),
            asks: asks.into(),
            evento_unix_ms: event
                .exchange_timestamp
                .unwrap_or(event.local_timestamp)
                .timestamp_millis(),
            recibida_en: event.local_timestamp,
            latencia_ms: i64::try_from(event.observed_latency_ms.unwrap_or(0)).unwrap_or(i64::MAX),
            secuencia: event.sequence_id.unwrap_or(line_no as u64),
            exchange_sequence: event.sequence_id,
            integrity_status: event.integrity.status,
            resyncs: u64::from(event.integrity.resync),
            sequence_gaps: u64::from(event.integrity.gap),
            checksum_failures: 0,
            invalidated_ms: 0,
            timestamp_confiable: event.exchange_timestamp.is_some(),
            conectado: matches!(event.source, TapeSource::WebSocket { .. }),
            ultimo_mensaje: if matches!(event.source, TapeSource::Rest { .. }) {
                "rest_fallback".into()
            } else {
                String::new()
            },
        });
    }
    quotes.sort_by_key(|quote| (quote.evento_unix_ms, quote.secuencia));
    Ok(quotes)
}

fn apply_tape_levels(book: &mut BTreeMap<i64, f64>, levels: &[NivelOrden]) {
    for level in levels {
        let price = (level.precio * 100_000_000.0).round() as i64;
        if level.cantidad == 0.0 {
            book.remove(&price);
        } else {
            book.insert(price, level.cantidad);
        }
    }
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
    let f = &r.quantitative_funnel;
    let mut s=format!("# Evaluación cronológica de tape\n\n- Split: **{}/{}/{}** (A/B/C)\n- Eventos comparables A/B/C: **{}/{}/{}**\n- Quotes crudos/válidos: **{}/{}**\n- Candidatos comparables: **{}**\n- Dislocaciones brutas/netas/netas con liquidez: **{}/{}/{}**\n- Netas por millón de quotes: **{:.2}**\n- Seed de optimización: **{}** (no se usa para seleccionar con C)\n- Campeón preregistrado con B: **{}**\n- Mejor resultado ex post en C: **{}**\n- ¿El campeón ganó C?: **{}**\n\n> Política de selección: {}\n\n> Definición: {}\n\n| Estrategia | P&L neto | P&L/BTC | Max DD | Fill órdenes | Profit factor | Costos | Ventanas negativas |\n|---|---:|---:|---:|---:|---:|---:|---:|\n",r.split.train,r.split.calibration,r.split.holdout,r.event_counts[0],r.event_counts[1],r.event_counts[2],f.raw_quotes,f.valid_quotes,f.comparable_candidates,f.gross_dislocations,f.net_dislocations,f.liquid_net_dislocations,f.net_per_million_quotes,r.seed,r.preregistered_champion,r.ex_post_holdout_winner,r.champion_won_holdout,r.selection_policy,f.definition);
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
    use std::io::Write;

    use crate::types::ExchangeConfig;

    use super::*;

    fn write_native_shard(root: &Path, name: &str, timestamp_ms: i64, price: f64) {
        let shard = root.join(name);
        fs::create_dir_all(&shard).unwrap();
        let config = crate::tape::CaptureConfig {
            schema_version: 1,
            pair: "BTC/USD".into(),
            exchanges: vec!["Kraken".into()],
            depth: 10,
        };
        let config_bytes = serde_json::to_vec_pretty(&config).unwrap();
        fs::write(shard.join(crate::tape::CONFIG_FILE), &config_bytes).unwrap();
        let timestamp = DateTime::<Utc>::from_timestamp_millis(timestamp_ms).unwrap();
        let event = PublicTapeEvent {
            schema_version: 1,
            exchange_timestamp: Some(timestamp),
            local_timestamp: timestamp,
            exchange: "Kraken".into(),
            pair: "BTC/USD".into(),
            source: TapeSource::WebSocket {
                url: "wss://fixture".into(),
            },
            kind: EventKind::Snapshot,
            sequence_id: Some(timestamp_ms as u64),
            previous_sequence: None,
            bids: vec![NivelOrden {
                precio: price,
                cantidad: 1.0,
            }],
            asks: vec![NivelOrden {
                precio: price + 1.0,
                cantidad: 1.0,
            }],
            integrity: crate::tape::IntegrityState {
                status: "snapshot".into(),
                gap: false,
                resync: false,
                connection_epoch: 0,
                reconnected: false,
            },
            observed_latency_ms: Some(1),
        };
        let mut events_bytes = serde_json::to_vec(&event).unwrap();
        events_bytes.write_all(b"\n").unwrap();
        fs::write(shard.join(EVENTS_FILE), &events_bytes).unwrap();
        let manifest = crate::tape::TapeManifest {
            schema_version: 1,
            dataset_id: name.into(),
            source_classification: "public_market_capture".into(),
            started_at: timestamp - chrono::Duration::milliseconds(1),
            ended_at: timestamp + chrono::Duration::milliseconds(1),
            exchanges: vec!["Kraken".into()],
            pairs: vec!["BTC/USD".into()],
            events: 1,
            snapshots: 1,
            sequence_gaps: 0,
            rest_fallback_events: 0,
            reconnect_events: 0,
            delivery_policy: "bounded_channel_await_no_application_drop".into(),
            events_by_exchange: BTreeMap::from([("Kraken".into(), 1)]),
            uncompressed_bytes: events_bytes.len() as u64,
            duration_ms: 2,
            sha256: format!("{:x}", Sha256::digest(&events_bytes)),
            git_commit: "fixture".into(),
            config_sha256: format!("{:x}", Sha256::digest(&config_bytes)),
        };
        fs::write(
            shard.join(MANIFEST_FILE),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }
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

    #[test]
    fn quantitative_funnel_is_monotone_and_does_not_call_every_quote_a_dislocation() {
        let events = vec![
            TapeEvent {
                timestamp_ms: 1,
                buy_exchange: "A".into(),
                sell_exchange: "B".into(),
                ask: 100.0,
                bid: 101.0,
                available_btc: 1.0,
                cost_quantity_btc: 1.0,
                latency_ms: 1,
                gross_bps: 10.0,
                base_cost_bps: 4.0,
                costs: CostosOperacion::default(),
                realized_move_bps: 0.0,
            },
            TapeEvent {
                timestamp_ms: 2,
                buy_exchange: "A".into(),
                sell_exchange: "B".into(),
                ask: 100.0,
                bid: 100.1,
                available_btc: 0.0,
                cost_quantity_btc: 0.0,
                latency_ms: 1,
                gross_bps: 1.0,
                base_cost_bps: 4.0,
                costs: CostosOperacion::default(),
                realized_move_bps: 0.0,
            },
        ];
        let funnel = quantitative_funnel(&[], &events);
        assert_eq!(funnel.comparable_candidates, 2);
        assert_eq!(funnel.gross_dislocations, 2);
        assert_eq!(funnel.net_dislocations, 1);
        assert_eq!(funnel.liquid_net_dislocations, 1);
        assert!(funnel.liquid_net_dislocations <= funnel.net_dislocations);
        assert!(funnel.net_dislocations <= funnel.gross_dislocations);
        assert!(funnel.gross_dislocations <= funnel.comparable_candidates);
    }

    #[test]
    fn strategy_funnel_reports_paper_conversion_separately_from_market_counts() {
        let event = TapeEvent {
            timestamp_ms: 1,
            buy_exchange: "A".into(),
            sell_exchange: "B".into(),
            ask: 100.0,
            bid: 101.0,
            available_btc: 1.0,
            cost_quantity_btc: 1.0,
            latency_ms: 1,
            gross_bps: 10.0,
            base_cost_bps: 4.0,
            costs: CostosOperacion::default(),
            realized_move_bps: 0.0,
        };
        let metrics = Metrics {
            filled_orders: 1,
            ..Default::default()
        };
        let funnel = strategy_funnel(&[event], &metrics);
        assert_eq!(funnel.net_dislocations, 1);
        assert_eq!(funnel.paper_orders_filled, 1);
        assert_eq!(funnel.conversion_from_net, 1.0);
    }

    #[test]
    fn champion_is_selected_from_calibration_scores_before_holdout() {
        let strategies = base_strategies([0.4, 0.2, 0.2, 0.1, 0.1], 7);
        let mut scores = vec![-10.0; strategies.len()];
        scores[2] = 5.0;
        assert_eq!(
            select_by_calibration(&strategies, &scores).as_deref(),
            Some(strategies[2].name.as_str())
        );
    }

    #[test]
    fn champion_selection_rejects_non_finite_scores_and_has_deterministic_ties() {
        let strategies = base_strategies([0.4, 0.2, 0.2, 0.1, 0.1], 7);
        let mut scores = vec![f64::NAN; strategies.len()];
        assert_eq!(select_by_calibration(&strategies, &scores), None);
        scores.fill(1.0);
        let expected = strategies
            .iter()
            .map(|strategy| strategy.name.as_str())
            .min()
            .unwrap();
        assert_eq!(
            select_by_calibration(&strategies, &scores).as_deref(),
            Some(expected)
        );
    }

    #[test]
    fn verified_native_corpus_loads_all_shards_without_manual_conversion() {
        let root =
            std::env::temp_dir().join(format!("mayab-evaluation-corpus-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        write_native_shard(&root, "shard-000001", 1_000, 100.0);
        write_native_shard(&root, "shard-000002", 2_000, 101.0);
        let quotes = load_quotes(&root).unwrap();
        assert_eq!(quotes.len(), 2);
        assert_eq!(quotes[0].bid, 100.0);
        assert_eq!(quotes[1].bid, 101.0);
        let _ = fs::remove_dir_all(root);
    }
}
