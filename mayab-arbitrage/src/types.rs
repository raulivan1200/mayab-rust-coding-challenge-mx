//! Contratos JSON compartidos entre motor, API, exports y dashboard.
//!
//! Los nombres Rust se mantienen en `snake_case`; los nombres JSON usan el
//! contrato camelCase esperado por el frontend mediante atributos Serde.
//!
//! El contrato público conserva números JSON. El order book interno sí usa
//! enteros escalados y tipos distintos para precio/cantidad; el resto del
//! dominio mantiene aliases semánticos `f64` mientras la migración financiera
//! se completa por módulo, sin afirmar una precisión que todavía no existe.

use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

/// Alias semánticos temporales. Aclaran firmas del dominio, pero aún no
/// impiden operaciones entre unidades en tiempo de compilación.
pub type PriceUnits = f64;
pub type QtyUnits = f64;
pub type MoneyUnits = f64;
pub type RateRatio = f64;

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
    pub bid: PriceUnits,
    #[serde(rename = "bidCantidad")]
    pub bid_cantidad: QtyUnits,
    pub ask: PriceUnits,
    #[serde(rename = "askCantidad")]
    pub ask_cantidad: QtyUnits,
    #[serde(default, skip_serializing_if = "SmallVec::is_empty")]
    pub bids: SmallVec<[NivelOrden; 10]>,
    #[serde(default, skip_serializing_if = "SmallVec::is_empty")]
    pub asks: SmallVec<[NivelOrden; 10]>,
    #[serde(rename = "eventoUnixMs")]
    pub evento_unix_ms: i64,
    #[serde(rename = "recibidaEn")]
    pub recibida_en: DateTime<Utc>,
    #[serde(rename = "latenciaMs")]
    pub latencia_ms: i64,
    pub secuencia: u64,
    #[serde(
        rename = "exchangeSequence",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub exchange_sequence: Option<u64>,
    #[serde(rename = "integrityStatus", default)]
    pub integrity_status: String,
    #[serde(rename = "resyncs", default)]
    pub resyncs: u64,
    #[serde(rename = "sequenceGaps", default)]
    pub sequence_gaps: u64,
    #[serde(rename = "checksumFailures", default)]
    pub checksum_failures: u64,
    #[serde(rename = "invalidatedMs", default)]
    pub invalidated_ms: i64,
    #[serde(rename = "timestampConfiable", default)]
    pub timestamp_confiable: bool,
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
#[serde(deny_unknown_fields)]
pub struct ExchangeConfig {
    pub nombre: String,
    #[serde(rename = "feeTaker")]
    pub fee_taker: RateRatio,
    #[serde(rename = "retiroBtc")]
    pub retiro_btc: QtyUnits,
    pub confiabilidad: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum TipoOportunidad {
    #[default]
    Lineal,
    Triangular,
}

/// Representa una etapa individual de un arbitraje triangular.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PiernaTriangular {
    pub exchange: String,
    pub par: String,
    pub accion: String, // "COMPRA" o "VENTA"
    pub precio: PriceUnits,
    pub cantidad: QtyUnits,
}

/// Desglose de costos simulados de una operación.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CostosOperacion {
    #[serde(rename = "feeCompraUsd")]
    pub fee_compra_usd: MoneyUnits,
    #[serde(rename = "feeVentaUsd")]
    pub fee_venta_usd: MoneyUnits,
    #[serde(rename = "deslizamientoUsd")]
    pub deslizamiento_usd: MoneyUnits,
    #[serde(rename = "retiroAmortUsd")]
    pub retiro_amort_usd: MoneyUnits,
    #[serde(rename = "latenciaRiesgoUsd")]
    pub latencia_riesgo_usd: MoneyUnits,
    #[serde(rename = "seleccionAdversaUsd", default)]
    pub seleccion_adversa_usd: MoneyUnits,
    #[serde(rename = "totalUsd")]
    pub total_usd: MoneyUnits,
}

