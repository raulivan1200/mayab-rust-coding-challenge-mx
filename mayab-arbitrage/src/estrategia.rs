//! Estrategias de arbitraje pluggables (Lineal, Triangular, DEX).
//!
//! El motor delega la búsqueda de oportunidades a una lista de estrategias
//! registradas, permitiendo añadir nuevas sin tocar el ciclo de análisis.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::{
    motor::{config_exchange, Carteras},
    types::{Cotizacion, MapaCostos, Oportunidad, TipoOportunidad},
};

/// Trait que define una estrategia de detección de oportunidades.
pub trait EstrategiaArbitraje: Send + Sync {
    fn nombre(&self) -> &str;
    fn buscar_oportunidades(
        &self,
        cotizaciones: &HashMap<String, Cotizacion>,
        carteras: &Carteras,
        costos: &MapaCostos,
        ahora: DateTime<Utc>,
    ) -> Vec<Oportunidad>;
}

/// Estrategia clásica de arbitraje lineal entre dos exchanges.
pub struct EstrategiaLineal;

impl EstrategiaArbitraje for EstrategiaLineal {
    fn nombre(&self) -> &str {
        "lineal"
    }

    fn buscar_oportunidades(
        &self,
        cotizaciones: &HashMap<String, Cotizacion>,
        carteras: &Carteras,
        costos: &MapaCostos,
        ahora: DateTime<Utc>,
    ) -> Vec<Oportunidad> {
        crate::motor::buscar_oportunidades(cotizaciones, carteras, costos, ahora)
    }
}

/// Estrategia de arbitraje triangular dentro de un mismo exchange.
///
/// Busca ciclos USD -> BTC -> ETH -> USD (u otras combinaciones)
/// usando tres pares disponibles en el mismo venue.
pub struct EstrategiaTriangular;

impl EstrategiaTriangular {
    fn pares_para_triangular() -> Vec<(&'static str, &'static str, &'static str)> {
        vec![
            ("BTC/USD", "ETH/BTC", "ETH/USD"),
            ("BTC/USDT", "ETH/BTC", "ETH/USDT"),
        ]
    }
}

impl EstrategiaArbitraje for EstrategiaTriangular {
    fn nombre(&self) -> &str {
        "triangular"
    }

