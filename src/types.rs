//! Contratos JSON compartidos entre motor, API, exports y dashboard.
//!
//! Los nombres Rust se mantienen en `snake_case`; los nombres JSON usan el
//! contrato camelCase esperado por el frontend mediante atributos Serde.

use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Nivel individual de un libro de órdenes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NivelOrden {
    pub precio: f64,
    pub cantidad: f64,
}

/// Snapshot normalizado de mejor compra/venta y profundidad disponible.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cotizacion {
    pub exchange: String,
    pub par: String,
    pub bid: f64,
    #[serde(rename = "bidCantidad")]
    pub bid_cantidad: f64,
    pub ask: f64,
    #[serde(rename = "askCantidad")]
    pub ask_cantidad: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bids: Vec<NivelOrden>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asks: Vec<NivelOrden>,
    #[serde(rename = "eventoUnixMs")]
    pub evento_unix_ms: i64,
    #[serde(rename = "recibidaEn")]
    pub recibida_en: DateTime<Utc>,
    #[serde(rename = "latenciaMs")]
    pub latencia_ms: i64,
    pub secuencia: u64,
    pub conectado: bool,
    #[serde(
        rename = "ultimoMensaje",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub ultimo_mensaje: String,
}

/// Parámetros simulados de costos y confiabilidad por exchange.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExchangeConfig {
    pub nombre: String,
    #[serde(rename = "feeTaker")]
    pub fee_taker: f64,
    #[serde(rename = "retiroBtc")]
    pub retiro_btc: f64,
    pub confiabilidad: f64,
}

/// Desglose de costos simulados de una operación.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CostosOperacion {
    #[serde(rename = "feeCompraUsd")]
    pub fee_compra_usd: f64,
    #[serde(rename = "feeVentaUsd")]
    pub fee_venta_usd: f64,
    #[serde(rename = "deslizamientoUsd")]
    pub deslizamiento_usd: f64,
    #[serde(rename = "retiroAmortUsd")]
    pub retiro_amort_usd: f64,
    #[serde(rename = "latenciaRiesgoUsd")]
    pub latencia_riesgo_usd: f64,
    #[serde(rename = "totalUsd")]
    pub total_usd: f64,
}

/// Oportunidad evaluada por el motor, ejecutable o descartada.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Oportunidad {
    pub id: String,
    #[serde(rename = "compraEn")]
    pub compra_en: String,
    #[serde(rename = "ventaEn")]
    pub venta_en: String,
    pub par: String,
    pub ask: f64,
    pub bid: f64,
    #[serde(rename = "diferencialBrutoUsd")]
    pub diferencial_bruto_usd: f64,
    #[serde(rename = "diferencialBrutoBps")]
    pub diferencial_bruto_bps: f64,
    #[serde(rename = "diferencialNetoUsd")]
    pub diferencial_neto_usd: f64,
    #[serde(rename = "diferencialNetoBps")]
    pub diferencial_neto_bps: f64,
    #[serde(rename = "cantidadBtc")]
    pub cantidad_btc: f64,
    #[serde(rename = "utilidadUsd")]
    pub utilidad_usd: f64,
    pub costos: CostosOperacion,
    #[serde(rename = "latenciaMaxMs")]
    pub latencia_max_ms: i64,
    #[serde(rename = "detectadaEn")]
    pub detectada_en: DateTime<Utc>,
    pub razon: String,
    #[serde(rename = "decisionCode")]
    pub decision_code: String,
    #[serde(rename = "decisionReason")]
    pub decision_reason: String,
    #[serde(rename = "decisionThreshold")]
    pub decision_threshold: f64,
    #[serde(rename = "decisionActual")]
    pub decision_actual: f64,
    pub ejecutable: bool,
    pub parcial: bool,
    #[serde(rename = "zScore")]
    pub z_score: f64,
}

