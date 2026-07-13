//! Configuración del binario a partir de variables de entorno.
//!
//! Los valores numéricos inválidos se registran y se sustituyen por defaults
//! seguros. Los errores en controles de seguridad detienen el arranque.

use std::{collections::HashMap, env, time::Duration};

use crate::types::{ExchangeConfig, MapaCostos};

#[derive(Clone, Debug, PartialEq, Eq)]
/// Entorno de despliegue: development, staging, production.
pub enum Environment {
    Development,
    Staging,
    Production,
    /// Valor no reconocido. Se conserva para impedir que un typo degrade el
    /// proceso accidentalmente a desarrollo.
    Unknown(String),
}

impl Environment {
    pub fn from_env() -> Self {
        Self::from_sources(
            env::var("MAYAB_ENV").ok().as_deref(),
            env::var("ENTORNO").ok().as_deref(),
        )
    }

    fn from_sources(mayab_env: Option<&str>, entorno: Option<&str>) -> Self {
        let value = mayab_env
            .filter(|value| !value.trim().is_empty())
            .or_else(|| entorno.filter(|value| !value.trim().is_empty()))
            .unwrap_or("development")
            .trim()
            .to_ascii_lowercase();
        match value.as_str() {
            "development" | "dev" => Self::Development,
            "production" | "prod" => Self::Production,
            "staging" | "stage" => Self::Staging,
            _ => Self::Unknown(value),
        }
    }

    pub fn requires_admin_token(&self) -> bool {
        matches!(self, Self::Production | Self::Unknown(_))
    }

    pub fn min_token_length(&self) -> usize {
        match self {
            Self::Production => 32,
            Self::Staging => 16,
            Self::Development => 0,
            Self::Unknown(_) => 32,
        }
    }
}

#[derive(Clone, Debug)]
/// Configuración inicial del proceso.
///
/// Contiene parámetros de servidor, capital simulado y costos del motor. No
/// incluye llaves API ni credenciales de exchanges.
pub struct Config {
    pub port: String,
    pub par_base: String,
    pub pares_extra: Vec<String>,
    pub token_admin: Option<String>,
    pub auditoria_db_path: String,
    pub intervalo_analisis: Duration,
    pub costos: MapaCostos,
    pub capital_inicial_usd: f64,
    pub balance_inicial_btc: f64,
    pub demo_rentable_inicial: bool,
    /// Jury Mode siempre precarga evidencia reproducible al arrancar, aunque
    /// `DEMO_RENTABLE_INICIAL` se haya desactivado explícitamente.
    pub judge_mode: bool,
    pub entorno: Environment,
    pub enabled_exchanges: Vec<String>,
    pub symbols: Vec<String>,
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
        exchanges.insert(
            "Bitfinex".to_string(),
            exchange(
                "Bitfinex",
                env_f64("FEE_BITFINEX", 0.0020),
                env_f64("RETIRO_BTC_BITFINEX", 0.00010),
                0.95,
            ),
        );
        exchanges.insert(
            "KuCoin".to_string(),
            exchange(
                "KuCoin",
                env_f64("FEE_KUCOIN", 0.0010),
                env_f64("RETIRO_BTC_KUCOIN", 0.00010),
                0.94,
            ),
        );
        exchanges.insert(
            "Gate.io".to_string(),
            exchange(
                "Gate.io",
                env_f64("FEE_GATEIO", 0.0020),
                env_f64("RETIRO_BTC_GATEIO", 0.00010),
                0.93,
            ),
        );
        exchanges.insert(
            "Bitstamp".to_string(),
            exchange(
                "Bitstamp",
                env_f64("FEE_BITSTAMP", 0.0025),
                env_f64("RETIRO_BTC_BITSTAMP", 0.00010),
                0.96,
            ),
        );
        exchanges.insert(
            "Gemini".to_string(),
            exchange(
                "Gemini",
                env_f64("FEE_GEMINI", 0.0035),
                env_f64("RETIRO_BTC_GEMINI", 0.00010),
                0.97,
            ),
        );
        exchanges.insert(
            "Jupiter".to_string(),
            exchange(
                "Jupiter",
                env_f64("FEE_JUPITER", 0.0010),
                env_f64("RETIRO_BTC_JUPITER", 0.00000),
                0.92,
            ),
        );
        exchanges.insert(
            "Raydium".to_string(),
            exchange(
                "Raydium",
                env_f64("FEE_RAYDIUM", 0.0025),
                env_f64("RETIRO_BTC_RAYDIUM", 0.00000),
                0.90,
            ),
        );