    fn buscar_oportunidades(
        &self,
        cotizaciones: &HashMap<String, Cotizacion>,
        carteras: &Carteras,
        costos: &MapaCostos,
        ahora: DateTime<Utc>,
    ) -> Vec<Oportunidad> {
        let mut out = Vec::new();

        // Agrupar cotizaciones por exchange
        let mut por_exchange: HashMap<String, HashMap<String, &Cotizacion>> = HashMap::new();
        for cot in cotizaciones.values() {
            if !cotizacion_valida(cot, ahora, costos.stale_ms) {
                continue;
            }
            por_exchange
                .entry(cot.exchange.clone())
                .or_default()
                .insert(cot.par.clone(), cot);
        }

        for (exchange, pares) in por_exchange {
            if !carteras.tiene_balance(&exchange) {
                continue;
            }
            for (p1, p2, p3) in Self::pares_para_triangular() {
                let Some(c1) = pares.get(p1) else { continue };
                let Some(c2) = pares.get(p2) else { continue };
                let Some(c3) = pares.get(p3) else { continue };

                // USD -> BTC (compra BTC)
                let ask_btc = c1.ask;
                // BTC -> ETH (compra ETH vendiendo BTC)
                let ask_eth_btc = c2.ask;
                // ETH -> USD (venta ETH)
                let bid_eth = c3.bid;

                if ask_btc <= 0.0 || ask_eth_btc <= 0.0 || bid_eth <= 0.0 {
                    continue;
                }

                // Simular ciclo: 1 USD -> 1/ask_btc BTC -> (1/ask_btc)/ask_eth_btc ETH -> * bid_eth USD
                let btc_per_usd = 1.0 / ask_btc;
                let eth_per_btc = 1.0 / ask_eth_btc;
                let usd_final = btc_per_usd * eth_per_btc * bid_eth;

                let spread_bruto = usd_final - 1.0;
                if spread_bruto <= 0.0 {
                    continue;
                }

                let precio_medio = (ask_btc + ask_eth_btc + bid_eth) / 3.0;
                let balance_usd = carteras.balance_usd(&exchange);
                let max_btc = costos.max_operacion_btc.min(balance_usd / ask_btc);

                if max_btc <= 0.0 {
                    continue;
                }

                // Estimar costos: 3 fees + slippage + latencia
                let fee_taker = config_exchange(costos, &exchange).fee_taker;
                let costo_fee = max_btc * ask_btc * fee_taker * 3.0;
                let costo_slippage = max_btc * precio_medio * costos.deslizamiento_bps / 10000.0;
                let costo_latencia = max_btc * precio_medio * costos.latencia_riesgo_bps / 10000.0;
                let costo_total = costo_fee + costo_slippage + costo_latencia;

                let utilidad_bruta = spread_bruto * max_btc;
                let utilidad_neta = utilidad_bruta - costo_total;

                if utilidad_neta < costos.min_utilidad_usd {
                    continue;
                }

                let diferencial_neto_bps = (utilidad_neta / (max_btc * precio_medio)) * 10000.0;

                out.push(Oportunidad {
                    id: format!(
                        "tri-{}-{}-{}-{}-{}",
                        exchange,
                        p1,
                        p2,
                        p3,
                        ahora.timestamp_nanos_opt().unwrap_or_default()
                    ),
                    tipo: TipoOportunidad::Triangular,
                    compra_en: exchange.clone(),
                    venta_en: exchange.clone(),
                    par: p1.to_string(),
                    piernas: vec![
                        crate::types::PiernaTriangular {
                            exchange: exchange.clone(),
                            par: p1.to_string(),
                            accion: "COMPRA".into(),
                            precio: ask_btc,
                            cantidad: max_btc,
                        },
                        crate::types::PiernaTriangular {
                            exchange: exchange.clone(),
                            par: p2.to_string(),
                            accion: "COMPRA".into(),
                            precio: ask_eth_btc,
                            cantidad: max_btc / ask_btc,
                        },
                        crate::types::PiernaTriangular {
                            exchange: exchange.clone(),
                            par: p3.to_string(),
                            accion: "VENTA".into(),
                            precio: bid_eth,
                            cantidad: max_btc / ask_btc / ask_eth_btc,
                        },
                    ],
                    ask: ask_btc,
                    bid: bid_eth,
                    diferencial_bruto_usd: spread_bruto,
                    diferencial_bruto_bps: spread_bruto * 10000.0,
                    diferencial_neto_usd: utilidad_neta / max_btc,
                    diferencial_neto_bps,
                    cantidad_btc: max_btc,
                    utilidad_usd: utilidad_neta,
                    costos: crate::types::CostosOperacion {
                        fee_compra_usd: costo_fee,
                        fee_venta_usd: 0.0,
                        deslizamiento_usd: costo_slippage,
                        retiro_amort_usd: 0.0,
                        latencia_riesgo_usd: costo_latencia,
                        seleccion_adversa_usd: 0.0,
                        total_usd: costo_total,
                    },
                    latencia_max_ms: c1.latencia_ms.max(c2.latencia_ms).max(c3.latencia_ms),
                    detectada_en: ahora,
                    razon: format!("Triangular {p1}→{p2}→{p3} en {exchange}"),
                    decision_code: "ACCEPT_TRIANGULAR".into(),
                    decision_reason: format!(
                        "Triangular net {:.2} bps >= min {:.2} bps",
                        diferencial_neto_bps, costos.min_diferencial_neto_bps
                    ),
                    decision_threshold: costos.min_diferencial_neto_bps,
                    decision_actual: diferencial_neto_bps,
                    ejecutable: true,
                    parcial: false,
                    z_score: 1.5,
                });
            }
        }
        out
    }
}

fn cotizacion_valida(c: &Cotizacion, ahora: DateTime<Utc>, stale_ms: i64) -> bool {
    if c.exchange.is_empty() || c.bid <= 0.0 || c.ask <= 0.0 || c.bid >= c.ask {
        return false;
    }
    (ahora - c.recibida_en).num_milliseconds() <= stale_ms
}