/// Operación simulada aceptada y aplicada a carteras en memoria.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Operacion {
    pub id: String,
    #[serde(rename = "compraEn")]
    pub compra_en: String,
    #[serde(rename = "ventaEn")]
    pub venta_en: String,
    pub par: String,
    #[serde(rename = "cantidadBtc")]
    pub cantidad_btc: f64,
    #[serde(rename = "precioCompra")]
    pub precio_compra: f64,
    #[serde(rename = "precioVenta")]
    pub precio_venta: f64,
    #[serde(rename = "utilidadUsd")]
    pub utilidad_usd: f64,
    pub costos: CostosOperacion,
    pub parcial: bool,
    #[serde(rename = "ejecutadaEn")]
    pub ejecutada_en: DateTime<Utc>,
    #[serde(rename = "latenciaMaxMs")]
    pub latencia_max_ms: i64,
}

/// Evento operativo o adverso registrado durante la simulación.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventoEjecucion {
    pub id: String,
    pub tipo: String,
    pub ruta: String,
    pub detalle: String,
    pub severidad: String,
    pub tiempo: DateTime<Utc>,
    #[serde(rename = "utilidadUsd")]
    pub utilidad_usd: f64,
    #[serde(rename = "cantidadBtc")]
    pub cantidad_btc: f64,
}

/// Movimiento interno simulado para mantener balances operativos.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rebalanceo {
    pub id: String,
    pub desde: String,
    pub hacia: String,
    pub activo: String,
    pub cantidad: f64,
    #[serde(rename = "costoUsd")]
    pub costo_usd: f64,
    pub razon: String,
    pub tiempo: DateTime<Utc>,
}

/// Registro forense de una decisión de aceptación o descarte.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditoriaDecision {
    pub id: String,
    pub ruta: String,
    pub par: String,
    pub decision: String,
    #[serde(rename = "decisionCode")]
    pub decision_code: String,
    #[serde(rename = "decisionReason")]
    pub decision_reason: String,
    #[serde(rename = "decisionThreshold")]
    pub decision_threshold: f64,
    #[serde(rename = "decisionActual")]
    pub decision_actual: f64,
    pub razon: String,
    pub score: f64,
    #[serde(rename = "pesosGa")]
    pub pesos_ga: Vec<f64>,
    #[serde(rename = "utilidadUsd")]
    pub utilidad_usd: f64,
    #[serde(rename = "diferencialNetoBps")]
    pub diferencial_neto_bps: f64,
    #[serde(rename = "cantidadBtc")]
    pub cantidad_btc: f64,
    #[serde(rename = "costoTotalUsd")]
    pub costo_total_usd: f64,
    #[serde(rename = "latenciaMaxMs")]
    pub latencia_max_ms: i64,
    #[serde(rename = "zScore")]
    pub z_score: f64,
    #[serde(rename = "compraUsdAntes")]
    pub compra_usd_antes: f64,
    #[serde(rename = "ventaBtcAntes")]
    pub venta_btc_antes: f64,
    pub tiempo: DateTime<Utc>,
}

/// Contribución de una feature al score ML/GA visible para auditoría.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeatureMlEdge {
    pub nombre: String,
    pub peso: f64,
    pub valor: f64,
    pub contribucion: f64,
}

/// Resumen explicable del ranking aprendido por GA para el último candidato.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EstadoMlEdge {
    pub activo: bool,
    pub modelo: String,
    pub version: String,
    pub decision: String,
    #[serde(rename = "scoreActual")]
    pub score_actual: f64,
    #[serde(rename = "confianza")]
    pub confianza: f64,
    #[serde(rename = "expectedValueUsd")]
    pub expected_value_usd: f64,
    #[serde(rename = "survivalProbability")]
    pub survival_probability: f64,
    #[serde(rename = "fillProbability")]
    pub fill_probability: f64,
    #[serde(rename = "adverseSelectionBps")]
    pub adverse_selection_bps: f64,
    pub features: Vec<FeatureMlEdge>,
    pub explicacion: String,
}

/// Balance simulado por exchange.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Balance {
    pub exchange: String,
    #[serde(rename = "usd")]
    pub usd: f64,
    #[serde(rename = "btc")]
    pub btc: f64,
}

/// Punto temporal usado en series de PnL y diferenciales.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PuntoSerie {
    pub tiempo: DateTime<Utc>,
    pub valor: f64,
}