/// Oportunidad evaluada por el motor, ejecutable o descartada.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Oportunidad {
    pub id: String,
    #[serde(default)]
    pub tipo: TipoOportunidad,
    #[serde(rename = "compraEn")]
    pub compra_en: String,
    #[serde(rename = "ventaEn")]
    pub venta_en: String,
    pub par: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub piernas: Vec<PiernaTriangular>,
    pub ask: PriceUnits,
    pub bid: PriceUnits,
    #[serde(rename = "diferencialBrutoUsd")]
    pub diferencial_bruto_usd: MoneyUnits,
    #[serde(rename = "diferencialBrutoBps")]
    pub diferencial_bruto_bps: f64,
    #[serde(rename = "diferencialNetoUsd")]
    pub diferencial_neto_usd: MoneyUnits,
    #[serde(rename = "diferencialNetoBps")]
    pub diferencial_neto_bps: f64,
    #[serde(rename = "cantidadBtc")]
    pub cantidad_btc: QtyUnits,
    #[serde(rename = "utilidadUsd")]
    pub utilidad_usd: MoneyUnits,
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
    #[serde(default)]
    pub tipo: TipoOportunidad,
    #[serde(rename = "compraEn")]
    pub compra_en: String,
    #[serde(rename = "ventaEn")]
    pub venta_en: String,
    pub par: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub piernas: Vec<PiernaTriangular>,
    #[serde(rename = "cantidadBtc")]
    pub cantidad_btc: QtyUnits,
    #[serde(rename = "precioCompra")]
    pub precio_compra: PriceUnits,
    #[serde(rename = "precioVenta")]
    pub precio_venta: PriceUnits,
    #[serde(rename = "utilidadUsd")]
    pub utilidad_usd: MoneyUnits,
    #[serde(rename = "utilidadEsperadaUsd", default)]
    pub utilidad_esperada_usd: MoneyUnits,
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
    pub utilidad_usd: MoneyUnits,
    #[serde(rename = "cantidadBtc")]
    pub cantidad_btc: QtyUnits,
}

/// Transición auditable de la máquina de estados de ejecución por piernas.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransicionEjecucion {
    pub id: String,
    #[serde(rename = "operacionId")]
    pub operacion_id: String,
    pub ruta: String,
    #[serde(rename = "estadoAnterior")]
    pub estado_anterior: String,
    pub estado: String,
    pub pierna: String,
    pub detalle: String,
    #[serde(rename = "exposicionBtc")]
    pub exposicion_btc: QtyUnits,
    #[serde(rename = "pnlRealizadoUsd")]
    pub pnl_realizado_usd: MoneyUnits,
    pub tiempo: DateTime<Utc>,
}

/// Movimiento interno simulado para mantener balances operativos.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rebalanceo {
    pub id: String,
    pub desde: String,
    pub hacia: String,
    pub activo: String,
    pub cantidad: QtyUnits,
    #[serde(rename = "costoUsd")]
    pub costo_usd: MoneyUnits,
    pub razon: String,
    pub tiempo: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReglaRebalanceo {
    pub id: String,
    #[serde(rename = "activoBase")]
    pub activo_base: String,
    #[serde(rename = "condicionBaseMenorA")]
    pub condicion_base_menor_a: QtyUnits,
    #[serde(rename = "activoCotizacion")]
    pub activo_cotizacion: String,
    #[serde(rename = "condicionCotizacionMayorA")]
    pub condicion_cotizacion_mayor_a: MoneyUnits,
    #[serde(rename = "montoTransferencia")]
    pub monto_transferencia: QtyUnits,
    pub desde: String,
    pub hacia: String,
    pub activa: bool,
}