        let enabled_exchanges = csv_env("ENABLED_EXCHANGES");
        if !enabled_exchanges.is_empty() {
            exchanges.retain(|nombre, _| {
                enabled_exchanges
                    .iter()
                    .any(|habilitado| habilitado.eq_ignore_ascii_case(nombre))
            });
        }
        let mut symbols = csv_env("SYMBOLS");
        if symbols.is_empty() {
            symbols.push(env_string("PAR_BASE", "BTC/USD"));
            symbols.extend(
                env_string("PARES_EXTRA", "ETH/USD,SOL/USD,BTC/USDT,ETH/BTC")
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
            );
        }
        symbols = symbols
            .into_iter()
            .map(|s| normalizar_simbolo(&s))
            .collect();
        symbols.sort();
        symbols.dedup();
        let par_base = symbols
            .first()
            .cloned()
            .unwrap_or_else(|| "BTC/USD".to_string());
        let pares_extra = symbols.iter().skip(1).cloned().collect();

        let costos = MapaCostos {
            max_operacion_btc: (positive(env_f64("MAX_OPERACION_BTC", 0.18), 0.18)),
            min_utilidad_usd: (non_negative(env_f64("MIN_UTILIDAD_USD", 1.25), 1.25)),
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
            circuit_breaker_perdida_usd: (non_negative(
                env_f64("CIRCUIT_BREAKER_PERDIDA_USD", 500.0),
                500.0,
            )),
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
            costo_rebalanceo_usd: (non_negative(env_f64("COSTO_REBALANCEO_USD", 5.0), 5.0)),
            rebalance_settlement_ms: non_negative_i64(
                env_i64("REBALANCE_SETTLEMENT_MS", 1800),
                1800,
            ),
            exchanges,
            webhook_url: env_optional("WEBHOOK_URL"),
        };

        Self {
            port: env_string("PORT", "8080"),
            par_base,
            pares_extra,
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
            judge_mode: env_bool("MAYAB_JUDGE_MODE", false),
            entorno: Environment::from_env(),
            enabled_exchanges,
            symbols,
        }
    }

    /// Rechaza configuraciones inseguras antes de abrir sockets o arrancar el motor.
    pub fn validate(&self) -> anyhow::Result<()> {
        if let Environment::Unknown(value) = &self.entorno {
            anyhow::bail!("entorno no reconocido: {value}; use development, staging o production");
        }
        if self.entorno.requires_admin_token() {
            let token = self
                .token_admin
                .as_deref()
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .ok_or_else(|| anyhow::anyhow!("ADMIN_TOKEN es obligatorio en production"))?;
            let min_length = self.entorno.min_token_length();
            if token.len() < min_length {
                anyhow::bail!(
                    "ADMIN_TOKEN debe tener al menos {min_length} caracteres en production"
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod environment_tests {
    use super::Environment;

    #[test]
    fn mayab_env_has_precedence_over_legacy_entorno() {
        assert_eq!(
            Environment::from_sources(Some("production"), Some("development")),
            Environment::Production
        );
    }

    #[test]
    fn unknown_environment_never_degrades_to_development() {
        let environment = Environment::from_sources(Some("prodution"), None);
        assert_eq!(environment, Environment::Unknown("prodution".into()));
        assert!(environment.requires_admin_token());
    }
}

fn csv_env(key: &str) -> Vec<String> {
    env::var(key)
        .ok()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn normalizar_simbolo(value: &str) -> String {
    let upper = value.trim().to_ascii_uppercase().replace('-', "/");
    if upper.contains('/') {
        return upper;
    }
    for quote in ["USDT", "USDC", "USD", "BTC", "ETH"] {
        if let Some(base) = upper.strip_suffix(quote).filter(|base| !base.is_empty()) {
            return format!("{base}/{quote}");
        }
    }
    upper
}

fn exchange(nombre: &str, fee_taker: f64, retiro_btc: f64, confiabilidad: f64) -> ExchangeConfig {
    ExchangeConfig {
        nombre: nombre.to_string(),
        fee_taker: (non_negative(fee_taker, 0.001)),
        retiro_btc: (non_negative(retiro_btc, 0.0001)),
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

#[cfg(test)]
mod config_tests {
    use super::normalizar_simbolo;

    #[test]
    fn normaliza_symbols_para_adaptadores() {
        assert_eq!(normalizar_simbolo("btc-usdt"), "BTC/USDT");
        assert_eq!(normalizar_simbolo("ETHUSD"), "ETH/USD");
    }
}