/// Métricas EWMA de latencia por exchange.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LatenciaExchange {
    pub exchange: String,
    #[serde(rename = "promedioMs")]
    pub promedio_ms: f64,
    #[serde(rename = "ultimoMs")]
    pub ultimo_ms: i64,
    #[serde(rename = "minMs")]
    pub min_ms: i64,
    #[serde(rename = "maxMs")]
    pub max_ms: i64,
    #[serde(rename = "p50Ms")]
    pub p50_ms: i64,
    #[serde(rename = "p95Ms")]
    pub p95_ms: i64,
    #[serde(rename = "p99Ms")]
    pub p99_ms: i64,
    pub eventos: u64,
    pub estado: String,
    #[serde(rename = "regionSugerida")]
    pub region_sugerida: String,
}

/// Métricas agregadas del motor.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Metricas {
    #[serde(rename = "uptimeSegundos")]
    pub uptime_segundos: i64,
    #[serde(rename = "eventosMercado")]
    pub eventos_mercado: u64,
    pub oportunidades: u64,
    pub operaciones: u64,
    #[serde(rename = "utilidadAcumuladaUsd")]
    pub utilidad_acumulada_usd: f64,
    #[serde(rename = "capitalInicialUsd")]
    pub capital_inicial_usd: f64,
    #[serde(rename = "capitalActualUsd")]
    pub capital_actual_usd: f64,
    #[serde(rename = "retornoBps")]
    pub retorno_bps: f64,
    #[serde(rename = "latenciaPromedioMs")]
    pub latencia_promedio_ms: f64,
    #[serde(rename = "estadoRiesgo")]
    pub estado_riesgo: String,
    pub trabajadores: usize,
    #[serde(rename = "sharpeRatio")]
    pub sharpe_ratio: f64,
    #[serde(rename = "winRate")]
    pub win_rate: f64,
    #[serde(rename = "maxDrawdownUsd")]
    pub max_drawdown_usd: f64,
    #[serde(rename = "operacionesTotales")]
    pub operaciones_totales: usize,
    #[serde(rename = "operacionesFallidas")]
    pub operaciones_fallidas: u64,
    #[serde(rename = "rebalanceosTotales")]
    pub rebalanceos_totales: usize,
    #[serde(rename = "costoRebalanceoAcumuladoUsd")]
    pub costo_rebalanceo_acumulado_usd: f64,
    #[serde(rename = "circuitBreakerActivo")]
    pub circuit_breaker_activo: bool,
    #[serde(rename = "modoConservador")]
    pub modo_conservador: bool,
    #[serde(rename = "ejecucionEnCurso")]
    pub ejecucion_en_curso: bool,
}

/// Configuración de costos, riesgo y parámetros operativos.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MapaCostos {
    #[serde(rename = "maxOperacionBtc")]
    pub max_operacion_btc: f64,
    #[serde(rename = "minUtilidadUsd")]
    pub min_utilidad_usd: f64,
    #[serde(rename = "minDiferencialNetoBps")]
    pub min_diferencial_neto_bps: f64,
    #[serde(rename = "deslizamientoBps")]
    pub deslizamiento_bps: f64,
    #[serde(rename = "latenciaRiesgoBps")]
    pub latencia_riesgo_bps: f64,
    #[serde(rename = "retiroAmortizadoBps")]
    pub retiro_amortizado_bps: f64,
    #[serde(rename = "staleMs")]
    pub stale_ms: i64,
    #[serde(rename = "enfriamientoMs")]
    pub enfriamiento_ms: i64,
    #[serde(rename = "usdtUsdPremiumBps")]
    pub usdt_usd_premium_bps: f64,
    #[serde(rename = "permitirCruceUsdUsdt", default)]
    pub permitir_cruce_usd_usdt: bool,
    #[serde(rename = "circuitBreakerPerdidaUsd")]
    pub circuit_breaker_perdida_usd: f64,
    #[serde(rename = "circuitBreakerVentanaMin")]
    pub circuit_breaker_ventana_min: i64,
    #[serde(rename = "volatilidadUmbralBps")]
    pub volatilidad_umbral_bps: f64,
    #[serde(rename = "volatilidadVentanaSeg")]
    pub volatilidad_ventana_seg: i64,
    #[serde(rename = "simularAdversidad")]
    pub simular_adversidad: bool,
    #[serde(rename = "probFalloOrden")]
    pub prob_fallo_orden: f64,
    #[serde(rename = "probMovimientoBrusco")]
    pub prob_movimiento_brusco: f64,
    #[serde(rename = "movimientoBruscoBps")]
    pub movimiento_brusco_bps: f64,
    #[serde(rename = "rebalanceUmbralPct")]
    pub rebalance_umbral_pct: f64,
    #[serde(rename = "rebalanceMaxTransferPct")]
    pub rebalance_max_transfer_pct: f64,
    #[serde(rename = "costoRebalanceoUsd", default)]
    pub costo_rebalanceo_usd: f64,
    pub exchanges: HashMap<String, ExchangeConfig>,
}