/// Capital de rebalanceo debitado pero todavía no disponible en destino.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransferenciaInventario {
    pub id: String,
    #[serde(rename = "rebalanceoId")]
    pub rebalanceo_id: String,
    pub desde: String,
    pub hacia: String,
    pub activo: String,
    #[serde(rename = "redElegida")]
    pub red_elegida: String,
    #[serde(rename = "retiroSuspendido")]
    pub retiro_suspendido: bool,
    #[serde(rename = "confirmacionesRequeridas")]
    pub confirmaciones_requeridas: u32,
    #[serde(rename = "cantidadBruta")]
    pub cantidad_bruta: QtyUnits,
    #[serde(rename = "cantidadNeta")]
    pub cantidad_neta: QtyUnits,
    #[serde(rename = "costoUsd")]
    pub costo_usd: MoneyUnits,
    #[serde(rename = "capitalBloqueadoUsd")]
    pub capital_bloqueado_usd: MoneyUnits,
    #[serde(rename = "probabilidadDemora")]
    pub probabilidad_demora: f64,
    pub estado: String,
    #[serde(rename = "nivelMinimoS")]
    pub nivel_minimo_s: QtyUnits,
    #[serde(rename = "objetivoS")]
    pub objetivo_s: QtyUnits,
    #[serde(rename = "bandaMuerta")]
    pub banda_muerta: QtyUnits,
    #[serde(rename = "feeActivo")]
    pub fee_activo: QtyUnits,
    #[serde(rename = "etaMs")]
    pub eta_ms: i64,
    #[serde(rename = "retrasoSimuladoMs")]
    pub retraso_simulado_ms: i64,
    #[serde(rename = "timeoutEn")]
    pub timeout_en: DateTime<Utc>,
    #[serde(rename = "costoOportunidadUsd")]
    pub costo_oportunidad_usd: MoneyUnits,
    #[serde(rename = "capacidadOperativaRestante")]
    pub capacidad_operativa_restante: QtyUnits,
    pub intentos: u32,
    #[serde(rename = "claveIdempotencia")]
    pub clave_idempotencia: String,
    #[serde(rename = "creadaEn")]
    pub creada_en: DateTime<Utc>,
    #[serde(rename = "liquidaEn")]
    pub liquida_en: DateTime<Utc>,
    #[serde(
        rename = "confirmadaEn",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub confirmada_en: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallo: Option<String>,
    pub razon: String,
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
    pub utilidad_usd: MoneyUnits,
    #[serde(rename = "diferencialNetoBps")]
    pub diferencial_neto_bps: f64,
    #[serde(rename = "cantidadBtc")]
    pub cantidad_btc: QtyUnits,
    #[serde(rename = "costoTotalUsd")]
    pub costo_total_usd: MoneyUnits,
    #[serde(rename = "latenciaMaxMs")]
    pub latencia_max_ms: i64,
    #[serde(rename = "zScore")]
    pub z_score: f64,
    #[serde(rename = "compraUsdAntes")]
    pub compra_usd_antes: MoneyUnits,
    #[serde(rename = "ventaBtcAntes")]
    pub venta_btc_antes: QtyUnits,
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
    pub expected_value_usd: MoneyUnits,
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
    pub usd: MoneyUnits,
    #[serde(rename = "btc")]
    pub btc: QtyUnits,
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

