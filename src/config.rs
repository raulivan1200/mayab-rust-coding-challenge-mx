//! Configuración del binario a partir de variables de entorno.
//!
//! Los valores inválidos no detienen el arranque: se registran en logs y se
//! sustituyen por defaults seguros para mantener la demo operable.

use std::{collections::HashMap, env, time::Duration};

use crate::types::{ExchangeConfig, MapaCostos};

#[derive(Clone, Debug)]
/// Configuración inicial del proceso.
///
/// Contiene parámetros de servidor, capital simulado y costos del motor. No
/// incluye llaves API ni credenciales de exchanges.
pub struct Config {
    pub port: String,
    pub par_base: String,
    pub token_admin: Option<String>,
    pub auditoria_db_path: String,
    pub intervalo_analisis: Duration,
    pub costos: MapaCostos,
    pub capital_inicial_usd: f64,
    pub balance_inicial_btc: f64,
    pub demo_rentable_inicial: bool,
}

impl Config {
    /// Construye la configuración leyendo variables de entorno.
    ///
    /// Los campos numéricos se normalizan para evitar valores negativos,
    /// infinitos o no parseables.
    pub fn from_env() -> Self {
        let mut exchanges = HashMap::new();
        exchanges.insert(
            "Binance".to_string(),
            exchange(
                "Binance",
                env_f64("FEE_BINANCE", 0.0010),
                env_f64("RETIRO_BTC_BINANCE", 0.00010),
                0.98,
            ),
        );
        exchanges.insert(
            "Kraken".to_string(),
            exchange(
                "Kraken",
                env_f64("FEE_KRAKEN", 0.0026),
                env_f64("RETIRO_BTC_KRAKEN", 0.00020),
                0.97,
            ),
        );
        exchanges.insert(
            "Coinbase".to_string(),
            exchange(
                "Coinbase",
                env_f64("FEE_COINBASE", 0.0060),
                env_f64("RETIRO_BTC_COINBASE", 0.00012),
                0.96,
            ),
        );
        exchanges.insert(
            "OKX".to_string(),
            exchange(
                "OKX",
                env_f64("FEE_OKX", 0.0010),
                env_f64("RETIRO_BTC_OKX", 0.00010),
                0.96,
            ),
        );
        exchanges.insert(
            "Bybit".to_string(),
            exchange(
                "Bybit",
                env_f64("FEE_BYBIT", 0.0010),
                env_f64("RETIRO_BTC_BYBIT", 0.00010),
                0.95,
            ),
        );

        let costos = MapaCostos {
            max_operacion_btc: positive(env_f64("MAX_OPERACION_BTC", 0.18), 0.18),
            min_utilidad_usd: non_negative(env_f64("MIN_UTILIDAD_USD", 1.25), 1.25),
            min_diferencial_neto_bps: non_negative(
                env_f64_alias(&["MIN_DIFERENCIAL_NETO_BPS", "MIN_SPREAD_NETO_BPS"], 0.65),
                0.65,
            ),
            deslizamiento_bps: non_negative(
                env_f64_alias(&["DESLIZAMIENTO_BPS", "SLIPPAGE_BPS"], 0.35),
                0.35,
            ),
            latencia_riesgo_bps: non_negative(env_f64("LATENCIA_RIESGO_BPS", 0.08), 0.08),
            retiro_amortizado_bps: non_negative(env_f64("RETIRO_AMORTIZADO_BPS", 0.12), 0.12),
            stale_ms: positive_i64(env_i64("STALE_MS", 4500), 4500),
            enfriamiento_ms: non_negative_i64(
                env_i64_alias(&["ENFRIAMIENTO_MS", "COOLDOWN_MS"], 1400),
                1400,
            ),
            usdt_usd_premium_bps: non_negative(env_f64("USDT_USD_PREMIUM_BPS", 3.0), 3.0),
            permitir_cruce_usd_usdt: env_bool("PERMITIR_CRUCE_USD_USDT", false),
            circuit_breaker_perdida_usd: non_negative(
                env_f64("CIRCUIT_BREAKER_PERDIDA_USD", 500.0),
                500.0,
            ),
            circuit_breaker_ventana_min: positive_i64(
                env_i64("CIRCUIT_BREAKER_VENTANA_MIN", 10),
                10,
            ),
            volatilidad_umbral_bps: non_negative(env_f64("VOLATILIDAD_UMBRAL_BPS", 50.0), 50.0),
            volatilidad_ventana_seg: positive_i64(env_i64("VOLATILIDAD_VENTANA_SEG", 30), 30),
            simular_adversidad: env_bool("SIMULAR_ADVERSIDAD", true),
            prob_fallo_orden: prob(env_f64("PROB_FALLO_ORDEN", 0.015), 0.015),
            prob_movimiento_brusco: prob(env_f64("PROB_MOVIMIENTO_BRUSCO", 0.020), 0.020),
            movimiento_brusco_bps: non_negative(env_f64("MOVIMIENTO_BRUSCO_BPS", 7.0), 7.0),
            rebalance_umbral_pct: non_negative(env_f64("REBALANCE_UMBRAL_PCT", 35.0), 35.0),
            rebalance_max_transfer_pct: prob_pct(env_f64("REBALANCE_MAX_TRANSFER_PCT", 35.0), 35.0),
            costo_rebalanceo_usd: non_negative(env_f64("COSTO_REBALANCEO_USD", 5.0), 5.0),
            exchanges,
        };

        Self {
            port: env_string("PORT", "8080"),
            par_base: env_string("PAR_BASE", "BTC/USD"),
            token_admin: env_optional("ADMIN_TOKEN"),
            auditoria_db_path: env_string("AUDITORIA_DB_PATH", "/tmp/mayab-auditoria.sqlite"),
            intervalo_analisis: Duration::from_millis(positive_i64(
                env_i64("INTERVALO_ANALISIS_MS", 70),
                70,
            ) as u64),
            costos,
            capital_inicial_usd: positive(env_f64("CAPITAL_INICIAL_USD", 250000.0), 250000.0),
            balance_inicial_btc: positive(env_f64("BALANCE_INICIAL_BTC", 1.25), 1.25),
            demo_rentable_inicial: env_bool("DEMO_RENTABLE_INICIAL", true),
        }
    }
}

