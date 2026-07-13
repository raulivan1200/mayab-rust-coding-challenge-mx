use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::types::MapaCostos;

pub const PUBLIC_SCHEMA_VERSION: &str = "mayab-public-v3";
pub const DEMO_RECIPE_VERSION: u32 = 3;
pub const DEMO_JURY_REFERENCE_PRICE_USD: f64 = 50_000.0;
pub const DEMO_RECIPE_STEPS: [&str; 7] = [
    "ga_replay_96",
    "partial_fill",
    "market_moved",
    "insufficient_liquidity",
    "second_leg_rejected_recovery",
    "market_profitable_18",
    "rebalance",
];

/// Identidad inmutable de la revisión compilada. Los valores se inyectan al
/// construir la imagen; los fallbacks mantienen builds locales reproducibles.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildVersion {
    pub git_sha: &'static str,
    pub build_time: &'static str,
    pub version: &'static str,
    pub environment: &'static str,
}

/// Proveniencia completa de la revisión y de la corrida que produce la
/// evidencia pública. A diferencia de [`BuildVersion`], estos campos enlazan
/// el binario con la configuración y el dataset observables en runtime.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceVersion {
    #[serde(flatten)]
    pub build: BuildVersion,
    pub schema_version: &'static str,
    pub evidence_session_id: String,
    pub dataset_hash: String,
    pub config_hash: String,
    pub hash_algorithm: &'static str,
}

pub const fn current() -> BuildVersion {
    BuildVersion {
        git_sha: match option_env!("MAYAB_GIT_SHA") {
            Some(value) => value,
            None => "local",
        },
        build_time: match option_env!("MAYAB_BUILD_TIME") {
            Some(value) => value,
            None => "not-recorded",
        },
        version: match option_env!("MAYAB_RELEASE_VERSION") {
            Some(value) => value,
            None => env!("CARGO_PKG_VERSION"),
        },
        environment: match option_env!("MAYAB_BUILD_ENV") {
            Some(value) => value,
            None => "development",
        },
    }
}

pub fn evidence(
    config: &MapaCostos,
    evidence_session_id: &str,
    dataset_hash: &str,
) -> EvidenceVersion {
    EvidenceVersion {
        build: current(),
        schema_version: PUBLIC_SCHEMA_VERSION,
        evidence_session_id: evidence_session_id.to_string(),
        dataset_hash: dataset_hash.to_string(),
        config_hash: canonical_sha256(config),
        hash_algorithm: "SHA-256",
    }
}

/// SHA-256 sobre JSON canónico (claves de objetos ordenadas recursivamente).
/// Este es el único algoritmo de huella usado por API, demo y exports.
pub fn canonical_sha256<T: Serialize>(value: &T) -> String {
    let json = serde_json::to_value(value).unwrap_or(Value::Null);
    let bytes = serde_json::to_vec(&canonical_json(json)).unwrap_or_default();
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

pub fn runtime_dataset_hash() -> String {
    if let Some(value) = option_env!("MAYAB_DATASET_HASH")
        .map(str::to_string)
        .or_else(|| std::env::var("MAYAB_DATASET_HASH").ok())
        .filter(|value| !value.trim().is_empty())
    {
        return value;
    }
    let path = std::env::var("MAYAB_RESEARCH_TAPE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "data/captura_real.json".to_string());
    match std::fs::read(Path::new(&path)) {
        Ok(bytes) => format!("sha256:{}", hex::encode(Sha256::digest(bytes))),
        Err(_) => "unavailable:no-mounted-dataset".to_string(),
    }
}

/// Identidad del input sintético fijo usado por Jury Mode. Se hashea la receta
/// y la seed, no el resultado; configuración y build tienen hashes separados.
pub fn demo_dataset_hash() -> String {
    canonical_sha256(&serde_json::json!({
        "kind": "mayab_deterministic_jury_recipe",
        "version": DEMO_RECIPE_VERSION,
        "seed": 42,
        "gaReplaySeed": "generation-derived-from-zero",
        "referencePriceUsd": DEMO_JURY_REFERENCE_PRICE_USD,
        "steps": DEMO_RECIPE_STEPS,
    }))
}

fn canonical_json(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries = map.into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonical_json(value)))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonical_json).collect()),
        scalar => scalar,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::types::ExchangeConfig;

    use super::*;

    fn config_with_order(names: &[&str]) -> MapaCostos {
        let exchanges = names
            .iter()
            .map(|name| {
                (
                    (*name).to_string(),
                    ExchangeConfig {
                        nombre: (*name).to_string(),
                        fee_taker: 0.001,
                        retiro_btc: 0.0001,
                        confiabilidad: 0.99,
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        MapaCostos {
            max_operacion_btc: 0.1,
            min_utilidad_usd: 1.0,
            min_diferencial_neto_bps: 1.0,
            deslizamiento_bps: 2.0,
            latencia_riesgo_bps: 1.0,
            retiro_amortizado_bps: 0.5,
            stale_ms: 2_000,
            enfriamiento_ms: 1_000,
            usdt_usd_premium_bps: 1.0,
            permitir_cruce_usd_usdt: false,
            circuit_breaker_perdida_usd: 100.0,
            circuit_breaker_ventana_min: 10,
            volatilidad_umbral_bps: 50.0,
            volatilidad_ventana_seg: 30,
            simular_adversidad: true,
            prob_fallo_orden: 0.01,
            prob_movimiento_brusco: 0.01,
            movimiento_brusco_bps: 10.0,
            rebalance_umbral_pct: 30.0,
            rebalance_max_transfer_pct: 50.0,
            costo_rebalanceo_usd: 5.0,
            rebalance_settlement_ms: 1_800,
            exchanges,
            webhook_url: None,
        }
    }

    #[test]
    fn version_has_non_empty_identity_fields() {
        let version = super::current();
        assert!(!version.git_sha.is_empty());
        assert!(!version.build_time.is_empty());
        assert!(!version.version.is_empty());
        assert!(!version.environment.is_empty());
    }

    #[test]
    fn evidence_version_binds_session_dataset_and_canonical_config() {
        let dataset = demo_dataset_hash();
        let first = evidence(&config_with_order(&["B", "A"]), "jury-test", &dataset);
        let second = evidence(&config_with_order(&["A", "B"]), "jury-test", &dataset);
        assert_eq!(first.schema_version, PUBLIC_SCHEMA_VERSION);
        assert_eq!(first.evidence_session_id, "jury-test");
        assert!(
            first.dataset_hash.starts_with("sha256:")
                || first.dataset_hash.starts_with("unavailable:")
        );
        assert!(first.config_hash.starts_with("sha256:"));
        assert_eq!(first.config_hash, second.config_hash);
    }
}