/// Telemetría interna del pipeline evento -> análisis -> decisión.
///
/// Se mantiene separada de la latencia de red para no confundir tiempo de
/// transporte del exchange con tiempo de cómputo del motor.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TelemetriaPipeline {
    #[serde(rename = "ciclosAnalisis")]
    pub ciclos_analisis: u64,
    #[serde(rename = "ciclosSinCambiosOmitidos")]
    pub ciclos_sin_cambios_omitidos: u64,
    #[serde(rename = "rutasEvaluadas")]
    pub rutas_evaluadas: u64,
    #[serde(rename = "eventosPorSegundo")]
    pub eventos_por_segundo: f64,
    #[serde(rename = "muestras")]
    pub muestras: usize,
    #[serde(rename = "computeP50Us")]
    pub compute_p50_us: u64,
    #[serde(rename = "computeP95Us")]
    pub compute_p95_us: u64,
    #[serde(rename = "computeP99Us")]
    pub compute_p99_us: u64,
    #[serde(rename = "schedulingP50Us")]
    pub scheduling_p50_us: u64,
    #[serde(rename = "schedulingP95Us")]
    pub scheduling_p95_us: u64,
    #[serde(rename = "schedulingP99Us")]
    pub scheduling_p99_us: u64,
    #[serde(rename = "quoteToDecisionP50Ms")]
    pub quote_to_decision_p50_ms: i64,
    #[serde(rename = "quoteToDecisionP95Ms")]
    pub quote_to_decision_p95_ms: i64,
    #[serde(rename = "quoteToDecisionP99Ms")]
    pub quote_to_decision_p99_ms: i64,
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
    pub utilidad_acumulada_usd: MoneyUnits,
    #[serde(rename = "capitalInicialUsd")]
    pub capital_inicial_usd: MoneyUnits,
    #[serde(rename = "capitalActualUsd")]
    pub capital_actual_usd: MoneyUnits,
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
    pub max_drawdown_usd: MoneyUnits,
    #[serde(rename = "operacionesTotales")]
    pub operaciones_totales: usize,
    #[serde(rename = "operacionesFallidas")]
    pub operaciones_fallidas: u64,
    #[serde(rename = "rebalanceosTotales")]
    pub rebalanceos_totales: usize,
    #[serde(rename = "costoRebalanceoAcumuladoUsd")]
    pub costo_rebalanceo_acumulado_usd: MoneyUnits,
    #[serde(rename = "circuitBreakerActivo")]
    pub circuit_breaker_activo: bool,
    #[serde(rename = "modoConservador")]
    pub modo_conservador: bool,
    #[serde(rename = "ejecucionEnCurso")]
    pub ejecucion_en_curso: bool,
    #[serde(rename = "sortinoRatio", default)]
    pub sortino_ratio: f64,
    #[serde(rename = "kellyCriterion", default)]
    pub kelly_criterion: f64,
    #[serde(default)]
    pub tobi: f64,
    #[serde(default)]
    pub bayesian: f64,
}

/// Configuración de costos, riesgo y parámetros operativos.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MapaCostos {
    #[serde(rename = "maxOperacionBtc")]
    pub max_operacion_btc: QtyUnits,
    #[serde(rename = "minUtilidadUsd")]
    pub min_utilidad_usd: MoneyUnits,
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
    pub circuit_breaker_perdida_usd: MoneyUnits,
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
    pub costo_rebalanceo_usd: MoneyUnits,
    #[serde(
        rename = "rebalanceSettlementMs",
        default = "default_rebalance_settlement_ms"
    )]
    pub rebalance_settlement_ms: i64,
    pub exchanges: HashMap<String, ExchangeConfig>,
    #[serde(rename = "webhookUrl", default, skip_serializing)]
    /// Destino operativo privado. Puede contener un token en la URL y por eso
    /// nunca forma parte del estado, WebSocket ni exports públicos.
    pub webhook_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PuntoPareto {
    pub x: f64,
    pub y: f64,
    pub umbral: f64,
}

/// Estado público del algoritmo genético.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidacionGa {
    pub campeon: String,
    pub challenger: String,
    pub dataset_hash: String,
    pub semillas_entrenamiento: usize,
    pub semillas_holdout: usize,
    pub holdout_sellado: bool,
    pub lectura: String,
}

impl Default for ValidacionGa {
    fn default() -> Self {
        Self {
            campeon: "baseline_hasta_validar_holdout".to_string(),
            challenger: "ga_pareto".to_string(),
            dataset_hash: crate::version::runtime_dataset_hash(),
            semillas_entrenamiento: 24,
            semillas_holdout: 24,
            holdout_sellado: true,
            lectura: "El GA live queda como challenger; la promoción a champion exige holdout sellado y validación fuera de muestra.".to_string(),
        }
    }
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
    pub max_operacion_optimizada_btc: QtyUnits,
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
    #[serde(rename = "fronteraPareto", default)]
    pub frontera_pareto: Vec<PuntoPareto>,
    #[serde(rename = "metaheuristicas")]
    pub metaheuristicas: Vec<String>,
    #[serde(rename = "validacion", default)]
    pub validacion: ValidacionGa,
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
    #[serde(default)]
    pub ejecuciones: usize,
    #[serde(rename = "dbBytes", default)]
    pub db_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(rename = "storageMode")]
    pub storage_mode: String,
    #[serde(rename = "storageStatus")]
    pub storage_status: String,
    #[serde(rename = "storagePersistent")]
    pub storage_persistent: bool,
    #[serde(rename = "queueCapacity", default)]
    pub queue_capacity: usize,
    #[serde(rename = "queuePending", default)]
    pub queue_pending: usize,
    #[serde(rename = "queueDropped", default)]
    pub queue_dropped: u64,
    #[serde(rename = "queueFailed", default)]
    pub queue_failed: u64,
    #[serde(
        rename = "queueLastError",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub queue_last_error: Option<String>,
}