fn exchange(nombre: &str, fee_taker: f64, retiro_btc: f64, confiabilidad: f64) -> ExchangeConfig {
    ExchangeConfig {
        nombre: nombre.to_string(),
        fee_taker: non_negative(fee_taker, 0.001),
        retiro_btc: non_negative(retiro_btc, 0.0001),
        confiabilidad,
    }
}

fn env_string(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn env_optional(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn env_f64(key: &str, fallback: f64) -> f64 {
    env_f64_alias(&[key], fallback)
}

fn env_f64_alias(keys: &[&str], fallback: f64) -> f64 {
    for key in keys {
        if let Ok(value) = env::var(key) {
            if let Ok(parsed) = value.trim().parse::<f64>() {
                if parsed.is_finite() {
                    return parsed;
                }
            }
            tracing::warn!(
                clave = *key,
                valor = value,
                valor_por_defecto = fallback,
                "variable de entorno invalida; usando default"
            );
            return fallback;
        }
    }
    fallback
}

fn env_i64(key: &str, fallback: i64) -> i64 {
    env_i64_alias(&[key], fallback)
}

fn env_i64_alias(keys: &[&str], fallback: i64) -> i64 {
    for key in keys {
        if let Ok(value) = env::var(key) {
            if let Ok(parsed) = value.trim().parse::<i64>() {
                return parsed;
            }
            tracing::warn!(
                clave = *key,
                valor = value,
                valor_por_defecto = fallback,
                "variable de entorno invalida; usando default"
            );
            return fallback;
        }
    }
    fallback
}

fn env_bool(key: &str, fallback: bool) -> bool {
    match env::var(key) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "si" | "on"
        ),
        Err(_) => fallback,
    }
}

fn positive(value: f64, fallback: f64) -> f64 {
    if value > 0.0 && value.is_finite() {
        value
    } else {
        fallback
    }
}

fn non_negative(value: f64, fallback: f64) -> f64 {
    if value >= 0.0 && value.is_finite() {
        value
    } else {
        fallback
    }
}

fn prob(value: f64, fallback: f64) -> f64 {
    if (0.0..=1.0).contains(&value) && value.is_finite() {
        value
    } else {
        fallback
    }
}

fn prob_pct(value: f64, fallback: f64) -> f64 {
    if (0.0..=100.0).contains(&value) && value.is_finite() {
        value
    } else {
        fallback
    }
}

fn positive_i64(value: i64, fallback: i64) -> i64 {
    if value > 0 {
        value
    } else {
        fallback
    }
}

fn non_negative_i64(value: i64, fallback: i64) -> i64 {
    if value >= 0 {
        value
    } else {
        fallback
    }
}
