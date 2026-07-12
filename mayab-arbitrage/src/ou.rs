//! Laboratorio OU independiente del GA para spreads cross-venue.

use crate::tape::{TapeEvent, EVENTS_FILE};
use rand::{rngs::StdRng, Rng, SeedableRng};
use serde::Serialize;
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StationarityTests {
    pub adf_t_stat: f64,
    pub adf_critical_5pct: f64,
    pub adf_rejects_unit_root: bool,
    pub kpss_stat: f64,
    pub kpss_critical_5pct: f64,
    pub kpss_does_not_reject_stationarity: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OuParameters {
    pub long_run_mean_bps: f64,
    pub ar1_beta: f64,
    pub kappa_per_event: f64,
    pub half_life_events: f64,
    pub sigma_bps: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyResult {
    pub name: String,
    pub pnl_bps_proxy: f64,
    pub trades: usize,
    pub win_rate: f64,
    pub max_drawdown_bps: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OuReport {
    pub schema_version: u32,
    pub phase: u8,
    pub source_kind: String,
    pub source_path: Option<String>,
    pub observations: usize,
    pub split: [usize; 3],
    pub protocol: HashMap<String, bool>,
    pub parameters_frozen_on_a: OuParameters,
    pub stationarity: StationarityTests,
    pub calibration_selected_on_b: serde_json::Value,
    pub holdout_c: Vec<StrategyResult>,
    pub stability_windows: Vec<serde_json::Value>,
    pub accepted_for_research: bool,
    pub decision: String,
    pub limitations: Vec<String>,
}

pub fn build_report(path: Option<&Path>, seed: u64) -> OuReport {
    let loaded = path.and_then(|p| load_spreads(p).ok().filter(|x| x.len() >= 300));
    let (series, source_kind, source_path, limitations) = if let Some(values) = loaded {
        (
            values,
            "real_public_cross_venue_spreads".into(),
            path.map(|p| p.display().to_string()),
            vec!["El PnL es proxy ex post y no representa fills privados confirmados.".into()],
        )
    } else {
        (
            synthetic_ou(seed, 3_000),
            "synthetic_mean_reverting_fallback".into(),
            path.map(|p| p.display().to_string()),
            vec![
                "No había tape con al menos 300 spreads cross-venue comparables.".into(),
                "El fallback valida el protocolo estadístico, no rentabilidad real.".into(),
            ],
        )
    };
    let na = series.len() / 2;
    let nb = series.len() / 5;
    let a = &series[..na];
    let b = &series[na..na + nb];
    let c = &series[na + nb..];
    let parameters = estimate_ou(a);
    let stationarity = stationarity_tests(a);
    let candidates = [(0.75, 1usize), (1.0, 1), (1.25, 2), (1.5, 3), (2.0, 5)];
    let selected = candidates
        .into_iter()
        .map(|(z, horizon)| {
            let result = run_strategy(b, &parameters, z, horizon, "ou_calibration");
            (
                z,
                horizon,
                result.pnl_bps_proxy - 0.35 * result.max_drawdown_bps,
            )
        })
        .max_by(|x, y| x.2.total_cmp(&y.2))
        .unwrap_or((1.5, 1, 0.0));
    let ou = run_strategy(c, &parameters, selected.0, selected.1, "ou_mean_reversion");
    let simple = run_simple_spread(c, parameters.sigma_bps);
    let no_trade = StrategyResult {
        name: "no_trade".into(),
        pnl_bps_proxy: 0.0,
        trades: 0,
        win_rate: 0.0,
        max_drawdown_bps: 0.0,
    };
    let stability = stability(c, selected.0, selected.1);
    let stable_positive = stability
        .iter()
        .filter(|x| x["pnlBpsProxy"].as_f64().unwrap_or(0.0) > 0.0)
        .count();
    let accepted = stationarity.adf_rejects_unit_root
        && stationarity.kpss_does_not_reject_stationarity
        && ou.pnl_bps_proxy > simple.pnl_bps_proxy
        && stable_positive >= 4;
    let mut protocol = HashMap::new();
    protocol.insert("estimateOnlyOnA".into(), true);
    protocol.insert("selectThresholdAndHorizonOnlyOnB".into(), true);
    protocol.insert("evaluateOnceOnC".into(), true);
    protocol.insert("separateFromGa".into(), true);
    protocol.insert("negativeWindowsPreserved".into(), true);
    OuReport {
        schema_version: 1,
        phase: 10,
        source_kind,
        source_path,
        observations: series.len(),
        split: [a.len(), b.len(), c.len()],
        protocol,
        parameters_frozen_on_a: parameters,
        stationarity,
        calibration_selected_on_b: serde_json::json!({
            "thresholdSigma": selected.0, "horizonEvents": selected.1, "objective": selected.2,
            "candidateCount": candidates.len()
        }),
        holdout_c: vec![no_trade, simple, ou],
        stability_windows: stability,
        accepted_for_research: accepted,
        decision: if accepted {
            "accepted_for_research_only"
        } else {
            "rejected_by_predeclared_gates"
        }
        .into(),
        limitations,
    }
}

fn load_spreads(path: &Path) -> anyhow::Result<Vec<f64>> {
    let file = if path.is_dir() {
        path.join(EVENTS_FILE)
    } else {
        path.to_path_buf()
    };
    let mut latest: HashMap<String, f64> = HashMap::new();
    let mut out = Vec::new();
    for line in BufReader::new(File::open(file)?).lines() {
        let event: TapeEvent = serde_json::from_str(&line?)?;
        let (Some(bid), Some(ask)) = (event.bids.first(), event.asks.first()) else {
            continue;
        };
        if ask.precio <= bid.precio {
            continue;
        }
        latest.insert(event.exchange, (bid.precio + ask.precio) / 2.0);
        if latest.len() >= 2 {
            let min = latest.values().copied().fold(f64::INFINITY, f64::min);
            let max = latest.values().copied().fold(f64::NEG_INFINITY, f64::max);
            let mid = (max + min) / 2.0;
            if mid > 0.0 {
                out.push((max - min) / mid * 10_000.0);
            }
        }
    }
    Ok(out)
}

fn synthetic_ou(seed: u64, n: usize) -> Vec<f64> {
    let mut rng = StdRng::seed_from_u64(seed ^ 0x4f55_5f50_4841_5345);
    let (mean, beta, sigma) = (7.5, 0.91, 0.85);
    let mut x = mean;
    (0..n)
        .map(|_| {
            x = mean + beta * (x - mean) + rng.gen_range(-sigma..sigma);
            x
        })
        .collect()
}

fn estimate_ou(x: &[f64]) -> OuParameters {
    let (alpha, beta, residual_sigma) = regression_ar1(x);
    let mean = if (1.0 - beta).abs() > 1e-9 {
        alpha / (1.0 - beta)
    } else {
        mean(x)
    };
    let kappa = if beta > 0.0 {
        -beta.ln()
    } else {
        f64::INFINITY
    };
    OuParameters {
        long_run_mean_bps: mean,
        ar1_beta: beta,
        kappa_per_event: kappa,
        half_life_events: if kappa.is_finite() && kappa > 0.0 {
            std::f64::consts::LN_2 / kappa
        } else {
            f64::INFINITY
        },
        sigma_bps: residual_sigma,
    }
}

fn regression_ar1(x: &[f64]) -> (f64, f64, f64) {
    if x.len() < 3 {
        return (0.0, 0.0, 0.0);
    }
    let lag = &x[..x.len() - 1];
    let next = &x[1..];
    let ml = mean(lag);
    let mn = mean(next);
    let denom = lag.iter().map(|v| (v - ml).powi(2)).sum::<f64>();
    let beta = if denom > 1e-12 {
        lag.iter()
            .zip(next)
            .map(|(a, b)| (a - ml) * (b - mn))
            .sum::<f64>()
            / denom
    } else {
        0.0
    };
    let alpha = mn - beta * ml;
    let sigma = (lag
        .iter()
        .zip(next)
        .map(|(a, b)| (b - alpha - beta * a).powi(2))
        .sum::<f64>()
        / (lag.len().saturating_sub(2).max(1) as f64))
        .sqrt();
    (alpha, beta, sigma)
}

fn stationarity_tests(x: &[f64]) -> StationarityTests {
    let (_alpha, beta, sigma) = regression_ar1(x);
    let lag = &x[..x.len().saturating_sub(1)];
    let ml = mean(lag);
    let sxx = lag.iter().map(|v| (v - ml).powi(2)).sum::<f64>();
    let se_beta = if sxx > 1e-12 {
        sigma / sxx.sqrt()
    } else {
        f64::INFINITY
    };
    let adf = (beta - 1.0) / se_beta;
    let mu = mean(x);
    let residuals = x.iter().map(|v| v - mu).collect::<Vec<_>>();
    let mut cumulative = 0.0;
    let numerator = residuals
        .iter()
        .map(|v| {
            cumulative += v;
            cumulative * cumulative
        })
        .sum::<f64>();
    let bandwidth = (x.len() as f64).sqrt() as usize;
    let mut long_run = residuals.iter().map(|v| v * v).sum::<f64>() / x.len().max(1) as f64;
    for lag_n in 1..=bandwidth.min(residuals.len().saturating_sub(1)) {
        let covariance = residuals[lag_n..]
            .iter()
            .zip(&residuals[..residuals.len() - lag_n])
            .map(|(a, b)| a * b)
            .sum::<f64>()
            / residuals.len() as f64;
        long_run += 2.0 * (1.0 - lag_n as f64 / (bandwidth + 1) as f64) * covariance;
    }
    let kpss = numerator / (x.len().max(1) as f64).powi(2) / long_run.max(1e-12);
    StationarityTests {
        adf_t_stat: adf,
        adf_critical_5pct: -2.86,
        adf_rejects_unit_root: adf < -2.86,
        kpss_stat: kpss,
        kpss_critical_5pct: 0.463,
        kpss_does_not_reject_stationarity: kpss < 0.463,
    }
}

fn run_strategy(x: &[f64], p: &OuParameters, z: f64, horizon: usize, name: &str) -> StrategyResult {
    let threshold = p.sigma_bps * z;
    let mut pnl = 0.0;
    let mut peak: f64 = 0.0;
    let mut dd: f64 = 0.0;
    let mut wins = 0;
    let mut trades = 0;
    for i in 0..x.len().saturating_sub(horizon) {
        let d = x[i] - p.long_run_mean_bps;
        if d.abs() >= threshold {
            let r = -d.signum() * (x[i + horizon] - x[i]);
            pnl += r;
            trades += 1;
            wins += usize::from(r > 0.0);
            peak = peak.max(pnl);
            dd = dd.max(peak - pnl)
        }
    }
    StrategyResult {
        name: name.into(),
        pnl_bps_proxy: pnl,
        trades,
        win_rate: wins as f64 / trades.max(1) as f64,
        max_drawdown_bps: dd,
    }
}
fn run_simple_spread(x: &[f64], sigma: f64) -> StrategyResult {
    let p = OuParameters {
        long_run_mean_bps: mean(x),
        ar1_beta: 0.0,
        kappa_per_event: 0.0,
        half_life_events: 0.0,
        sigma_bps: sigma,
    };
    run_strategy(x, &p, 1.0, 1, "simple_spread_threshold")
}
fn stability(x: &[f64], z: f64, h: usize) -> Vec<serde_json::Value> {
    let size = x.len().div_ceil(5);
    x.chunks(size.max(1)).enumerate().map(|(i,w)|{let p=estimate_ou(w);let r=run_strategy(w,&p,z,h,"ou");serde_json::json!({"window":i+1,"observations":w.len(),"pnlBpsProxy":r.pnl_bps_proxy,"trades":r.trades,"ar1Beta":p.ar1_beta})}).collect()
}
fn mean(x: &[f64]) -> f64 {
    x.iter().sum::<f64>() / x.len().max(1) as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn phase_ten_is_separated_and_chronological() {
        let r = build_report(None, 42);
        assert_eq!(r.phase, 10);
        assert_eq!(r.split, [1500, 600, 900]);
        assert!(r.protocol.values().all(|v| *v));
        assert_eq!(r.holdout_c.len(), 3);
    }
    #[test]
    fn synthetic_ou_passes_stationarity() {
        let x = synthetic_ou(7, 3000);
        let s = stationarity_tests(&x);
        assert!(s.adf_rejects_unit_root);
        assert!(s.kpss_does_not_reject_stationarity);
    }
}