impl EstadoPersistencia {
    pub fn inactiva(ruta: &str) -> Self {
        Self {
            activa: false,
            backend: "timescaledb".to_string(),
            ruta: ruta.to_string(),
            operaciones: 0,
            oportunidades: 0,
            eventos: 0,
            auditorias: 0,
            rebalanceos: 0,
            ejecuciones: 0,
            db_bytes: 0,
            error: Some("backend no disponible".to_string()),
            storage_mode: "timescaledb".to_string(),
            storage_status: "unavailable".to_string(),
            storage_persistent: false,
            queue_capacity: 0,
            queue_pending: 0,
            queue_dropped: 0,
            queue_failed: 0,
            queue_last_error: None,
        }
    }
}

/// Proveniencia de la corrida visible para separar mercado live de PnL demo.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EstadoCorrida {
    pub id: String,
    pub modo: String,
    #[serde(rename = "iniciadaEn")]
    pub iniciada_en: DateTime<Utc>,
    #[serde(rename = "fuentePnl")]
    pub fuente_pnl: String,
    #[serde(rename = "ejecucionReal")]
    pub ejecucion_real: bool,
    /// SHA-256 del tape o receta exacta que alimenta esta corrida.
    #[serde(rename = "datasetHash")]
    pub dataset_hash: String,
}

/// Snapshot completo expuesto por `/api/estado` y WebSocket.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EstadoPublico {
    #[serde(rename = "generadoEn")]
    pub generado_en: DateTime<Utc>,
    pub corrida: EstadoCorrida,
    pub cotizaciones: Vec<Cotizacion>,
    pub oportunidades: VecDeque<Oportunidad>,
    pub operaciones: VecDeque<Operacion>,
    #[serde(rename = "eventosEjecucion")]
    pub eventos_ejecucion: VecDeque<EventoEjecucion>,
    #[serde(rename = "trazasEjecucion")]
    pub trazas_ejecucion: VecDeque<TransicionEjecucion>,
    #[serde(
        rename = "ejecucionesDosPiernas",
        default,
        skip_serializing_if = "VecDeque::is_empty"
    )]
    pub ejecuciones_dos_piernas: VecDeque<crate::execution::ExecutionReport>,
    #[serde(rename = "auditoriaDecisiones")]
    pub auditoria_decisiones: VecDeque<AuditoriaDecision>,
    pub rebalanceos: VecDeque<Rebalanceo>,
    #[serde(rename = "transferenciasInventario")]
    pub transferencias_inventario: VecDeque<TransferenciaInventario>,
    pub balances: Vec<Balance>,
    #[serde(rename = "latenciasExchange")]
    pub latencias_exchange: Vec<LatenciaExchange>,
    #[serde(rename = "telemetriaPipeline")]
    pub telemetria_pipeline: TelemetriaPipeline,
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
    #[serde(rename = "reglasRebalanceo", default)]
    pub reglas_rebalanceo: Vec<ReglaRebalanceo>,
}

fn default_rebalance_settlement_ms() -> i64 {
    1_800
}

#[cfg(test)]
mod tests {
    use super::MapaCostos;

    #[test]
    fn webhook_url_is_never_serialized_in_public_configuration() {
        let config = MapaCostos {
            webhook_url: Some("https://hooks.example/secret-token".into()),
            ..MapaCostos::default()
        };

        let serialized = serde_json::to_value(config).expect("configuración serializable");

        assert!(serialized.get("webhookUrl").is_none());
        assert!(!serialized.to_string().contains("secret-token"));
    }
}