/// Estado público del algoritmo genético.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EstadoGenetico {
    pub activo: bool,
    pub generacion: i64,
    #[serde(rename = "mejorFitness")]
    pub mejor_fitness: f64,
    #[serde(rename = "fitnessPromedio")]
    pub fitness_promedio: f64,
    #[serde(rename = "retadorFitness")]
    pub retador_fitness: f64,
    pub diversidad: f64,
    #[serde(rename = "tasaMutacion")]
    pub tasa_mutacion: f64,
    #[serde(rename = "tasaCruce")]
    pub tasa_cruce: f64,
    pub poblacion: usize,
    pub convergente: bool,
    #[serde(rename = "mejoresPesos")]
    pub mejores_pesos: Vec<f64>,
    #[serde(rename = "umbralOptimizado")]
    pub umbral_optimizado: f64,
    #[serde(rename = "maxOperacionOptimizadaBtc")]
    pub max_operacion_optimizada_btc: f64,
    #[serde(rename = "toleranciaLatenciaMs")]
    pub tolerancia_latencia_ms: i64,
    #[serde(rename = "operacionesEvaluadas")]
    pub operaciones_evaluadas: usize,
    #[serde(rename = "fallosEvaluados")]
    pub fallos_evaluados: usize,
    #[serde(rename = "mejoraGeneracional")]
    pub mejora_generacional: f64,
    #[serde(rename = "temperaturaAnnealing")]
    pub temperatura_annealing: f64,
    #[serde(rename = "inyeccionesDiferenciales")]
    pub inyecciones_diferenciales: usize,
    #[serde(rename = "metaheuristicas")]
    pub metaheuristicas: Vec<String>,
}

/// Estado de la auditoría durable local.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EstadoPersistencia {
    pub activa: bool,
    pub backend: String,
    pub ruta: String,
    pub operaciones: usize,
    pub oportunidades: usize,
    pub eventos: usize,
    pub auditorias: usize,
    pub rebalanceos: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Snapshot completo expuesto por `/api/estado` y WebSocket.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EstadoPublico {
    #[serde(rename = "generadoEn")]
    pub generado_en: DateTime<Utc>,
    pub cotizaciones: Vec<Cotizacion>,
    pub oportunidades: VecDeque<Oportunidad>,
    pub operaciones: VecDeque<Operacion>,
    #[serde(rename = "eventosEjecucion")]
    pub eventos_ejecucion: VecDeque<EventoEjecucion>,
    #[serde(rename = "auditoriaDecisiones")]
    pub auditoria_decisiones: VecDeque<AuditoriaDecision>,
    pub rebalanceos: VecDeque<Rebalanceo>,
    pub balances: Vec<Balance>,
    #[serde(rename = "latenciasExchange")]
    pub latencias_exchange: Vec<LatenciaExchange>,
    #[serde(rename = "seriePnl")]
    pub serie_pnl: VecDeque<PuntoSerie>,
    #[serde(rename = "serieDiferencial")]
    pub serie_diferencial: VecDeque<PuntoSerie>,
    pub metricas: Metricas,
    pub configuracion: MapaCostos,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genetico: Option<EstadoGenetico>,
    #[serde(rename = "mlEdge", default, skip_serializing_if = "Option::is_none")]
    pub ml_edge: Option<EstadoMlEdge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistencia: Option<EstadoPersistencia>,
    #[serde(rename = "exchangesActivos")]
    pub exchanges_activos: HashMap<String, bool>,
    #[serde(rename = "paresActivos")]
    pub pares_activos: Vec<String>,
}
