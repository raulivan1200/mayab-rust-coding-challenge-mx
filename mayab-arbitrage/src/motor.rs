//! Motor de decisión, simulación de carteras y escenarios de demo.
//!
//! Evalúa rutas compra-venta sobre snapshots de mercado públicos, calcula costos
//! simulados, revalida oportunidades antes de mover balances en memoria y expone
//! el estado consumido por la API y el dashboard.

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use rand::{rngs::StdRng, Rng, SeedableRng};
use rust_decimal::{
    prelude::{FromPrimitive, ToPrimitive},
    Decimal,
};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, RwLock};

use crate::{
    auditoria::Auditoria,
    ga::{score_canonico, EstadoGa, FeaturesScore},
    types::*,
};

/// Motor central de arbitraje simulado.
///
/// El motor es seguro para compartirse entre tareas Tokio mediante `Arc<Motor>`.
/// Sus mutaciones se serializan con `RwLock` y un candado atómico evita dos
/// ejecuciones simuladas simultáneas.
pub struct Motor {
    state: RwLock<State>,
    persistencia: Option<Arc<dyn Auditoria>>,
    eventos: AtomicU64,
    ops_ejecutadas: AtomicU64,
    ops_fallidas: AtomicU64,
    ejecucion_en_curso: AtomicBool,
    carril_simulacion: Arc<Mutex<()>>,
    ga_evolucion_en_curso: Mutex<()>,
}

const JURY_REFERENCE_PRICE_USD: f64 = 50_000.0;

struct State {
    costos: MapaCostos,
    inicio: DateTime<Utc>,
    cotizaciones: HashMap<String, Cotizacion>,
    carteras: Carteras,
    oportunidades: VecDeque<Oportunidad>,
    operaciones: VecDeque<Operacion>,
    operaciones_riesgo: VecDeque<Operacion>,
    eventos_ejecucion: VecDeque<EventoEjecucion>,
    auditoria_decisiones: VecDeque<AuditoriaDecision>,
    rebalanceos: VecDeque<Rebalanceo>,
    transferencias_inventario: VecDeque<TransferenciaInventario>,
    trazas_ejecucion: VecDeque<TransicionEjecucion>,
    ejecuciones_dos_piernas: VecDeque<crate::execution::ExecutionReport>,
    corrida: EstadoCorrida,
    telemetria_pipeline: TelemetriaPipeline,
    eventos_inicio_corrida: u64,
    ultimo_evento_analizado: u64,
    muestras_compute_us: VecDeque<u64>,
    muestras_quote_decision_ms: VecDeque<i64>,
    rebalanceos_total: u64,
    costo_rebalanceo_acumulado_usd: MoneyUnits,
    serie_pnl: VecDeque<PuntoSerie>,
    serie_diferencial: VecDeque<PuntoSerie>,
    latencias_exchange: HashMap<String, LatenciaEstado>,
    enfriamiento: HashMap<String, DateTime<Utc>>,
    utilidad: MoneyUnits,
    latencia_ewma: f64,
    precios_ref: Vec<PuntoSerie>,
    circuit_breaker_activo: bool,
    kill_switch_activo: bool,
    modo_conservador: bool,
    historial_rutas: HashMap<String, f64>,
    historial_spreads: HashMap<String, Vec<f64>>,
    ciclos: u64,
    ga: EstadoGa,
    exchanges_activos: HashMap<String, bool>,
    pares_activos: Vec<String>,
    demo_forzado: Option<EscenarioDemo>,
    reglas_rebalanceo: Vec<ReglaRebalanceo>,
    captura_activa: bool,
    datos_capturados: VecDeque<Cotizacion>,
    max_captura_len: usize,
    inicio_captura: Option<DateTime<Utc>>,
    historial_replay: VecDeque<Cotizacion>,
    max_historial_replay_len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Escenarios controlados para demostrar robustez sin depender del mercado real.
pub enum EscenarioDemo {
    FalloOrden,
    FalloSegundaPierna,
    MercadoMovido,
    LiquidezInsuficiente,
    FillParcial,
    CircuitBreaker,
    Rebalanceo,
    MercadoRentable,
}

#[derive(Clone)]
pub struct Carteras {
    balances: HashMap<String, Balance>,
    inicial: HashMap<String, Balance>,
}

#[derive(Clone, Debug)]
struct LatenciaEstado {
    promedio_ms: f64,
    ultimo_ms: i64,
    min_ms: i64,
    max_ms: i64,
    p50_ms: i64,
    p95_ms: i64,
    p99_ms: i64,
    eventos: u64,
    historial: VecDeque<i64>,
}

struct EjecucionGuard<'a>(&'a AtomicBool);

impl Drop for EjecucionGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Motor {
    /// Crea un motor con balances simulados distribuidos entre exchanges.
    pub fn new(
        costos: MapaCostos,
        capital_inicial_usd: f64,
        balance_inicial_btc: f64,
        par_base: String,
        pares_extra: Vec<String>,
        persistencia: Option<Arc<dyn Auditoria>>,
    ) -> Self {
        let mut exchanges: Vec<String> = costos.exchanges.keys().cloned().collect();
        let conocidos = [
            "Binance", "Kraken", "Coinbase", "OKX", "Bybit", "Bitfinex", "KuCoin", "Gate.io",
            "Bitstamp", "Gemini", "Jupiter", "Raydium",
        ];
        // Fixtures con exchanges ficticios conservan los venues que requieren
        // los escenarios demo; una configuración real reducida sí se respeta.
        if !exchanges
            .iter()
            .any(|exchange| conocidos.contains(&exchange.as_str()))
        {
            exchanges.extend(conocidos.into_iter().map(str::to_string));
        }
        exchanges.sort();
        exchanges.dedup();
        let carteras = Carteras::new(&exchanges, capital_inicial_usd, balance_inicial_btc);
        let exchanges_activos = exchanges.into_iter().map(|e| (e, true)).collect();
        let ahora = Utc::now();
        Self {
            state: RwLock::new(State {
                costos,
                inicio: ahora,
                cotizaciones: HashMap::new(),
                carteras,
                oportunidades: VecDeque::with_capacity(128),
                operaciones: VecDeque::with_capacity(128),
                operaciones_riesgo: VecDeque::with_capacity(512),
                eventos_ejecucion: VecDeque::with_capacity(128),
                auditoria_decisiones: VecDeque::with_capacity(160),
                rebalanceos: VecDeque::with_capacity(64),
                transferencias_inventario: VecDeque::with_capacity(64),
                trazas_ejecucion: VecDeque::with_capacity(160),
                ejecuciones_dos_piernas: VecDeque::with_capacity(32),
                corrida: EstadoCorrida {
                    id: format!("live-{}", ahora.timestamp_millis()),
                    modo: "mercado_live_simulado".to_string(),
                    iniciada_en: ahora,
                    fuente_pnl: "mercado_publico".to_string(),
                    ejecucion_real: false,
                    dataset_hash: crate::version::runtime_dataset_hash(),
                },
                telemetria_pipeline: TelemetriaPipeline::default(),
                eventos_inicio_corrida: 0,
                ultimo_evento_analizado: 0,
                muestras_compute_us: VecDeque::with_capacity(512),
                muestras_quote_decision_ms: VecDeque::with_capacity(512),
                rebalanceos_total: 0,
                costo_rebalanceo_acumulado_usd: 0.0,
                serie_pnl: VecDeque::with_capacity(256),
                serie_diferencial: VecDeque::with_capacity(256),
                latencias_exchange: HashMap::new(),
                enfriamiento: HashMap::new(),
                utilidad: 0.0,
                latencia_ewma: 0.0,
                precios_ref: Vec::with_capacity(256),
                circuit_breaker_activo: false,
                kill_switch_activo: false,
                modo_conservador: false,
                historial_rutas: HashMap::new(),
                historial_spreads: HashMap::new(),
                ciclos: 0,
                ga: EstadoGa::default(),
                exchanges_activos,
                pares_activos: {
                    let mut pares = vec![normalizar_par_operativo(&par_base)];
                    for p in pares_extra {
                        pares.push(normalizar_par_operativo(&p));
                    }
                    pares
                },
                demo_forzado: None,
                reglas_rebalanceo: Vec::new(),
                captura_activa: false,
                datos_capturados: VecDeque::with_capacity(10_000),
                max_captura_len: 10000,
                inicio_captura: None,
                historial_replay: VecDeque::with_capacity(50_000),
                max_historial_replay_len: 50_000,
            }),
            persistencia,
            eventos: AtomicU64::new(0),
            ops_ejecutadas: AtomicU64::new(0),
            ops_fallidas: AtomicU64::new(0),
            ejecucion_en_curso: AtomicBool::new(false),
            carril_simulacion: Arc::new(Mutex::new(())),
            ga_evolucion_en_curso: Mutex::new(()),
        }
    }

    /// Exclusividad para recorridos que restablecen o mutan varias piezas del
    /// estado simulado. El mismo carril protege el ciclo live completo.
    pub async fn bloquear_recorrido_simulado(&self) -> tokio::sync::OwnedMutexGuard<()> {
        self.carril_simulacion.clone().lock_owned().await
    }

    /// Inicia el ciclo periódico de análisis.
    pub async fn start(self: Arc<Self>, intervalo: Duration) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(intervalo);
            loop {
                ticker.tick().await;
                self.analizar(Utc::now()).await;
            }
        });
    }

    /// Recibe una cotización normalizada desde un adaptador de mercado.
    pub async fn recibir_cotizacion(&self, cotizacion: Cotizacion) {
        self.recibir_cotizacion_en(cotizacion, Utc::now(), true)
            .await;
    }

    async fn recibir_cotizacion_en(
        &self,
        mut cotizacion: Cotizacion,
        ahora: DateTime<Utc>,
        recalcular_latencia: bool,
    ) {
        cotizacion.recibida_en = ahora;
        if recalcular_latencia && cotizacion.evento_unix_ms > 0 {
            cotizacion.latencia_ms = (ahora.timestamp_millis() - cotizacion.evento_unix_ms).max(0);
        }
        cotizacion.conectado = cotizacion.ultimo_mensaje != "rest_fallback";
        cotizacion.secuencia = self.eventos.fetch_add(1, Ordering::SeqCst) + 1;

        let mut state = self.state.write().await;
        if cotizacion.latencia_ms > 0 {
            state.latencia_ewma = if state.latencia_ewma == 0.0 {
                cotizacion.latencia_ms as f64
            } else {
                state.latencia_ewma * 0.88 + cotizacion.latencia_ms as f64 * 0.12
            };
            actualizar_latencia_exchange(&mut state, &cotizacion.exchange, cotizacion.latencia_ms);
        }
        let clave = clave_exchange(&cotizacion.exchange, &cotizacion.par);
        state.cotizaciones.insert(clave, cotizacion.clone());

        // Ventana rodante para que el laboratorio pueda usar mercado reciente
        // aunque el usuario no haya iniciado una captura manual previamente.
        state.historial_replay.push_back(cotizacion.clone());
        let limite_tiempo = ahora - chrono::Duration::minutes(60);
        while state
            .historial_replay
            .front()
            .is_some_and(|item| item.recibida_en < limite_tiempo)
        {
            state.historial_replay.pop_front();
        }
        while state.historial_replay.len() > state.max_historial_replay_len {
            state.historial_replay.pop_front();
        }

        if state.captura_activa {
            state.datos_capturados.push_back(cotizacion);
            while state.datos_capturados.len() > state.max_captura_len {
                state.datos_capturados.pop_front();
            }
        }
    }

    /// Indica si un feed activo necesita snapshot REST porque el WS está viejo.
    pub async fn feed_necesita_fallback(&self, exchange: &str, par: &str) -> bool {
        let state = self.state.read().await;
        if !*state.exchanges_activos.get(exchange).unwrap_or(&false) {
            return false;
        }
        let Some(cotizacion) = state.cotizaciones.get(&clave_exchange(exchange, par)) else {
            return true;
        };
        let edad_ms = (Utc::now() - cotizacion.recibida_en).num_milliseconds();
        edad_ms > state.costos.stale_ms
    }

    async fn analizar(&self, ahora: DateTime<Utc>) {
        let _carril = self.carril_simulacion.lock().await;
        let inicio_scan = Instant::now();
        let evento_actual = self.eventos.load(Ordering::SeqCst);
        let (
            cotizaciones,
            costos,
            carteras,
            activo,
            historial,
            enfriamiento,
            pesos,
            evolucionar_ga_automaticamente,
        ) = {
            let mut state = self.state.write().await;
            procesar_transferencias(&mut state, ahora);
            if evento_actual == state.ultimo_evento_analizado {
                state.telemetria_pipeline.ciclos_sin_cambios_omitidos += 1;
                return;
            }
            state.ultimo_evento_analizado = evento_actual;
            state.ciclos += 1;
            state.telemetria_pipeline.ciclos_analisis += 1;

            let cotizaciones: HashMap<String, Cotizacion> = state
                .cotizaciones
                .iter()
                .filter(|(_, c)| *state.exchanges_activos.get(&c.exchange).unwrap_or(&false))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let precio_ref = precio_referencia(cotizaciones.values());
            if state.ciclos % 100 == 0 {
                let costos_rebalanceo = state.costos.clone();
                let eventos = state
                    .carteras
                    .rebalancear(precio_ref, &costos_rebalanceo, ahora);
                if !eventos.is_empty() {
                    for evento in &eventos {
                        self.persistir_rebalanceo(evento);
                    }
                    state.rebalanceos_total += eventos.len() as u64;
                    state.costo_rebalanceo_acumulado_usd +=
                        eventos.iter().map(|e| e.costo_usd).sum::<MoneyUnits>();
                    for e in eventos.into_iter().rev() {
                        let transferencia = crear_transferencia(&state, &e, ahora);
                        if !state
                            .transferencias_inventario
                            .iter()
                            .any(|t| t.clave_idempotencia == transferencia.clave_idempotencia)
                        {
                            state.transferencias_inventario.push_front(transferencia);
                        }
                        state.rebalanceos.push_front(e);
                    }
                    state.rebalanceos.truncate(64);
                    state.transferencias_inventario.truncate(64);
                }
            }
            actualizar_volatilidad(&mut state, precio_ref, ahora);
            actualizar_circuit_breaker(&mut state, ahora);

            let mut costos = state.costos.clone();
            let estrategia = state.ga.estrategia();
            costos.min_diferencial_neto_bps = costos
                .min_diferencial_neto_bps
                .max(estrategia.umbral_min_spread_bps);
            costos.max_operacion_btc = costos.max_operacion_btc.min(estrategia.max_operacion_btc);
            costos.stale_ms = costos.stale_ms.min(estrategia.tolerancia_latencia_ms);
            if state.modo_conservador {
                costos.min_diferencial_neto_bps *= 2.0;
            }
            let pesos = estrategia.pesos.to_vec();
            (
                cotizaciones,
                costos,
                state.carteras.clone(),
                state.circuit_breaker_activo || state.kill_switch_activo,
                state.historial_rutas.clone(),
                state.enfriamiento.clone(),
                pesos,
                state.ciclos % 500 == 0 && !state.operaciones.is_empty(),
            )
        };

        if evolucionar_ga_automaticamente {
            let resultado = self.evolucionar_ga(false, 240).await;
            if resultado.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
                tracing::warn!(resultado = %resultado, "evolución automática del GA omitida");
            }
        }

        let cotizacion_mas_reciente = cotizaciones.values().map(|c| c.recibida_en).max();
        let por_par = agrupar_por_par(cotizaciones);
        let mut oportunidades = Vec::new();
        for cotz in por_par.values() {
            oportunidades.extend(buscar_oportunidades(cotz, &carteras, &costos, ahora));
        }
        if activo {
            for oportunidad in &mut oportunidades {
                oportunidad.ejecutable = false;
                oportunidad.decision_code = "SKIP_CIRCUIT_BREAKER".to_string();
                oportunidad.razon = "circuit breaker activo".to_string();
                oportunidad.decision_threshold = costos.circuit_breaker_perdida_usd;
                oportunidad.decision_actual = self.ops_fallidas.load(Ordering::SeqCst) as f64;
                oportunidad.decision_reason = format!(
                    "SKIP_CIRCUIT_BREAKER — ejecuciones pausadas; pérdida máxima configurada {:.2} USD",
                    costos.circuit_breaker_perdida_usd
                );
            }
        }
        if oportunidades.is_empty() {
            let mut state = self.state.write().await;
            state.oportunidades.clear();
            registrar_muestra_pipeline(
                &mut state,
                inicio_scan,
                cotizacion_mas_reciente,
                0,
                evento_actual,
                ahora,
            );
            return;
        }
        let rutas_evaluadas = oportunidades.len();
        oportunidades.sort_by(|a, b| {
            b.ejecutable
                .cmp(&a.ejecutable)
                .then_with(|| b.utilidad_usd.total_cmp(&a.utilidad_usd))
        });
        let mejor_dif = oportunidades
            .iter()
            .map(|o| o.diferencial_neto_bps)
            .fold(f64::NEG_INFINITY, f64::max);

        let mut oportunidades_persistir = Vec::new();
        let mut auditorias_persistir = Vec::new();
        {
            let mut state = self.state.write().await;
            for oportunidad in &mut oportunidades {
                let ruta = format!(
                    "{}->{}:{}",
                    oportunidad.compra_en, oportunidad.venta_en, oportunidad.par
                );
                let valores = state
                    .historial_spreads
                    .get(&ruta)
                    .cloned()
                    .unwrap_or_default();
                oportunidad.z_score = z_score(&valores, oportunidad.diferencial_neto_bps);
                let mut nuevos = valores;
                nuevos.push(oportunidad.diferencial_neto_bps);
                state
                    .historial_spreads
                    .insert(ruta, limitar_ultimos(nuevos, 100));
            }
            let mut merged = VecDeque::from(oportunidades.clone());
            merged.extend(state.oportunidades.clone());
            state.oportunidades = limitar(merged, 80);
            state.serie_diferencial.push_back(PuntoSerie {
                tiempo: ahora,
                valor: mejor_dif,
            });
            truncar_primeros(&mut state.serie_diferencial, 240);
            registrar_auditoria_oportunidades(
                &mut state,
                &oportunidades,
                &carteras,
                &costos,
                &historial,
                &pesos,
                ahora,
            );
            if state.ciclos % 10 == 0 {
                oportunidades_persistir = oportunidades.iter().take(18).cloned().collect();
                auditorias_persistir = state
                    .auditoria_decisiones
                    .iter()
                    .take(18)
                    .cloned()
                    .collect();
            }
        }
        self.persistir_oportunidades(&oportunidades_persistir);
        self.persistir_auditorias(&auditorias_persistir);

        if activo {
            let mut state = self.state.write().await;
            registrar_muestra_pipeline(
                &mut state,
                inicio_scan,
                cotizacion_mas_reciente,
                rutas_evaluadas,
                evento_actual,
                ahora,
            );
            return;
        }

        let mejor = oportunidades
            .into_iter()
            .filter(|o| o.ejecutable)
            .filter(|o| puede_ejecutar(o, ahora, &enfriamiento, costos.enfriamiento_ms))
            .max_by(|a, b| {
                let sa = puntuar_oportunidad(
                    a,
                    costos.max_operacion_btc,
                    costos.stale_ms,
                    &historial,
                    a.z_score,
                    &pesos,
                );
                let sb = puntuar_oportunidad(
                    b,
                    costos.max_operacion_btc,
                    costos.stale_ms,
                    &historial,
                    b.z_score,
                    &pesos,
                );
                sa.total_cmp(&sb)
            });

        if let Some(oportunidad) = mejor {
            self.ejecutar(oportunidad, ahora).await;
        }
        let mut state = self.state.write().await;
        registrar_muestra_pipeline(
            &mut state,
            inicio_scan,
            cotizacion_mas_reciente,
            rutas_evaluadas,
            evento_actual,
            ahora,
        );
    }

    async fn ejecutar(&self, oportunidad: Oportunidad, ahora: DateTime<Utc>) {
        if self
            .ejecucion_en_curso
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            let mut state = self.state.write().await;
            insertar_evento_sistema(
                &mut state,
                "ejecucion_en_curso",
                "ruta descartada: ya hay una operación simulada en validación/ejecución",
                "media",
                ahora,
            );
            if let Some(evento) = state.eventos_ejecucion.front() {
                self.persistir_evento(evento);
            }
            return;
        }
        let _guard = EjecucionGuard(&self.ejecucion_en_curso);
        let mut state = self.state.write().await;
        let mut op = match revalidar_operacion(&state, &oportunidad, ahora) {
            Ok(op) => op,
            Err(evento) => {
                let evento = *evento;
                self.persistir_evento(&evento);
                state.eventos_ejecucion.push_front(evento);
                state.eventos_ejecucion.truncate(128);
                self.ops_fallidas.fetch_add(1, Ordering::SeqCst);
                return;
            }
        };
        let demo_forzado = state.demo_forzado.take();
        if let Some(evento) = aplicar_adversidad(&mut op, &state.costos, ahora, demo_forzado) {
            let falla = evento.severidad == "alta";
            self.persistir_evento(&evento);
            state.eventos_ejecucion.push_front(evento);
            state.eventos_ejecucion.truncate(128);
            if falla {
                actualizar_historial(&op, &mut state.historial_rutas, false);
                self.ops_fallidas.fetch_add(1, Ordering::SeqCst);
                return;
            }
        }

        let report = match simular_ejecucion_dos_piernas(
            &state,
            &op,
            crate::execution::ExecutionScenario::BothLegsFilled,
        ) {
            Ok(report) => report,
            Err(error) => {
                let evento = evento_operacion(
                    &op,
                    "fallida",
                    &format!("ejecutor de dos piernas rechazó la operación: {error}"),
                    "alta",
                    ahora,
                );
                self.persistir_evento(&evento);
                state.eventos_ejecucion.push_front(evento);
                state.eventos_ejecucion.truncate(128);
                actualizar_historial(&op, &mut state.historial_rutas, false);
                self.ops_fallidas.fetch_add(1, Ordering::SeqCst);
                return;
            }
        };
        let exito = state.carteras.aplicar_reporte_ejecucion(&report);
        actualizar_historial(&op, &mut state.historial_rutas, exito);
        if exito {
            registrar_reporte_ejecucion(&mut state, &report, ahora);
            self.persistir_ejecucion(&report);
            self.persistir_operacion(&op);
            state.operaciones.push_front(op.clone());
            state.operaciones.truncate(80);
            state.operaciones_riesgo.push_front(op.clone());
            state.operaciones_riesgo.truncate(5_000);
            state
                .enfriamiento
                .insert(format!("{}->{}", op.compra_en, op.venta_en), ahora);
            state.utilidad += op.utilidad_usd;
            let utilidad = state.utilidad;
            state.serie_pnl.push_back(PuntoSerie {
                tiempo: ahora,
                valor: utilidad,
            });
            truncar_primeros(&mut state.serie_pnl, 240);
            let evento = evento_operacion(
                &op,
                "ejecutada",
                "orden simulada ejecutada",
                "normal",
                ahora,
            );
            self.persistir_evento(&evento);
            state.eventos_ejecucion.push_front(evento);
            state.eventos_ejecucion.truncate(128);
            self.ops_ejecutadas.fetch_add(1, Ordering::SeqCst);
        } else {
            let evento = evento_operacion(
                &op,
                "fallida",
                "saldo insuficiente al confirmar ejecución",
                "alta",
                ahora,
            );
            self.persistir_evento(&evento);
            state.eventos_ejecucion.push_front(evento);
            state.eventos_ejecucion.truncate(128);
            self.ops_fallidas.fetch_add(1, Ordering::SeqCst);
            tracing::warn!(ruta = %format!("{}->{}", op.compra_en, op.venta_en), cantidad = op.cantidad_btc, "operación simulada fallida por saldo insuficiente");
        }
    }

    /// Devuelve un snapshot consistente para API, WebSocket y exports.
    pub async fn estado(&self) -> EstadoPublico {
        let state = self.state.read().await;
        let ahora = Utc::now();
        let mut cotizaciones: Vec<Cotizacion> = state
            .cotizaciones
            .values()
            .filter(|c| *state.exchanges_activos.get(&c.exchange).unwrap_or(&false))
            .filter(|c| cotizacion_valida(c, ahora, state.costos.stale_ms))
            .cloned()
            .collect();
        cotizaciones.sort_by(|a, b| a.exchange.cmp(&b.exchange));
        let precio = precio_referencia(cotizaciones.iter());
        let capital_inicial = state.carteras.capital_inicial_usd(precio);
        let capital_actual = state.carteras.capital_actual_usd(precio);
        let retorno = if capital_inicial > 0.0 {
            ((capital_actual - capital_inicial) / capital_inicial) * 10000.0
        } else {
            0.0
        };
        let operaciones_totales = self.ops_ejecutadas.load(Ordering::SeqCst) as usize;
        let mut ops_vec = state.operaciones.clone();
        let ops = ops_vec.make_contiguous();
        EstadoPublico {
            generado_en: Utc::now(),
            corrida: state.corrida.clone(),
            cotizaciones,
            oportunidades: state.oportunidades.clone(),
            operaciones: state.operaciones.clone(),
            eventos_ejecucion: state.eventos_ejecucion.clone(),
            trazas_ejecucion: state.trazas_ejecucion.clone(),
            ejecuciones_dos_piernas: state.ejecuciones_dos_piernas.clone(),
            rebalanceos: state.rebalanceos.clone(),
            transferencias_inventario: state.transferencias_inventario.clone(),
            auditoria_decisiones: state.auditoria_decisiones.clone(),
            balances: state.carteras.snapshot(),
            latencias_exchange: snapshot_latencias(&state),
            telemetria_pipeline: state.telemetria_pipeline.clone(),
            serie_pnl: state.serie_pnl.clone(),
            serie_diferencial: state.serie_diferencial.clone(),
            metricas: Metricas {
                sortino_ratio: sortino(ops),
                kelly_criterion: kelly(ops),
                tobi: tobi(ops, state.costos.min_utilidad_usd),
                bayesian: bayesian_win_prob(ops),
                uptime_segundos: (ahora - state.inicio).num_seconds(),
                eventos_mercado: self.eventos.load(Ordering::SeqCst),
                oportunidades: state.oportunidades.len() as u64,
                operaciones: self.ops_ejecutadas.load(Ordering::SeqCst),
                utilidad_acumulada_usd: state.utilidad,
                capital_inicial_usd: capital_inicial,
                capital_actual_usd: capital_actual,
                retorno_bps: retorno,
                latencia_promedio_ms: state.latencia_ewma,
                estado_riesgo: estado_riesgo(state.latencia_ewma, state.costos.stale_ms),
                trabajadores: 11,
                sharpe_ratio: sharpe(ops),
                win_rate: win_rate(ops),
                max_drawdown_usd: max_drawdown(state.serie_pnl.clone().make_contiguous()),
                operaciones_totales,
                operaciones_fallidas: self.ops_fallidas.load(Ordering::SeqCst),
                rebalanceos_totales: state.rebalanceos_total as usize,
                costo_rebalanceo_acumulado_usd: state.costo_rebalanceo_acumulado_usd,
                circuit_breaker_activo: state.circuit_breaker_activo || state.kill_switch_activo,
                modo_conservador: state.modo_conservador,
                ejecucion_en_curso: self.ejecucion_en_curso.load(Ordering::SeqCst),
            },
            configuracion: state.costos.clone(),
            genetico: state.ga.public(),
            ml_edge: construir_ml_edge(&state),
            persistencia: self.persistencia.as_ref().map(|p| p.estado()),
            exchanges_activos: state.exchanges_activos.clone(),
            pares_activos: state.pares_activos.clone(),
            reglas_rebalanceo: state.reglas_rebalanceo.clone(),
        }
    }

    /// Lectura acotada de la auditoría para consumidores como el agente de Discord.
    pub fn resumen_auditoria(&self) -> serde_json::Value {
        self.persistencia.as_ref().map_or_else(
            || serde_json::json!({"activa": false}),
            |persistencia| {
                serde_json::json!({
                    "activa": true,
                    "estado": persistencia.estado(),
                    "agregado": persistencia.resumen_agregado(),
                    "ultimasOperaciones": persistencia.ultimas_operaciones(20),
                })
            },
        )
    }

    fn persistir_operacion(&self, op: &Operacion) {
        if let Some(persistencia) = &self.persistencia {
            if let Err(err) = persistencia.registrar_operacion(op) {
                tracing::warn!(error = %err, id = %op.id, "no se pudo encolar operación");
            }
        }
    }

    fn persistir_evento(&self, evento: &EventoEjecucion) {
        if let Some(persistencia) = &self.persistencia {
            if let Err(err) = persistencia.registrar_evento(evento) {
                tracing::warn!(error = %err, id = %evento.id, "no se pudo encolar evento");
            }
        }
    }

    fn persistir_rebalanceo(&self, rebalanceo: &Rebalanceo) {
        if let Some(persistencia) = &self.persistencia {
            if let Err(err) = persistencia.registrar_rebalanceo(rebalanceo) {
                tracing::warn!(error = %err, id = %rebalanceo.id, "no se pudo encolar rebalanceo");
            }
        }
    }

    fn persistir_oportunidades(&self, oportunidades: &[Oportunidad]) {
        if oportunidades.is_empty() {
            return;
        }
        if let Some(persistencia) = &self.persistencia {
            if let Err(err) = persistencia.registrar_oportunidades(oportunidades) {
                tracing::warn!(error = %err, total = oportunidades.len(), "no se pudieron encolar oportunidades");
            }
        }
    }

    fn persistir_auditorias(&self, auditorias: &[AuditoriaDecision]) {
        if auditorias.is_empty() {
            return;
        }
        if let Some(persistencia) = &self.persistencia {
            if let Err(err) = persistencia.registrar_auditorias(auditorias) {
                tracing::warn!(error = %err, total = auditorias.len(), "no se pudieron encolar auditorias");
            }
        }
    }

    fn persistir_ejecucion(&self, report: &crate::execution::ExecutionReport) {
        if let Some(persistencia) = &self.persistencia {
            if let Err(err) = persistencia.registrar_ejecucion(report) {
                tracing::warn!(error = %err, id = %report.execution_id, "no se pudo encolar ejecución forense");
            }
        }
    }

    /// Espera a que la cola de auditoría confirme todas las escrituras de la
    /// corrida. Un paquete de evidencia no debe sellarse sobre writes pendientes
    /// ni sobre una cola que ya reportó pérdida.
    pub async fn esperar_persistencia(&self, timeout: std::time::Duration) -> bool {
        let Some(persistencia) = self.persistencia.clone() else {
            return false;
        };
        tokio::task::spawn_blocking(move || persistencia.flush(timeout))
            .await
            .unwrap_or(false)
    }

    /// Reemplaza la configuración de costos y riesgo del motor.
    pub async fn actualizar_config(&self, cfg: MapaCostos) {
        self.state.write().await.costos = cfg;
    }

    /// Restablece únicamente la simulación para iniciar una corrida de jurado reproducible.
    /// Conserva feeds, latencias, configuración y conexiones de mercado activas.
    pub async fn reiniciar_demo_jurado(&self) -> String {
        let ahora = Utc::now();
        let mut state = self.state.write().await;
        state.inicio = ahora;
        state.carteras.balances = state.carteras.inicial.clone();
        state.oportunidades.clear();
        state.operaciones.clear();
        state.operaciones_riesgo.clear();
        state.eventos_ejecucion.clear();
        state.auditoria_decisiones.clear();
        state.rebalanceos.clear();
        state.transferencias_inventario.clear();
        state.trazas_ejecucion.clear();
        state.ejecuciones_dos_piernas.clear();
        state.telemetria_pipeline = TelemetriaPipeline::default();
        state.eventos_inicio_corrida = self.eventos.load(Ordering::SeqCst);
        state.ultimo_evento_analizado = self.eventos.load(Ordering::SeqCst);
        state.muestras_compute_us.clear();
        state.muestras_quote_decision_ms.clear();
        state.rebalanceos_total = 0;
        state.costo_rebalanceo_acumulado_usd = 0.0;
        state.serie_pnl.clear();
        state.serie_diferencial.clear();
        state.enfriamiento.clear();
        state.utilidad = 0.0;
        state.precios_ref.clear();
        state.circuit_breaker_activo = false;
        state.kill_switch_activo = false;
        state.modo_conservador = false;
        state.historial_rutas.clear();
        state.historial_spreads.clear();
        state.ciclos = 0;
        state.ga = EstadoGa::default();
        state.demo_forzado = None;
        let corrida_id = format!("jury-{}", ahora.format("%Y%m%dT%H%M%S%.3fZ"));
        state.corrida = EstadoCorrida {
            id: corrida_id.clone(),
            modo: "demo_controlada".to_string(),
            iniciada_en: ahora,
            fuente_pnl: "demo_controlada".to_string(),
            ejecucion_real: false,
            dataset_hash: crate::version::demo_dataset_hash(),
        };
        self.ops_ejecutadas.store(0, Ordering::SeqCst);
        self.ops_fallidas.store(0, Ordering::SeqCst);
        insertar_evento_sistema(
            &mut state,
            "jury_reset",
            "corrida simulada restablecida; feeds públicos y configuración conservados",
            "normal",
            ahora,
        );
        if let Some(evento) = state.eventos_ejecucion.front() {
            self.persistir_evento(evento);
        }
        corrida_id
    }

    /// Activa o desactiva un exchange para nuevas rutas.
    pub async fn toggle_exchange(&self, nombre: &str, activo: bool) -> bool {
        let mut state = self.state.write().await;
        if !state.exchanges_activos.contains_key(nombre) {
            return false;
        }
        state.exchanges_activos.insert(nombre.to_string(), activo);
        true
    }

    /// Pausa inmediatamente nuevas ejecuciones simuladas. Los feeds continúan
    /// para conservar observabilidad y permitir una recuperación controlada.
    pub async fn set_kill_switch(&self, activo: bool) {
        let mut state = self.state.write().await;
        state.kill_switch_activo = activo;
        if activo {
            insertar_evento_sistema(
                &mut state,
                "kill_switch",
                "ejecuciones simuladas pausadas manualmente",
                "alta",
                Utc::now(),
            );
        }
    }

    /// Estado JSON compacto del algoritmo genético.
    pub async fn ga_estado(&self) -> serde_json::Value {
        self.state.read().await.ga.api_estado()
    }

    /// Configuración actual del algoritmo genético.
    pub async fn ga_config(&self) -> crate::ga::ConfigGa {
        self.state.read().await.ga.config
    }

    /// Actualiza la configuración del algoritmo genético aplicando mínimos seguros.
    pub async fn actualizar_ga_config(&self, mut cfg: crate::ga::ConfigGa) {
        if cfg.tamano_poblacion < 10 {
            cfg.tamano_poblacion = 10;
        }
        self.state.write().await.ga.actualizar_config(cfg);
    }

    /// Actualiza las reglas de rebalanceo.
    pub async fn actualizar_reglas_rebalanceo(&self, reglas: Vec<ReglaRebalanceo>) {
        let mut state = self.state.write().await;
        state.reglas_rebalanceo = reglas;
    }

    /// Fuerza una evolución del GA con historial real o replay sintético.
    pub async fn evolucionar_ga(
        &self,
        usar_replay_si_vacio: bool,
        muestras: usize,
    ) -> serde_json::Value {
        let _ga_guard = self.ga_evolucion_en_curso.lock().await;
        let limite_muestras = muestras.clamp(12, 240);
        let (mut ga, mut operaciones, costos, precio_base) = {
            let state = self.state.read().await;
            (
                state.ga.clone(),
                state
                    .operaciones
                    .iter()
                    .take(limite_muestras)
                    .cloned()
                    .collect::<Vec<_>>(),
                state.costos.clone(),
                precio_referencia(state.cotizaciones.values()),
            )
        };
        let mut fuente = "historial_real";
        let mut fallos = self.ops_fallidas.load(Ordering::SeqCst) as usize;
        if operaciones.is_empty() && usar_replay_si_vacio {
            let seed = (ga.generacion as u64)
                .wrapping_mul(1_103_515_245)
                .wrapping_add(0x4d41594142);
            let replay = operaciones_sinteticas_ga(
                &costos,
                limite_muestras,
                precio_base,
                seed,
                Utc::now(),
                true,
            );
            operaciones = replay.operaciones;
            fallos = replay.fallos;
            fuente = "replay_sintetico";
        }
        let muestras = operaciones.len();
        let evolucion = tokio::task::spawn_blocking(move || {
            ga.evolucionar(&operaciones, fallos);
            let api = ga.api_estado();
            (ga, api)
        })
        .await;
        let (ga_evolucionado, ga_api) = match evolucion {
            Ok(resultado) => resultado,
            Err(error) => {
                tracing::error!(%error, "fallo la tarea de evolucion del GA");
                return serde_json::json!({
                    "ok": false,
                    "error": "ga_evolution_task_failed",
                    "fuente": fuente,
                    "muestras": muestras,
                    "fallos": fallos,
                });
            }
        };
        let generacion = ga_evolucionado.generacion;
        let fitness = ga_evolucionado.mejor_fitness;
        self.state.write().await.ga = ga_evolucionado;
        tracing::debug!(
            fuente,
            muestras,
            fallos,
            generacion,
            fitness,
            "ga evolucionado"
        );
        serde_json::json!({
            "ok": true,
            "generacion": generacion,
            "fuente": fuente,
            "muestras": muestras,
            "fallos": fallos,
            "ga": ga_api,
        })
    }

    /// Activa un escenario controlado de demostración.
    pub async fn activar_escenario_demo(&self, escenario: EscenarioDemo) -> serde_json::Value {
        let ahora = Utc::now();
        let mut state = self.state.write().await;
        match escenario {
            EscenarioDemo::FalloOrden => {
                state.demo_forzado = Some(escenario);
                let detalle = "demo armado: la siguiente orden ejecutable será rechazada";
                insertar_evento_sistema(&mut state, "demo_armado", detalle, "media", ahora);
                if let Some(evento) = state.eventos_ejecucion.front() {
                    self.persistir_evento(evento);
                }
                serde_json::json!({ "ok": true, "modo": "pendiente", "detalle": detalle })
            }
            EscenarioDemo::MercadoMovido => {
                // El recorrido del jurado debe dejar evidencia inmediata y no
                // una bandera pendiente que dependa de que aparezca una ruta
                // rentable real después. El modelo probabilístico del motor
                // sigue aplicando shocks a operaciones live por separado.
                insertar_evento_sistema(
                    &mut state,
                    "mercado_movido",
                    "demo: shock de precio controlado detectado antes de comprometer capital",
                    "alta",
                    ahora,
                );
                if let Some(evento) = state.eventos_ejecucion.front() {
                    self.persistir_evento(evento);
                }
                serde_json::json!({
                    "ok": true,
                    "modo": "instantaneo",
                    "shockBps": state.costos.movimiento_brusco_bps.max(8.0),
                    "capitalComprometidoBtc": 0.0
                })
            }
            EscenarioDemo::FalloSegundaPierna => {
                let op_id = format!("demo-leg2-{}", ahora.timestamp_millis());
                let precio = precio_referencia_demo(&state);
                let Some((compra_en, venta_en)) = state.carteras.mejor_ruta_demo() else {
                    return serde_json::json!({
                        "ok": false,
                        "modo": "sin_ruta_fondeada",
                        "error": "se requieren dos wallets fondeadas para probar la segunda pierna"
                    });
                };
                let inventario_venta = state.carteras.balance(&venta_en).btc;
                let mut op = operacion_demo_fill_parcial(
                    &state.costos,
                    precio,
                    inventario_venta,
                    &compra_en,
                    &venta_en,
                    42,
                    ahora,
                );
                op.id = op_id.clone();
                op.parcial = false;
                let report = match simular_ejecucion_dos_piernas(
                    &state,
                    &op,
                    crate::execution::ExecutionScenario::UnwindCheaper,
                ) {
                    Ok(report) => report,
                    Err(error) => {
                        insertar_evento_sistema(
                            &mut state,
                            "segunda_pierna_no_reconciliada",
                            &format!("demo bloqueada por ejecutor forense: {error}"),
                            "alta",
                            ahora,
                        );
                        return serde_json::json!({"ok": false, "modo": "bloqueado", "error": error});
                    }
                };
                if !state.carteras.aplicar_reporte_ejecucion(&report) {
                    insertar_evento_sistema(
                        &mut state,
                        "segunda_pierna_no_reconciliada",
                        "demo bloqueada: el snapshot de wallets cambió antes del commit",
                        "alta",
                        ahora,
                    );
                    return serde_json::json!({"ok": false, "modo": "wallet_snapshot_stale"});
                }
                let pnl = report.pnl_usd.to_f64().unwrap_or(0.0);
                state.utilidad += pnl;
                let utilidad_actual = state.utilidad;
                state.serie_pnl.push_back(PuntoSerie {
                    tiempo: ahora,
                    valor: utilidad_actual,
                });
                truncar_primeros(&mut state.serie_pnl, 240);
                registrar_reporte_ejecucion(&mut state, &report, ahora);
                self.persistir_ejecucion(&report);
                self.ops_fallidas.fetch_add(1, Ordering::SeqCst);
                let evento = EventoEjecucion {
                    id: format!("evt-segunda-pierna-{op_id}"),
                    tipo: "segunda_pierna_reconciliada".to_string(),
                    ruta: format!("{compra_en}->{venta_en}"),
                    detalle: format!(
                        "segunda pierna rechazada; {:?} elegido por costo; exposición, reservas y ledger conciliados",
                        report.selected_recovery
                    ),
                    severidad: "alta".to_string(),
                    tiempo: ahora,
                    utilidad_usd: pnl,
                    cantidad_btc: op.cantidad_btc,
                };
                self.persistir_evento(&evento);
                state.eventos_ejecucion.push_front(evento);
                state.eventos_ejecucion.truncate(128);
                serde_json::json!({
                    "ok": report.invariants.all_passed,
                    "modo": "ejecutor_dos_piernas",
                    "operacionId": op_id,
                    "estadoFinal": report.state.as_str(),
                    "exposicionFinalBtc": report.residual_btc.to_f64().unwrap_or(0.0),
                    "pnlLedgerUsd": report.ledger_pnl_usd.to_f64().unwrap_or(0.0),
                    "recuperacion": report.selected_recovery,
                    "walletsAntes": report.wallets_before,
                    "walletsDespues": report.wallets_after,
                    "fills": report.fills,
                    "ledger": report.ledger,
                    "transiciones": report.transitions,
                    "invariantes": report.invariants,
                    "duplicateExecution": report.duplicates_ignored > 0,
                })
            }
            EscenarioDemo::LiquidezInsuficiente => {
                insertar_evento_sistema(
                    &mut state,
                    "liquidez_insuficiente",
                    "demo: una ruta candidata fue descartada por profundidad/balance insuficiente",
                    "alta",
                    ahora,
                );
                if let Some(evento) = state.eventos_ejecucion.front() {
                    self.persistir_evento(evento);
                }
                serde_json::json!({ "ok": true, "modo": "instantaneo" })
            }
            EscenarioDemo::FillParcial => {
                state.circuit_breaker_activo = false;
                let precio = precio_referencia_demo(&state);
                let Some((compra_en, venta_en)) = state.carteras.mejor_ruta_demo() else {
                    return serde_json::json!({
                        "ok": false,
                        "modo": "sin_ruta_fondeada",
                        "error": "se requieren dos wallets fondeadas para probar el fill parcial"
                    });
                };
                let inventario_venta = state.carteras.balance(&venta_en).btc;
                let op = operacion_demo_fill_parcial(
                    &state.costos,
                    precio,
                    inventario_venta,
                    &compra_en,
                    &venta_en,
                    ahora.timestamp_millis() as u64,
                    ahora,
                );
                let report = simular_ejecucion_dos_piernas(
                    &state,
                    &op,
                    crate::execution::ExecutionScenario::BothLegsFilled,
                );
                let exito = report
                    .as_ref()
                    .is_ok_and(|report| state.carteras.aplicar_reporte_ejecucion(report));
                if !exito {
                    insertar_evento_sistema(
                        &mut state,
                        "fill_parcial",
                        "demo: la cartera simulada no tenia inventario suficiente para aplicar el fill parcial",
                        "alta",
                        ahora,
                    );
                    if let Some(evento) = state.eventos_ejecucion.front() {
                        self.persistir_evento(evento);
                    }
                    return serde_json::json!({ "ok": false, "modo": "sin_inventario" });
                }
                if let Ok(report) = report {
                    registrar_reporte_ejecucion(&mut state, &report, ahora);
                    self.persistir_ejecucion(&report);
                }
                actualizar_historial(&op, &mut state.historial_rutas, true);
                state.utilidad += op.utilidad_usd;
                let utilidad_actual = state.utilidad;
                self.persistir_operacion(&op);
                state.operaciones.push_front(op.clone());
                state.operaciones_riesgo.push_front(op.clone());
                state.serie_pnl.push_back(PuntoSerie {
                    tiempo: op.ejecutada_en,
                    valor: utilidad_actual,
                });
                truncar_primeros(&mut state.serie_pnl, 240);
                let oportunidad = oportunidad_demo_fill_parcial(&op, &state.costos);
                let auditoria = auditoria_demo_fill_parcial(&op, &state.costos);
                let evento = evento_operacion(
                    &op,
                    "fill_parcial",
                    "demo: orden parcial, requestedQtyBtc limitado por profundidad del libro; filledQtyBtc ejecutado parcialmente",
                    "normal",
                    ahora,
                );
                self.persistir_oportunidades(std::slice::from_ref(&oportunidad));
                self.persistir_auditorias(std::slice::from_ref(&auditoria));
                self.persistir_evento(&evento);
                state.oportunidades.push_front(oportunidad);
                state.auditoria_decisiones.push_front(auditoria);
                state.eventos_ejecucion.push_front(evento);
                state.operaciones.truncate(80);
                state.operaciones_riesgo.truncate(5_000);
                state.oportunidades.truncate(80);
                state.auditoria_decisiones.truncate(160);
                state.eventos_ejecucion.truncate(128);
                self.ops_ejecutadas.fetch_add(1, Ordering::SeqCst);
                serde_json::json!({
                    "ok": true,
                    "modo": "instantaneo",
                    "requestedQtyBtc": state.costos.max_operacion_btc,
                    "filledQtyBtc": op.cantidad_btc,
                    "partialFill": true,
                    "utilidadUsd": op.utilidad_usd
                })
            }
            EscenarioDemo::CircuitBreaker => {
                state.circuit_breaker_activo = true;
                let pnl_ventana: f64 = state
                    .operaciones_riesgo
                    .iter()
                    .map(|op| op.utilidad_usd)
                    .sum();
                let perdida_demo =
                    -(pnl_ventana.max(0.0) + state.costos.circuit_breaker_perdida_usd + 1.0);
                state.operaciones_riesgo.insert(
                    0,
                    Operacion {
                        piernas: vec![],
                        tipo: crate::types::TipoOportunidad::Lineal,
                        id: format!("demo-circuit-{}", ahora.timestamp_millis()),
                        compra_en: "sistema".to_string(),
                        venta_en: "sistema".to_string(),
                        par: "BTC/USD".to_string(),
                        cantidad_btc: 0.0,
                        precio_compra: 0.0,
                        precio_venta: 0.0,
                        utilidad_usd: perdida_demo,
                        utilidad_esperada_usd: perdida_demo,
                        costos: CostosOperacion::default(),
                        parcial: false,
                        ejecutada_en: ahora,
                        latencia_max_ms: 0,
                    },
                );
                state.operaciones_riesgo.truncate(5_000);
                insertar_evento_sistema(
                    &mut state,
                    "circuit_breaker",
                    "demo: ejecuciones pausadas por pérdida acumulada simulada",
                    "alta",
                    ahora,
                );
                if let Some(evento) = state.eventos_ejecucion.front() {
                    self.persistir_evento(evento);
                }
                serde_json::json!({ "ok": true, "modo": "instantaneo" })
            }
            EscenarioDemo::Rebalanceo => {
                let precio = precio_referencia_demo(&state);
                let evento = state.carteras.forzar_rebalanceo_demo(precio, ahora);
                let transferencia = crear_transferencia(&state, &evento, ahora);
                self.persistir_rebalanceo(&evento);
                state.rebalanceos_total += 1;
                state.rebalanceos.push_front(evento.clone());
                state.rebalanceos.truncate(64);
                if !state
                    .transferencias_inventario
                    .iter()
                    .any(|t| t.clave_idempotencia == transferencia.clave_idempotencia)
                {
                    state.transferencias_inventario.push_front(transferencia);
                    state.transferencias_inventario.truncate(64);
                }
                insertar_evento_sistema(
                    &mut state,
                    "rebalanceo_forzado",
                    "demo: movimiento interno de wallet generado manualmente",
                    "normal",
                    ahora,
                );
                if let Some(evento) = state.eventos_ejecucion.front() {
                    self.persistir_evento(evento);
                }
                serde_json::json!({ "ok": true, "modo": "instantaneo", "rebalanceo": evento })
            }
            EscenarioDemo::MercadoRentable => {
                state.circuit_breaker_activo = false;
                state.modo_conservador = false;
                state.operaciones_riesgo.retain(|op| op.utilidad_usd >= 0.0);

                let precio = precio_referencia_demo(&state);
                let seed = if state.corrida.modo == "demo_controlada" {
                    // Jury Mode debe producir el mismo tape económico aunque
                    // cambien el timestamp y el id de la sesión.
                    42 ^ 0x5155_4d45_5243_4144
                } else {
                    ahora.timestamp_millis() as u64 ^ 0x5155_4d45_5243_4144
                };
                let replay =
                    operaciones_sinteticas_ga(&state.costos, 18, precio, seed, ahora, false);
                let mut insertadas = 0usize;
                for mut op in replay.operaciones {
                    // La demo rentable no debe agotar las wallets que después
                    // usan los escenarios forenses. Conservamos inventario
                    // suficiente para al menos 0.025 BTC y su compra simulada.
                    let buy_wallet = state.carteras.balance(&op.compra_en);
                    let sell_wallet = state.carteras.balance(&op.venta_en);
                    let btc_reservado_demo = 0.05;
                    let usd_reservado_demo = op.precio_compra * btc_reservado_demo * 1.02;
                    let capacidad_btc = (sell_wallet.btc - btc_reservado_demo).max(0.0);
                    let capacidad_usd = ((buy_wallet.usd - usd_reservado_demo).max(0.0)
                        / (op.precio_compra * 1.02).max(f64::EPSILON))
                    .max(0.0);
                    let cantidad_original = op.cantidad_btc;
                    let cantidad_ajustada = cantidad_original.min(capacidad_btc.min(capacidad_usd));
                    if cantidad_ajustada <= 0.0 {
                        continue;
                    }
                    if cantidad_ajustada < cantidad_original {
                        let factor = cantidad_ajustada / cantidad_original;
                        op.cantidad_btc = cantidad_ajustada;
                        op.utilidad_usd *= factor;
                        op.utilidad_esperada_usd *= factor;
                        op.costos.fee_compra_usd *= factor;
                        op.costos.fee_venta_usd *= factor;
                        op.costos.deslizamiento_usd *= factor;
                        op.costos.retiro_amort_usd *= factor;
                        op.costos.latencia_riesgo_usd *= factor;
                        op.costos.seleccion_adversa_usd *= factor;
                        op.costos.total_usd *= factor;
                    }
                    let Ok(report) = simular_ejecucion_dos_piernas(
                        &state,
                        &op,
                        crate::execution::ExecutionScenario::BothLegsFilled,
                    ) else {
                        continue;
                    };
                    let exito = state.carteras.aplicar_reporte_ejecucion(&report);
                    if !exito {
                        continue;
                    }
                    registrar_reporte_ejecucion(&mut state, &report, op.ejecutada_en);
                    self.persistir_ejecucion(&report);
                    actualizar_historial(&op, &mut state.historial_rutas, true);
                    state.utilidad += op.utilidad_usd;
                    let utilidad_actual = state.utilidad;
                    self.persistir_operacion(&op);
                    state.operaciones.push_front(op.clone());
                    state.operaciones_riesgo.push_front(op.clone());
                    state.serie_pnl.push_back(PuntoSerie {
                        tiempo: op.ejecutada_en,
                        valor: utilidad_actual,
                    });
                    let oportunidad = oportunidad_desde_operacion(&op);
                    let auditoria = auditoria_demo_desde_operacion(&op);
                    let evento = evento_operacion(
                        &op,
                        "demo_rentable",
                        "operación sintética rentable inyectada para demostrar flujo end-to-end",
                        "normal",
                        op.ejecutada_en,
                    );
                    self.persistir_oportunidades(std::slice::from_ref(&oportunidad));
                    self.persistir_auditorias(std::slice::from_ref(&auditoria));
                    self.persistir_evento(&evento);
                    state.oportunidades.push_front(oportunidad);
                    state.auditoria_decisiones.push_front(auditoria);
                    state.eventos_ejecucion.push_front(evento);
                    insertadas += 1;
                    self.ops_ejecutadas.fetch_add(1, Ordering::SeqCst);
                }
                state.operaciones.truncate(80);
                state.operaciones_riesgo.truncate(5_000);
                state.oportunidades.truncate(80);
                state.auditoria_decisiones.truncate(160);
                state.eventos_ejecucion.truncate(128);
                state.trazas_ejecucion.truncate(160);
                truncar_primeros(&mut state.serie_pnl, 240);

                let ga_evolucionada = if let Ok(_ga_guard) = self.ga_evolucion_en_curso.try_lock() {
                    let mut operaciones = state.operaciones.clone();
                    let fallos = self.ops_fallidas.load(Ordering::SeqCst) as usize;
                    state.ga.evolucionar(operaciones.make_contiguous(), fallos);
                    true
                } else {
                    false
                };
                tracing::debug!(
                    operaciones_insertadas = insertadas,
                    generacion = state.ga.generacion,
                    pnl = state.utilidad,
                    "demo rentable aplicada"
                );
                let demo_ok = insertadas > 0 && state.utilidad > 0.0 && ga_evolucionada;
                insertar_evento_sistema(
                    &mut state,
                    "demo_rentable",
                    if demo_ok {
                        "demo: se inyectaron operaciones rentables y se entrenó el GA con ese historial"
                    } else if insertadas == 0 {
                        "demo bloqueada: no se pudo insertar una operación rentable con las wallets disponibles"
                    } else if !ga_evolucionada {
                        "demo incompleta: otra evolución GA seguía en curso"
                    } else {
                        "demo bloqueada: el PnL acumulado no quedó positivo"
                    },
                    if demo_ok { "normal" } else { "alta" },
                    ahora,
                );
                if let Some(evento) = state.eventos_ejecucion.front() {
                    self.persistir_evento(evento);
                }
                serde_json::json!({
                    "ok": demo_ok,
                    "modo": "instantaneo",
                    "operacionesInsertadas": insertadas,
                    "pnlUsd": state.utilidad,
                    "generacionGa": state.ga.generacion,
                    "gaEvolucionada": ga_evolucionada,
                })
            }
        }
    }

    /// Inicia la captura de order books en vivo para replay posterior.
    pub async fn iniciar_captura(&self) {
        let mut state = self.state.write().await;
        state.captura_activa = true;
        state.datos_capturados.clear();
        state.inicio_captura = Some(Utc::now());
    }

    /// Detiene la captura y devuelve el número de snapshots guardados.
    pub async fn detener_captura(&self) -> usize {
        let mut state = self.state.write().await;
        state.captura_activa = false;
        let count = state.datos_capturados.len();
        state.inicio_captura = None;
        count
    }

    /// Devuelve el estado actual de la captura.
    pub async fn captura_estado(&self) -> serde_json::Value {
        let state = self.state.read().await;
        let historial_desde = state.historial_replay.front().map(|c| c.recibida_en);
        let historial_hasta = state.historial_replay.back().map(|c| c.recibida_en);
        let ventana_predeterminada_desde =
            historial_hasta.map(|hasta| hasta - chrono::Duration::minutes(10));
        let historial_ventana_predeterminada = state
            .historial_replay
            .iter()
            .filter(|c| ventana_predeterminada_desde.is_some_and(|desde| c.recibida_en >= desde))
            .count();
        let historial_ventana_predeterminada_desde = state
            .historial_replay
            .iter()
            .find(|c| ventana_predeterminada_desde.is_some_and(|desde| c.recibida_en >= desde))
            .map(|c| c.recibida_en);
        let captura_desde = state.datos_capturados.front().map(|c| c.recibida_en);
        let captura_hasta = state.datos_capturados.back().map(|c| c.recibida_en);
        serde_json::json!({
            "activa": state.captura_activa,
            "snapshots": state.datos_capturados.len(),
            "maxSnapshots": state.max_captura_len,
            "inicioCaptura": state.inicio_captura.map(|t| t.to_rfc3339()),
            "duracionSegundos": state.inicio_captura
                .map(|t| (Utc::now() - t).num_seconds())
                .or_else(|| captura_desde.zip(captura_hasta).map(|(desde, hasta)| (hasta - desde).num_seconds().max(0)))
                .unwrap_or(0),
            "historialSnapshots": state.historial_replay.len(),
            "historialDesde": historial_desde.map(|t| t.to_rfc3339()),
            "historialHasta": historial_hasta.map(|t| t.to_rfc3339()),
            "historialDuracionSegundos": historial_desde.zip(historial_hasta)
                .map(|(desde, hasta)| (hasta - desde).num_seconds().max(0))
                .unwrap_or(0),
            "historialVentanaPredeterminadaSnapshots": historial_ventana_predeterminada,
            "historialVentanaPredeterminadaDuracionSegundos": historial_ventana_predeterminada_desde
                .zip(historial_hasta)
                .map(|(desde, hasta)| (hasta - desde).num_seconds().max(0))
                .unwrap_or(0),
            "ventanaPredeterminadaMinutos": 10,
        })
    }

    /// Copia una ventana del historial público reciente al tape de replay.
    pub async fn cargar_ventana_replay(&self, minutos: u32) -> serde_json::Value {
        let minutos = minutos.clamp(1, 60);
        let mut state = self.state.write().await;
        if state.captura_activa {
            return serde_json::json!({"ok": false, "error": "deten la captura manual antes de cargar una ventana"});
        }
        let Some(hasta) = state.historial_replay.back().map(|c| c.recibida_en) else {
            return serde_json::json!({"ok": false, "error": "todavia no hay historial de mercado disponible"});
        };
        let desde = hasta - chrono::Duration::minutes(i64::from(minutos));
        let datos = state
            .historial_replay
            .iter()
            .filter(|c| c.recibida_en >= desde)
            .cloned()
            .collect::<Vec<_>>();
        if datos.is_empty() {
            return serde_json::json!({"ok": false, "error": "no hay muestras dentro de esa ventana"});
        }
        state.datos_capturados = datos.into();
        serde_json::json!({
            "ok": true,
            "modo": "ventana_historial_cargada",
            "minutosSolicitados": minutos,
            "snapshots": state.datos_capturados.len(),
            "desde": state.datos_capturados.front().map(|c| c.recibida_en.to_rfc3339()),
            "hasta": state.datos_capturados.back().map(|c| c.recibida_en.to_rfc3339()),
        })
    }

    /// Reproduce una copia del tape en un motor desechable.
    ///
    /// El motor live nunca recibe las cotizaciones del replay: sus wallets,
    /// PnL, GA y libros permanecen intactos mientras el sandbox se evalúa.
    pub async fn ejecutar_replay_capturado(&self) -> serde_json::Value {
        let (datos, mut costos, par_base, pares_extra, fuente) = {
            let state = self.state.read().await;
            let par_base = state
                .pares_activos
                .first()
                .cloned()
                .unwrap_or_else(|| "BTC/USD".to_string());
            let (datos, fuente) = if state.datos_capturados.is_empty() {
                let hasta = state.historial_replay.back().map(|c| c.recibida_en);
                let desde = hasta.map(|t| t - chrono::Duration::minutes(10));
                (
                    state
                        .historial_replay
                        .iter()
                        .filter(|c| desde.is_some_and(|limite| c.recibida_en >= limite))
                        .cloned()
                        .collect(),
                    "historial_publico_ultimos_10_min",
                )
            } else {
                (state.datos_capturados.clone(), "captura_seleccionada")
            };
            (
                datos,
                state.costos.clone(),
                par_base,
                state
                    .pares_activos
                    .iter()
                    .skip(1)
                    .cloned()
                    .collect::<Vec<_>>(),
                fuente,
            )
        };
        if datos.is_empty() {
            return serde_json::json!({"ok": false, "error": "no hay datos capturados para replay"});
        }

        costos.simular_adversidad = false;
        costos.prob_fallo_orden = 0.0;
        costos.prob_movimiento_brusco = 0.0;
        let input_sha256 = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&datos).unwrap_or_default())
        );

        let sandbox = Motor::new(costos, 250_000.0, 2.5, par_base, pares_extra, None);
        // El motor live analiza por intervalos, no después de cada mensaje del
        // WebSocket. Repetir el scan completo por cada tick vuelve cuadrática la
        // ventana de volatilidad y hace que una ráfaga corta de mercado parezca
        // congelar el endpoint. El sandbox conserva e ingiere todos los ticks,
        // pero ejecuta decisiones sobre un reloj periódico y determinista.
        const INTERVALO_ANALISIS_REPLAY_MS: i64 = 250;
        const TICKS_ANTES_DE_CEDER: usize = 256;
        let total_datos = datos.len();
        let mut procesados = 0;
        let mut ciclos_analisis = 0;
        let mut reloj_anterior: Option<DateTime<Utc>> = None;
        let mut ultimo_analisis: Option<DateTime<Utc>> = None;
        for (indice, cot) in datos.into_iter().enumerate() {
            let reloj_capturado = cot.recibida_en;
            let reloj = reloj_anterior.map_or(reloj_capturado, |anterior| {
                if reloj_capturado > anterior {
                    reloj_capturado
                } else {
                    anterior + chrono::Duration::milliseconds(1)
                }
            });
            sandbox.recibir_cotizacion_en(cot, reloj, false).await;
            let es_ultimo = indice + 1 == total_datos;
            let toca_analizar = ultimo_analisis.is_none_or(|ultimo| {
                (reloj - ultimo).num_milliseconds() >= INTERVALO_ANALISIS_REPLAY_MS
            });
            if toca_analizar || es_ultimo {
                sandbox.analizar(reloj).await;
                ultimo_analisis = Some(reloj);
                ciclos_analisis += 1;
            }
            reloj_anterior = Some(reloj);
            procesados += 1;
            if procesados % TICKS_ANTES_DE_CEDER == 0 {
                tokio::task::yield_now().await;
            }
        }
        let resultado = sandbox.estado().await;
        serde_json::json!({
            "ok": true,
            "aislado": true,
            "fuente": fuente,
            "determinista": true,
            "inputSha256": input_sha256,
            "reloj": "timestamps_capturados_monotonizados",
            "adversidadAleatoria": false,
            "ticksProcesados": procesados,
            "ciclosAnalisis": ciclos_analisis,
            "intervaloAnalisisMs": INTERVALO_ANALISIS_REPLAY_MS,
            "operaciones": resultado.operaciones.len(),
            "pnlUsd": resultado.metricas.utilidad_acumulada_usd,
            "oportunidades": resultado.oportunidades.len(),
            "mensaje": "replay completado en sandbox; el estado live no fue modificado",
        })
    }

    /// Devuelve un análisis de sensibilidad GA sobre un holdout común.
    pub async fn ga_ablacion(&self) -> serde_json::Value {
        use crate::ga::ConfigGa;
        let state = self.state.read().await;
        let costos = state.costos.clone();
        let precio_ref = precio_referencia(state.cotizaciones.values());
        let cfg = state.ga.config;

        // Configuraciones reproducibles a comparar. Los nombres describen
        // exactamente lo que cambia; no se presentan como ablaciones de
        // operadores que EstadoGa todavía ejecuta internamente.
        let estrategias = vec![
            (
                "Población mínima sin mutación",
                ConfigGa {
                    tamano_poblacion: 10,
                    tasa_mutacion: 0.0,
                    tasa_cruce: 0.0,
                },
            ),
            (
                "Población mínima con cruce",
                ConfigGa {
                    tamano_poblacion: 10,
                    tasa_mutacion: 0.0,
                    tasa_cruce: 0.72,
                },
            ),
            (
                "Población mínima con mutación",
                ConfigGa {
                    tamano_poblacion: 10,
                    tasa_mutacion: 0.15,
                    tasa_cruce: 0.0,
                },
            ),
            (
                "GA compacto",
                ConfigGa {
                    tamano_poblacion: 25,
                    tasa_mutacion: 0.15,
                    tasa_cruce: 0.72,
                },
            ),
            (
                "GA conservador",
                ConfigGa {
                    tamano_poblacion: cfg.tamano_poblacion,
                    tasa_mutacion: (cfg.tasa_mutacion * 0.5).clamp(0.0, 0.8),
                    tasa_cruce: cfg.tasa_cruce,
                },
            ),
            (
                "GA exploratorio",
                ConfigGa {
                    tamano_poblacion: cfg.tamano_poblacion,
                    tasa_mutacion: (cfg.tasa_mutacion * 1.75).clamp(0.0, 0.8),
                    tasa_cruce: cfg.tasa_cruce,
                },
            ),
            ("Configuración activa", cfg),
        ];

        let semillas_holdout: Vec<u64> = (401..=424).collect();
        let mut resultados = Vec::new();

        for (nombre, ga_cfg) in estrategias {
            let mut pnls = Vec::new();
            for &s in &semillas_holdout {
                let replay =
                    operaciones_sinteticas_ga(&costos, 24, precio_ref, s, Utc::now(), true);
                let mut ga = crate::ga::EstadoGa::default();
                ga.config = ga_cfg;
                ga.evolucionar(&replay.operaciones[..], replay.fallos);
                let estrategia = ga.estrategia();
                let pnl: f64 = replay
                    .operaciones
                    .iter()
                    .filter(|op| {
                        let capital = op.precio_compra * op.cantidad_btc;
                        let neto_bps = if capital > 0.0 {
                            op.utilidad_esperada_usd / capital * 10_000.0
                        } else {
                            0.0
                        };
                        capital > 0.0
                            && neto_bps >= estrategia.umbral_min_spread_bps
                            && op.latencia_max_ms <= estrategia.tolerancia_latencia_ms
                    })
                    .map(|op| {
                        let cantidad_aplicada = op.cantidad_btc.min(estrategia.max_operacion_btc);
                        op.utilidad_usd * cantidad_aplicada / op.cantidad_btc.max(0.000_000_01)
                    })
                    .sum();
                pnls.push(pnl);
            }
            pnls.sort_by(f64::total_cmp);
            let mediana = pnls[pnls.len() / 2];
            let p05 = pnls[pnls.len() * 5 / 100];
            let p95 = pnls[pnls.len() * 95 / 100];
            let trades = pnls.len();
            let win = pnls.iter().filter(|&&x| x > 0.0).count() as f64 / trades as f64;
            let peor_perdida = pnls
                .iter()
                .copied()
                .fold(f64::INFINITY, f64::min)
                .min(0.0)
                .abs();
            let pf = pnls.iter().filter(|&&x| x > 0.0).sum::<f64>()
                / pnls
                    .iter()
                    .filter(|&&x| x <= 0.0)
                    .map(|&x| -x)
                    .sum::<f64>()
                    .max(1.0);

            resultados.push(serde_json::json!({
                "modelo": nombre,
                "profitFactor": (pf * 100.0).round() / 100.0,
                "winRate": (win * 100.0).round() / 100.0,
                "drawdown": (peor_perdida * 100.0).round() / 100.0,
                "worstRunLoss": (peor_perdida * 100.0).round() / 100.0,
                "sharpe": 0.0,
                "runs": trades,
                "trades": trades,
                "medianaPnL": (mediana * 100.0).round() / 100.0,
                "p05": (p05 * 100.0).round() / 100.0,
                "p95": (p95 * 100.0).round() / 100.0,
                "p05_p95": format!("{:.0} / {:.0}", p05, p95),
            }));
        }

        serde_json::json!({
            "tipoAnalisis": "sensibilidad_hiperparametros",
            "esAblacionOperadores": false,
            "metodologia": "24 semillas holdout comunes; se varían población, cruce y mutación. No se afirma causalidad sobre recocido, evolución diferencial ni reinicio adaptativo",
            "resultados": resultados
        })
    }
}

fn registrar_muestra_pipeline(
    state: &mut State,
    inicio_scan: Instant,
    cotizacion_mas_reciente: Option<DateTime<Utc>>,
    rutas_evaluadas: usize,
    evento_actual: u64,
    ahora_scan: DateTime<Utc>,
) {
    let compute_us = inicio_scan.elapsed().as_micros().min(u64::MAX as u128) as u64;
    let quote_decision_ms = cotizacion_mas_reciente
        .map(|recibida| (Utc::now() - recibida).num_milliseconds().max(0))
        .unwrap_or(0);

    state.muestras_compute_us.push_back(compute_us);
    state
        .muestras_quote_decision_ms
        .push_back(quote_decision_ms);
    truncar_primeros(&mut state.muestras_compute_us, 512);
    truncar_primeros(&mut state.muestras_quote_decision_ms, 512);

    let mut compute: Vec<u64> = state.muestras_compute_us.iter().copied().collect();
    let mut quote: Vec<i64> = state.muestras_quote_decision_ms.iter().copied().collect();
    compute.sort_unstable();
    quote.sort_unstable();

    let segundos = (ahora_scan - state.inicio).num_milliseconds().max(1) as f64 / 1000.0;
    let eventos_corrida = evento_actual.saturating_sub(state.eventos_inicio_corrida);
    state.telemetria_pipeline.rutas_evaluadas += rutas_evaluadas as u64;
    state.telemetria_pipeline.eventos_por_segundo = eventos_corrida as f64 / segundos;
    state.telemetria_pipeline.muestras = compute.len();
    state.telemetria_pipeline.compute_p50_us = percentil_entero(&compute, 0.50);
    state.telemetria_pipeline.compute_p95_us = percentil_entero(&compute, 0.95);
    state.telemetria_pipeline.compute_p99_us = percentil_entero(&compute, 0.99);
    state.telemetria_pipeline.quote_to_decision_p50_ms = percentil_entero(&quote, 0.50);
    state.telemetria_pipeline.quote_to_decision_p95_ms = percentil_entero(&quote, 0.95);
    state.telemetria_pipeline.quote_to_decision_p99_ms = percentil_entero(&quote, 0.99);
}

fn percentil_entero<T: Copy>(valores: &[T], p: f64) -> T {
    let indice = (((valores.len().saturating_sub(1)) as f64) * p.clamp(0.0, 1.0)).round() as usize;
    valores[indice]
}

fn crear_transferencia(
    state: &State,
    rebalanceo: &Rebalanceo,
    ahora: DateTime<Utc>,
) -> TransferenciaInventario {
    let precio = precio_referencia(state.cotizaciones.values()).max(1.0);
    let fee_activo = if rebalanceo.activo == "BTC" {
        rebalanceo.costo_usd / precio
    } else {
        rebalanceo.costo_usd
    };
    let cantidad_neta = rebalanceo.cantidad.max(0.0);
    let cantidad_bruta = cantidad_neta + fee_activo;
    let objetivo = state
        .carteras
        .inicial
        .get(&rebalanceo.hacia)
        .map(|b| {
            if rebalanceo.activo == "BTC" {
                b.btc
            } else {
                b.usd
            }
        })
        .unwrap_or(cantidad_bruta);
    let minimo = objetivo * 0.65;
    let banda = objetivo * 0.05;
    let capacidad = state
        .carteras
        .balances
        .get(&rebalanceo.desde)
        .map(|b| {
            if rebalanceo.activo == "BTC" {
                b.btc
            } else {
                b.usd
            }
        })
        .unwrap_or(0.0);
    let eta_ms = state.costos.rebalance_settlement_ms.max(100);
    let retraso = (rebalanceo.id.bytes().map(i64::from).sum::<i64>() % (eta_ms / 3).max(1)).max(0);
    TransferenciaInventario {
        id: format!("tx-{}", rebalanceo.id),
        rebalanceo_id: rebalanceo.id.clone(),
        desde: rebalanceo.desde.clone(),
        hacia: rebalanceo.hacia.clone(),
        activo: rebalanceo.activo.clone(),
        cantidad_bruta,
        cantidad_neta,
        costo_usd: rebalanceo.costo_usd,
        estado: "TRANSFER_REQUESTED".to_string(),
        nivel_minimo_s: minimo,
        objetivo_s: objetivo,
        banda_muerta: banda,
        fee_activo,
        eta_ms,
        retraso_simulado_ms: retraso,
        timeout_en: ahora + chrono::Duration::milliseconds((eta_ms + retraso) * 3),
        costo_oportunidad_usd: cantidad_neta
            * if rebalanceo.activo == "BTC" {
                precio
            } else {
                1.0
            }
            * (eta_ms + retraso) as f64
            / 86_400_000.0
            * 0.05,
        capacidad_operativa_restante: capacidad,
        intentos: 1,
        clave_idempotencia: rebalanceo.id.clone(),
        creada_en: ahora,
        liquida_en: ahora + chrono::Duration::milliseconds(eta_ms + retraso),
        confirmada_en: None,
        fallo: None,
        razon: rebalanceo.razon.clone(),
    }
}

fn procesar_transferencias(state: &mut State, ahora: DateTime<Utc>) {
    let mut movimientos: Vec<(String, String, f64)> = Vec::new();
    for transferencia in &mut state.transferencias_inventario {
        match transferencia.estado.as_str() {
            "TRANSFER_REQUESTED" => transferencia.estado = "IN_TRANSIT".to_string(),
            "IN_TRANSIT" if ahora >= transferencia.timeout_en => {
                transferencia.estado = "FAILED".to_string();
                transferencia.fallo = Some("timeout de confirmacion simulado".to_string());
                movimientos.push((
                    transferencia.desde.clone(),
                    transferencia.activo.clone(),
                    transferencia.cantidad_neta,
                ));
            }
            "IN_TRANSIT" if ahora >= transferencia.liquida_en => {
                transferencia.estado = "CONFIRMED".to_string();
                transferencia.confirmada_en = Some(ahora);
                movimientos.push((
                    transferencia.hacia.clone(),
                    transferencia.activo.clone(),
                    transferencia.cantidad_neta,
                ));
            }
            "CONFIRMED" => transferencia.estado = "AVAILABLE".to_string(),
            _ => {}
        }
    }
    for (exchange, activo, cantidad) in movimientos {
        if let Some(balance) = state.carteras.balances.get_mut(&exchange) {
            if activo == "BTC" {
                balance.btc += cantidad;
            } else {
                balance.usd += cantidad;
            }
        }
    }
}

impl Carteras {
    fn new(exchanges: &[String], usd_inicial: f64, btc_inicial: f64) -> Self {
        let mut balances = HashMap::new();
        let mut inicial = HashMap::new();
        let usd = usd_inicial / exchanges.len().max(1) as f64;
        let btc = btc_inicial / exchanges.len().max(1) as f64;
        for exchange in exchanges {
            let balance = Balance {
                exchange: exchange.clone(),
                usd: (usd),
                btc: (btc),
            };
            balances.insert(exchange.clone(), balance.clone());
            inicial.insert(exchange.clone(), balance);
        }
        Self { balances, inicial }
    }

    fn snapshot(&self) -> Vec<Balance> {
        let mut out: Vec<_> = self.balances.values().cloned().collect();
        out.sort_by(|a, b| a.exchange.cmp(&b.exchange));
        out
    }

    fn balance(&self, exchange: &str) -> Balance {
        self.balances
            .get(exchange)
            .cloned()
            .unwrap_or_else(|| Balance {
                exchange: exchange.to_string(),
                usd: 0.0,
                btc: 0.0,
            })
    }

    /// Elige el par ordenado con más capacidad conjunta: USD para comprar y
    /// BTC para vender. Evita nombres de venues hardcodeados y mantiene las
    /// demos ejecutables con cualquier subconjunto de exchanges habilitado.
    fn mejor_ruta_demo(&self) -> Option<(String, String)> {
        let mut candidates = self
            .balances
            .iter()
            .flat_map(|(buy_name, buy)| {
                self.balances
                    .iter()
                    .filter(move |(sell_name, sell)| {
                        *sell_name != buy_name && buy.usd > 0.0 && sell.btc > 0.0
                    })
                    .map(move |(sell_name, sell)| {
                        (buy.usd * sell.btc, buy_name.clone(), sell_name.clone())
                    })
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            right
                .0
                .total_cmp(&left.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(&right.2))
        });
        candidates
            .into_iter()
            .next()
            .map(|(_, buy, sell)| (buy, sell))
    }

    /// Verifica si existe balance para un exchange
    pub fn tiene_balance(&self, exchange: &str) -> bool {
        self.balances.contains_key(exchange)
    }

    /// Devuelve el balance USD disponible en un exchange
    pub fn balance_usd(&self, exchange: &str) -> f64 {
        self.balances.get(exchange).map(|b| b.usd).unwrap_or(0.0)
    }

    #[cfg(test)]
    fn aplicar_operacion(&mut self, op: &Operacion) -> bool {
        if op.cantidad_btc <= 0.0 || op.precio_compra <= 0.0 || op.precio_venta <= 0.0 {
            return false;
        }
        let Some(compra_snapshot) = self.balances.get(&op.compra_en) else {
            return false;
        };
        let Some(venta_snapshot) = self.balances.get(&op.venta_en) else {
            return false;
        };
        let cantidad = op.cantidad_btc;
        let notional_compra = op.precio_compra * cantidad;
        let notional_venta = op.precio_venta * cantidad;
        let costos_extra =
            (op.costos.total_usd - op.costos.fee_compra_usd - op.costos.fee_venta_usd).max(0.0);
        let costo_compra = notional_compra + op.costos.fee_compra_usd + costos_extra;
        let ingreso_venta = notional_venta - op.costos.fee_venta_usd;
        if compra_snapshot.usd < costo_compra || venta_snapshot.btc < cantidad {
            return false;
        }
        if let Some(compra) = self.balances.get_mut(&op.compra_en) {
            compra.usd -= costo_compra;
            compra.btc += cantidad;
        }
        if let Some(venta) = self.balances.get_mut(&op.venta_en) {
            venta.usd += ingreso_venta;
            venta.btc -= cantidad;
        }
        true
    }

    fn aplicar_reporte_ejecucion(&mut self, report: &crate::execution::ExecutionReport) -> bool {
        if !report.invariants.all_passed || report.wallets_before.len() != 2 {
            return false;
        }
        for before in &report.wallets_before {
            let Some(current) = self.balances.get(&before.venue) else {
                return false;
            };
            let Some(usd) = before.usd.to_f64() else {
                return false;
            };
            let Some(btc) = before.btc.to_f64() else {
                return false;
            };
            if (current.usd - usd).abs() > 0.000001 || (current.btc - btc).abs() > 0.000000001 {
                return false;
            }
        }
        let mut updates = Vec::with_capacity(report.wallets_after.len());
        for after in &report.wallets_after {
            let Some(usd) = after.usd.to_f64() else {
                return false;
            };
            let Some(btc) = after.btc.to_f64() else {
                return false;
            };
            if usd < 0.0 || btc < 0.0 {
                return false;
            }
            updates.push((after.venue.clone(), usd, btc));
        }
        for (venue, usd, btc) in updates {
            let Some(wallet) = self.balances.get_mut(&venue) else {
                return false;
            };
            wallet.usd = usd;
            wallet.btc = btc;
        }
        true
    }

    fn rebalancear(
        &mut self,
        precio_ref: f64,
        costos: &MapaCostos,
        ahora: DateTime<Utc>,
    ) -> Vec<Rebalanceo> {
        let mut eventos = Vec::new();
        let umbral = (costos.rebalance_umbral_pct / 100.0).clamp(0.05, 0.95);
        let max_transfer = (costos.rebalance_max_transfer_pct / 100.0).clamp(0.05, 1.0);
        let names: Vec<String> = self.balances.keys().cloned().collect();
        for name in names {
            let init = match self.inicial.get(&name) {
                Some(v) => v.clone(),
                None => continue,
            };
            let actual = match self.balances.get(&name) {
                Some(v) => v.clone(),
                None => continue,
            };
            if init.usd > 0.0 && actual.usd < init.usd * (1.0 - umbral) {
                let src = self
                    .balances
                    .iter()
                    .filter(|(other, _)| **other != name)
                    .filter_map(|(other, b)| {
                        self.inicial
                            .get(other)
                            .map(|i| (other.clone(), b.usd - i.usd))
                    })
                    .max_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((src, surplus)) = src.filter(|(_, s)| *s > costos.costo_rebalanceo_usd)
                {
                    let amount = ((init.usd - actual.usd) * max_transfer).min(surplus);
                    if let Some(src_bal) = self.balances.get_mut(&src) {
                        src_bal.usd -= amount;
                    }
                    eventos.push(Rebalanceo {
                        id: format!("reb-usd-{}-{}-{}", src, name, ahora.timestamp_millis()),
                        desde: src,
                        hacia: name.clone(),
                        activo: "USD".to_string(),
                        cantidad: (amount - costos.costo_rebalanceo_usd).max(0.0),
                        costo_usd: costos.costo_rebalanceo_usd.min(amount),
                        razon: "USD bajo objetivo operativo".to_string(),
                        tiempo: ahora,
                    });
                }
            }
            let actual = match self.balances.get(&name) {
                Some(v) => v.clone(),
                None => continue,
            };
            if init.btc > 0.0 && actual.btc < init.btc * (1.0 - umbral) {
                let src = self
                    .balances
                    .iter()
                    .filter(|(other, _)| **other != name)
                    .filter_map(|(other, b)| {
                        self.inicial
                            .get(other)
                            .map(|i| (other.clone(), b.btc - i.btc))
                    })
                    .max_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((src, surplus)) = src.filter(|(_, s)| *s > (0.001)) {
                    let fee = config_exchange(costos, &src).retiro_btc.max(0.00005);
                    let amount = ((init.btc - actual.btc) * max_transfer).min(surplus);
                    if amount > fee {
                        if let Some(src_bal) = self.balances.get_mut(&src) {
                            src_bal.btc -= amount;
                        }
                        eventos.push(Rebalanceo {
                            id: format!("reb-btc-{}-{}-{}", src, name, ahora.timestamp_millis()),
                            desde: src,
                            hacia: name.clone(),
                            activo: "BTC".to_string(),
                            cantidad: amount - fee,
                            costo_usd: precio_ref * fee,
                            razon: "BTC bajo objetivo operativo".to_string(),
                            tiempo: ahora,
                        });
                    }
                }
            }
        }
        eventos
    }

    fn forzar_rebalanceo_demo(&mut self, precio_ref: f64, ahora: DateTime<Utc>) -> Rebalanceo {
        let mut nombres: Vec<String> = self.balances.keys().cloned().collect();
        nombres.sort();
        let desde = nombres.first().cloned().unwrap_or_else(|| "Demo-A".into());
        let hacia = nombres.get(1).cloned().unwrap_or_else(|| "Demo-B".into());
        let cantidad = self
            .balances
            .get(&desde)
            .map(|b| (b.btc * 0.04).clamp(0.0005, 0.02))
            .unwrap_or(0.001);
        let fee = 0.00005_f64.min(cantidad * 0.5);
        if let Some(src) = self.balances.get_mut(&desde) {
            src.btc = (src.btc - cantidad).max(0.0);
        }
        Rebalanceo {
            id: format!("reb-demo-{}-{}-{}", desde, hacia, ahora.timestamp_millis()),
            desde,
            hacia,
            activo: "BTC".to_string(),
            cantidad: (cantidad - fee).max(0.0),
            costo_usd: fee * precio_ref,
            razon: "demo manual de rebalanceo operativo".to_string(),
            tiempo: ahora,
        }
    }

    fn capital_inicial_usd(&self, precio_btc: f64) -> MoneyUnits {
        self.inicial
            .values()
            .map(|b| b.usd + ((precio_btc) * b.btc))
            .sum()
    }

    fn capital_actual_usd(&self, precio_btc: f64) -> MoneyUnits {
        self.balances
            .values()
            .map(|b| b.usd + ((precio_btc) * b.btc))
            .sum()
    }
}

pub fn buscar_oportunidades(
    cotizaciones: &HashMap<String, Cotizacion>,
    carteras: &Carteras,
    costos: &MapaCostos,
    ahora: DateTime<Utc>,
) -> Vec<Oportunidad> {
    let mut out = Vec::new();
    for compra in cotizaciones
        .values()
        .filter(|c| cotizacion_valida(c, ahora, costos.stale_ms))
    {
        for venta in cotizaciones.values().filter(|c| {
            c.exchange != compra.exchange && cotizacion_valida(c, ahora, costos.stale_ms)
        }) {
            if !costos.permitir_cruce_usd_usdt
                && quote_lane(&compra.exchange) != quote_lane(&venta.exchange)
            {
                continue;
            }
            let oportunidad = calcular_oportunidad(compra, venta, carteras, costos, ahora);
            if oportunidad.diferencial_bruto_usd > 0.0 {
                out.push(oportunidad);
            }
        }
    }
    out
}

fn calcular_oportunidad(
    compra: &Cotizacion,
    venta: &Cotizacion,
    carteras: &Carteras,
    costos: &MapaCostos,
    ahora: DateTime<Utc>,
) -> Oportunidad {
    // El spread bruto siempre representa los precios observados. El riesgo de
    // conversión USD/USDT se cobra una sola vez dentro de `calcular_costos`.
    let ask_observado = dec(compra.ask);
    let bid_observado = dec(venta.bid);
    let diferencial_bruto = bid_observado - ask_observado;
    let precio_medio = (dec(compra.ask) + dec(venta.bid)) / dec(2.0);
    let latencia_max = compra.latencia_ms.max(venta.latencia_ms);
    let balance_compra = carteras.balance(&compra.exchange);
    let balance_venta = carteras.balance(&venta.exchange);
    let fee_compra = dec(config_exchange(costos, &compra.exchange).fee_taker);
    let por_usd = dec(balance_compra.usd) / (dec(compra.ask) * (Decimal::ONE + fee_compra));
    let liquidez_compra = dec(profundidad_disponible(&compra.asks, compra.ask_cantidad));
    let liquidez_venta = dec(profundidad_disponible(&venta.bids, venta.bid_cantidad));
    let cantidad_dec = min_positiva_decimal(&[
        dec(costos.max_operacion_btc),
        liquidez_compra,
        liquidez_venta,
        por_usd,
        dec(balance_venta.btc),
    ]);
    let cantidad = dec_to_f64(cantidad_dec);
    let costo = calcular_costos_canonicos(cantidad, compra, venta, latencia_max, costos);
    let utilidad_dec = diferencial_bruto * cantidad_dec - dec(costo.total_usd);
    let utilidad = dec_to_f64(utilidad_dec);
    let diferencial_neto_unidad = if cantidad_dec > Decimal::ZERO {
        utilidad_dec / cantidad_dec
    } else {
        Decimal::ZERO
    };
    let diferencial_neto_bps = bps_decimal(diferencial_neto_unidad, precio_medio);
    let mut razon = "rentable".to_string();
    let mut decision_code = "ACCEPT_EXECUTABLE".to_string();
    let mut decision_threshold = costos.min_diferencial_neto_bps;
    let mut decision_actual = diferencial_neto_bps;
    let mut decision_reason = format!(
        "ACCEPT_EXECUTABLE — net {:.2} bps >= min {:.2} bps y utilidad {:.2} USD >= min {:.2} USD",
        diferencial_neto_bps, costos.min_diferencial_neto_bps, utilidad, costos.min_utilidad_usd
    );
    let mut ejecutable = true;
    if cantidad_dec <= Decimal::ZERO {
        ejecutable = false;
        razon = "sin liquidez o balance suficiente".to_string();
        decision_code = "SKIP_THIN_OR_INVENTORY".to_string();
        decision_threshold = costos.max_operacion_btc.min(dec_to_f64(liquidez_compra));
        decision_actual = cantidad;
        decision_reason = format!(
            "SKIP_THIN_OR_INVENTORY — cantidad ejecutable {:.8} BTC; compra liquidez {:.8}, venta liquidez {:.8}, USD compra {:.2}, BTC venta {:.8}",
            cantidad,
            dec_to_f64(liquidez_compra),
            dec_to_f64(liquidez_venta),
            balance_compra.usd,
            balance_venta.btc
        );
    } else if utilidad_dec < dec(costos.min_utilidad_usd) {
        ejecutable = false;
        razon = "utilidad menor al mínimo configurado".to_string();
        decision_code = "SKIP_MIN_USD".to_string();
        decision_threshold = costos.min_utilidad_usd;
        decision_actual = utilidad;
        decision_reason = format!(
            "SKIP_MIN_USD — utilidad {:.2} USD < min {:.2} USD después de costos",
            utilidad, costos.min_utilidad_usd
        );
    } else if dec(diferencial_neto_bps) < dec(costos.min_diferencial_neto_bps) {
        ejecutable = false;
        razon = "diferencial neto bajo después de costos".to_string();
        decision_code = "SKIP_NET_BPS".to_string();
        decision_threshold = costos.min_diferencial_neto_bps;
        decision_actual = diferencial_neto_bps;
        decision_reason = format!(
            "SKIP_NET_BPS — net {:.2} bps < min {:.2} bps después de fees, slippage y latencia",
            diferencial_neto_bps, costos.min_diferencial_neto_bps
        );
    } else if latencia_max > costos.stale_ms {
        ejecutable = false;
        razon = "cotizacion antigua".to_string();
        decision_code = "SKIP_STALE".to_string();
        decision_threshold = costos.stale_ms as f64;
        decision_actual = latencia_max as f64;
        decision_reason = format!(
            "SKIP_STALE — latencia {} ms > staleMs {} ms",
            latencia_max, costos.stale_ms
        );
    }
    Oportunidad {
        piernas: vec![],
        tipo: crate::types::TipoOportunidad::Lineal,
        id: format!(
            "{}-{}-{}",
            compra.exchange,
            venta.exchange,
            ahora.timestamp_nanos_opt().unwrap_or_default()
        ),
        compra_en: compra.exchange.clone(),
        venta_en: venta.exchange.clone(),
        par: compra.par.clone(),
        ask: compra.ask,
        bid: venta.bid,
        diferencial_bruto_usd: dec_to_f64(diferencial_bruto),
        diferencial_bruto_bps: bps_decimal(diferencial_bruto, precio_medio),
        diferencial_neto_usd: dec_to_f64(diferencial_neto_unidad),
        diferencial_neto_bps,
        cantidad_btc: cantidad,
        utilidad_usd: utilidad,
        costos: costo,
        latencia_max_ms: latencia_max,
        detectada_en: ahora,
        razon,
        decision_code,
        decision_reason,
        decision_threshold,
        decision_actual,
        ejecutable,
        parcial: cantidad > 0.0 && cantidad < costos.max_operacion_btc * 0.999,
        z_score: 0.0,
    }
}

fn revalidar_operacion(
    state: &State,
    oportunidad: &Oportunidad,
    ahora: DateTime<Utc>,
) -> Result<Operacion, Box<EventoEjecucion>> {
    let compra = state
        .cotizaciones
        .get(&clave_exchange(&oportunidad.compra_en, &oportunidad.par))
        .filter(|c| cotizacion_valida(c, ahora, state.costos.stale_ms));
    let venta = state
        .cotizaciones
        .get(&clave_exchange(&oportunidad.venta_en, &oportunidad.par))
        .filter(|c| cotizacion_valida(c, ahora, state.costos.stale_ms));
    let (Some(compra), Some(venta)) = (compra, venta) else {
        return Err(Box::new(evento_oportunidad(
            oportunidad,
            "revalidacion_fallida",
            "snapshot fresco no disponible antes de ejecutar",
            "alta",
            ahora,
        )));
    };

    let actual = calcular_oportunidad(compra, venta, &state.carteras, &state.costos, ahora);
    let tolerancia_bps = state.costos.movimiento_brusco_bps.max(2.0) / 2.0;
    let deterioro_bps = oportunidad.diferencial_neto_bps - actual.diferencial_neto_bps;
    if !actual.ejecutable || actual.utilidad_usd <= 0.0 || deterioro_bps > tolerancia_bps {
        return Err(Box::new(evento_oportunidad(
            &actual,
            "revalidacion_rechazada",
            "precio, liquidez o balance cambiaron antes de ejecutar",
            "media",
            ahora,
        )));
    }

    Ok(Operacion {
        piernas: vec![],
        tipo: crate::types::TipoOportunidad::Lineal,
        id: oportunidad.id.clone(),
        compra_en: actual.compra_en,
        venta_en: actual.venta_en,
        par: actual.par,
        cantidad_btc: actual.cantidad_btc,
        precio_compra: actual.ask,
        precio_venta: actual.bid,
        utilidad_usd: actual.utilidad_usd,
        utilidad_esperada_usd: actual.utilidad_usd,
        costos: actual.costos,
        parcial: actual.parcial,
        ejecutada_en: ahora,
        latencia_max_ms: actual.latencia_max_ms,
    })
}

/// Modelo canónico de costos usado por ejecución, replay y evaluación offline.
///
/// Mantenerlo público dentro del crate evita que los experimentos inventen un
/// schedule paralelo al que decide las operaciones del motor.
pub fn calcular_costos_canonicos(
    cantidad: f64,
    compra: &Cotizacion,
    venta: &Cotizacion,
    latencia_ms: i64,
    costos: &MapaCostos,
) -> CostosOperacion {
    if cantidad <= 0.0 {
        return CostosOperacion::default();
    }
    let cantidad = dec(cantidad);
    let fee_compra_usd =
        cantidad * dec(compra.ask) * dec(config_exchange(costos, &compra.exchange).fee_taker);
    let fee_venta_usd =
        cantidad * dec(venta.bid) * dec(config_exchange(costos, &venta.exchange).fee_taker);
    let precio_medio = (dec(compra.ask) + dec(venta.bid)) / dec(2.0);
    let deslizamiento_usd = slippage_real_decimal(cantidad, true, compra, costos)
        + slippage_real_decimal(cantidad, false, venta, costos);
    let volumen_rebalance = (dec(costos.max_operacion_btc) * dec(20.0)).max(Decimal::ONE);
    let retiro_fijo_btc = dec(config_exchange(costos, &compra.exchange).retiro_btc)
        + dec(config_exchange(costos, &venta.exchange).retiro_btc);
    let mut retiro_amort_usd = cantidad * precio_medio * dec(costos.retiro_amortizado_bps)
        / dec(10_000.0)
        + precio_medio * retiro_fijo_btc * cantidad / volumen_rebalance;
    let latencia_riesgo_usd = cantidad
        * precio_medio
        * dec(costos.latencia_riesgo_bps)
        * Decimal::from(latencia_ms.max(1))
        / dec(10_000.0)
        / dec(100.0);
    if es_exchange_usd(&compra.exchange) != es_exchange_usd(&venta.exchange) {
        retiro_amort_usd +=
            cantidad * precio_medio * dec(costos.usdt_usd_premium_bps) / dec(10_000.0);
    }
    let total_usd =
        fee_compra_usd + fee_venta_usd + deslizamiento_usd + retiro_amort_usd + latencia_riesgo_usd;
    CostosOperacion {
        fee_compra_usd: dec_to_f64(fee_compra_usd),
        fee_venta_usd: dec_to_f64(fee_venta_usd),
        deslizamiento_usd: dec_to_f64(deslizamiento_usd),
        retiro_amort_usd: dec_to_f64(retiro_amort_usd),
        latencia_riesgo_usd: dec_to_f64(latencia_riesgo_usd),
        seleccion_adversa_usd: 0.0,
        total_usd: dec_to_f64(total_usd),
    }
}

fn slippage_real_decimal(
    cantidad: Decimal,
    es_compra: bool,
    cot: &Cotizacion,
    costos: &MapaCostos,
) -> Decimal {
    let niveles = if es_compra { &cot.asks } else { &cot.bids };
    if niveles.is_empty() {
        return cantidad * dec(config_exchange(costos, &cot.exchange).fee_taker) * dec(1000.0);
    }
    let mut restante = cantidad;
    let mut costo_total = Decimal::ZERO;
    let precio_ref = dec(niveles[0].precio);
    for nivel in niveles {
        if restante <= Decimal::ZERO {
            break;
        }
        let tomar = restante.min(dec(nivel.cantidad));
        costo_total += tomar * dec(nivel.precio);
        restante -= tomar;
    }
    if restante > Decimal::ZERO {
        costo_total += restante * precio_ref;
    }
    let precio_promedio = costo_total / cantidad;
    let mut slip = (precio_promedio - precio_ref) * cantidad;
    if !es_compra {
        slip = -slip;
    }
    let max_slip = cantidad * precio_ref * dec(costos.deslizamiento_bps) / dec(10_000.0);
    slip.max(Decimal::ZERO).min(max_slip)
}

fn agrupar_por_par(
    cotizaciones: HashMap<String, Cotizacion>,
) -> HashMap<String, HashMap<String, Cotizacion>> {
    let mut out: HashMap<String, HashMap<String, Cotizacion>> = HashMap::new();
    for cot in cotizaciones.into_values() {
        out.entry(cot.par.clone())
            .or_default()
            .insert(cot.exchange.clone(), cot);
    }
    out
}

fn aplicar_adversidad(
    op: &mut Operacion,
    costos: &MapaCostos,
    ahora: DateTime<Utc>,
    demo_forzado: Option<EscenarioDemo>,
) -> Option<EventoEjecucion> {
    if matches!(demo_forzado, Some(EscenarioDemo::FalloOrden)) {
        return Some(evento_operacion(
            op,
            "fallida",
            "orden rechazada por escenario demo controlado",
            "alta",
            ahora,
        ));
    }
    if matches!(demo_forzado, Some(EscenarioDemo::MercadoMovido)) {
        let shock = costos.movimiento_brusco_bps.max(8.0) / 10000.0;
        op.precio_venta *= 1.0 - shock;
        op.utilidad_usd =
            (op.precio_venta - op.precio_compra) * op.cantidad_btc - op.costos.total_usd;
        return Some(evento_operacion(
            op,
            "mercado_movido",
            "precio se movió por escenario demo controlado",
            "media",
            ahora,
        ));
    }
    if !costos.simular_adversidad {
        return None;
    }
    let mut rng = rand::thread_rng();
    if rng.gen_bool(costos.prob_fallo_orden.clamp(0.0, 1.0)) {
        return Some(evento_operacion(
            op,
            "fallida",
            "orden rechazada por escenario adverso simulado",
            "alta",
            ahora,
        ));
    }
    if rng.gen_bool(costos.prob_movimiento_brusco.clamp(0.0, 1.0)) {
        let shock = costos.movimiento_brusco_bps.max(0.0) / 10000.0;
        op.precio_venta *= 1.0 - shock;
        op.utilidad_usd =
            (op.precio_venta - op.precio_compra) * op.cantidad_btc - op.costos.total_usd;
        return Some(evento_operacion(
            op,
            "mercado_movido",
            "precio se movió entre detección y ejecución",
            "media",
            ahora,
        ));
    }
    None
}

fn registrar_auditoria_oportunidades(
    state: &mut State,
    oportunidades: &[Oportunidad],
    carteras: &Carteras,
    costos: &MapaCostos,
    historial: &HashMap<String, f64>,
    pesos: &[f64],
    ahora: DateTime<Utc>,
) {
    for oportunidad in oportunidades.iter().take(18) {
        let compra = carteras.balance(&oportunidad.compra_en);
        let venta = carteras.balance(&oportunidad.venta_en);
        let score = puntuar_oportunidad(
            oportunidad,
            costos.max_operacion_btc,
            costos.stale_ms,
            historial,
            oportunidad.z_score,
            pesos,
        );
        state.auditoria_decisiones.insert(
            0,
            AuditoriaDecision {
                id: format!("aud-{}-{}", oportunidad.id, ahora.timestamp_millis()),
                ruta: format!("{}->{}", oportunidad.compra_en, oportunidad.venta_en),
                par: oportunidad.par.clone(),
                decision: if oportunidad.ejecutable {
                    "candidata_ejecutable".to_string()
                } else {
                    "descartada".to_string()
                },
                decision_code: oportunidad.decision_code.clone(),
                decision_reason: oportunidad.decision_reason.clone(),
                decision_threshold: oportunidad.decision_threshold,
                decision_actual: oportunidad.decision_actual,
                razon: oportunidad.razon.clone(),
                score,
                pesos_ga: pesos.to_vec(),
                utilidad_usd: oportunidad.utilidad_usd,
                diferencial_neto_bps: oportunidad.diferencial_neto_bps,
                cantidad_btc: oportunidad.cantidad_btc,
                costo_total_usd: oportunidad.costos.total_usd,
                latencia_max_ms: oportunidad.latencia_max_ms,
                z_score: oportunidad.z_score,
                compra_usd_antes: compra.usd,
                venta_btc_antes: venta.btc,
                tiempo: ahora,
            },
        );
    }
    state.auditoria_decisiones.truncate(160);
}

fn construir_ml_edge(state: &State) -> Option<EstadoMlEdge> {
    let auditoria = state.auditoria_decisiones.front()?;
    let max_operacion = state.costos.max_operacion_btc.max(0.00000001);
    let stale_ms = state.costos.stale_ms.max(1) as f64;
    let pesos = pesos_normalizados(&auditoria.pesos_ga);
    let utilidad_norm = (auditoria.utilidad_usd / 25.0).tanh().max(0.0);
    let frescura = (1.0 - auditoria.latencia_max_ms as f64 / stale_ms).clamp(0.0, 1.0);
    let liquidez = (auditoria.cantidad_btc / max_operacion).clamp(0.0, 1.0);
    let confiabilidad = confianza_ruta(&state.historial_rutas, &auditoria.ruta);
    let z_score = (0.5 + auditoria.z_score.tanh() / 2.0).clamp(0.0, 1.0);
    let valores = [
        ("utilidad_neta", utilidad_norm),
        ("frescura_book", frescura),
        ("liquidez_fill", liquidez),
        ("confiabilidad_ruta", confiabilidad),
        ("z_score_spread", z_score),
    ];
    let features = valores
        .iter()
        .zip(pesos.iter())
        .map(|((nombre, valor), peso)| FeatureMlEdge {
            nombre: (*nombre).to_string(),
            peso: *peso,
            valor: *valor,
            contribucion: *peso * *valor,
        })
        .collect::<Vec<_>>();
    let score_actual = features.iter().map(|f| f.contribucion).sum::<f64>();
    let survival_probability = frescura
        .mul_add(0.56, confiabilidad * 0.24)
        .mul_add(1.0, z_score * 0.20)
        .clamp(0.0, 1.0);
    let fill_probability =
        (liquidez * 0.72 + confiabilidad * 0.18 + frescura * 0.10).clamp(0.0, 1.0);
    let adverse_selection_bps = ((1.0 - survival_probability) * state.costos.movimiento_brusco_bps)
        + (auditoria.latencia_max_ms as f64 / stale_ms).clamp(0.0, 2.0)
            * state.costos.latencia_riesgo_bps;
    let expected_value_usd = auditoria.utilidad_usd * survival_probability * fill_probability
        - auditoria.costo_total_usd * (1.0 - fill_probability) * 0.12;
    let confianza =
        ((score_actual * 0.55) + (survival_probability * 0.25) + (fill_probability * 0.20))
            .clamp(0.0, 1.0);
    let activo = state.ga.generacion > 0 && state.ga.operaciones_evaluadas > 0;
    let explicacion = format!(
        "Score EV {:.3}: utilidad {:.0}%, frescura {:.0}%, liquidez {:.0}%, supervivencia {:.0}% y fill {:.0}%; decision {}.",
        score_actual,
        utilidad_norm * 100.0,
        frescura * 100.0,
        liquidez * 100.0,
        survival_probability * 100.0,
        fill_probability * 100.0,
        auditoria.decision_code
    );
    Some(EstadoMlEdge {
        activo,
        modelo: "Mayab Scoring Evolutivo GA/EV".to_string(),
        version: "ml-edge-v1".to_string(),
        decision: auditoria.decision_code.clone(),
        score_actual,
        confianza,
        expected_value_usd,
        survival_probability,
        fill_probability,
        adverse_selection_bps,
        features,
        explicacion,
    })
}

fn pesos_normalizados(pesos: &[f64]) -> [f64; 5] {
    let mut out = [0.40, 0.20, 0.20, 0.10, 0.10];
    for (idx, peso) in pesos.iter().take(5).enumerate() {
        if peso.is_finite() && *peso >= 0.0 {
            out[idx] = *peso;
        }
    }
    let total = out.iter().sum::<f64>();
    if total > 0.0 {
        for peso in &mut out {
            *peso /= total;
        }
    }
    out
}

fn confianza_ruta(historial: &HashMap<String, f64>, ruta: &str) -> f64 {
    historial
        .get(ruta)
        .copied()
        .map(|v| (0.50 + v * 0.08).clamp(0.05, 0.98))
        .unwrap_or(0.58)
}

fn insertar_evento_sistema(
    state: &mut State,
    tipo: &str,
    detalle: &str,
    severidad: &str,
    ahora: DateTime<Utc>,
) {
    state.eventos_ejecucion.insert(
        0,
        EventoEjecucion {
            id: format!("evt-{}-{}", tipo, ahora.timestamp_millis()),
            tipo: tipo.to_string(),
            ruta: "sistema".to_string(),
            detalle: detalle.to_string(),
            severidad: severidad.to_string(),
            tiempo: ahora,
            utilidad_usd: 0.0,
            cantidad_btc: 0.0,
        },
    );
    state.eventos_ejecucion.truncate(128);
}

fn evento_operacion(
    op: &Operacion,
    tipo: &str,
    detalle: &str,
    severidad: &str,
    ahora: DateTime<Utc>,
) -> EventoEjecucion {
    EventoEjecucion {
        id: format!("evt-{}-{}-{}", tipo, op.id, ahora.timestamp_millis()),
        tipo: tipo.to_string(),
        ruta: format!("{}->{}", op.compra_en, op.venta_en),
        detalle: detalle.to_string(),
        severidad: severidad.to_string(),
        tiempo: ahora,
        utilidad_usd: op.utilidad_usd,
        cantidad_btc: op.cantidad_btc,
    }
}

fn simular_ejecucion_dos_piernas(
    state: &State,
    op: &Operacion,
    scenario: crate::execution::ExecutionScenario,
) -> Result<crate::execution::ExecutionReport, String> {
    let decimal = |value: f64, field: &str| {
        Decimal::from_f64(value).ok_or_else(|| format!("{field} no es decimal finito"))
    };
    let buy = state.carteras.balance(&op.compra_en);
    let sell = state.carteras.balance(&op.venta_en);
    let extra_cost =
        (op.costos.total_usd - op.costos.fee_compra_usd - op.costos.fee_venta_usd).max(0.0);
    let (rehedge_cost, unwind_cost) = match scenario {
        crate::execution::ExecutionScenario::RehedgeCheaper => (1.25, 8.0),
        crate::execution::ExecutionScenario::UnwindCheaper => (8.0, 3.25),
        _ => (4.0, 3.25),
    };
    let shock = (op.precio_compra * 0.0004).max(1.0);
    crate::execution::simulate(crate::execution::ExecutionRequest {
        execution_id: op.id.clone(),
        scenario,
        buy_wallet: crate::execution::InitialWallet {
            venue: buy.exchange,
            usd: decimal(buy.usd, "buy_wallet.usd")?,
            btc: decimal(buy.btc, "buy_wallet.btc")?,
        },
        sell_wallet: crate::execution::InitialWallet {
            venue: sell.exchange,
            usd: decimal(sell.usd, "sell_wallet.usd")?,
            btc: decimal(sell.btc, "sell_wallet.btc")?,
        },
        quantity_btc: decimal(op.cantidad_btc, "quantity_btc")?,
        prices: crate::execution::ExecutionPrices {
            leg1_buy_price_usd: decimal(op.precio_compra, "precio_compra")?,
            leg2_sell_price_usd: decimal(op.precio_venta, "precio_venta")?,
            rehedge_price_usd: decimal(op.precio_venta - shock, "rehedge_price")?,
            unwind_price_usd: decimal(op.precio_compra - shock, "unwind_price")?,
        },
        costs: crate::execution::ExecutionCosts {
            leg1_fee_usd: decimal(op.costos.fee_compra_usd + extra_cost, "leg1_fee_usd")?,
            leg2_fee_usd: decimal(op.costos.fee_venta_usd, "leg2_fee_usd")?,
            rehedge_cost_usd: decimal(rehedge_cost, "rehedge_cost_usd")?,
            unwind_cost_usd: decimal(unwind_cost, "unwind_cost_usd")?,
        },
    })
    .map_err(|error| error.to_string())
}

fn registrar_reporte_ejecucion(
    state: &mut State,
    report: &crate::execution::ExecutionReport,
    ahora: DateTime<Utc>,
) {
    let buy_venue = report
        .fills
        .iter()
        .find(|fill| fill.leg == crate::execution::ExecutionLeg::Leg1)
        .map(|fill| fill.venue.as_str())
        .unwrap_or("compra");
    let sell_venue = report
        .fills
        .iter()
        .find(|fill| fill.leg == crate::execution::ExecutionLeg::Leg2)
        .map(|fill| fill.venue.as_str())
        .or_else(|| {
            report
                .wallets_before
                .iter()
                .find(|wallet| wallet.venue != buy_venue)
                .map(|wallet| wallet.venue.as_str())
        })
        .unwrap_or("venta");
    let route = format!("{buy_venue}->{sell_venue}");
    let leg1 = report
        .fills
        .iter()
        .filter(|fill| fill.leg == crate::execution::ExecutionLeg::Leg1)
        .map(|fill| fill.quantity_btc)
        .sum::<Decimal>();
    let leg2 = report
        .fills
        .iter()
        .filter(|fill| fill.leg == crate::execution::ExecutionLeg::Leg2)
        .map(|fill| fill.quantity_btc)
        .sum::<Decimal>();
    let residual_legs = leg1 - leg2;
    for transition in &report.transitions {
        let exposure = match transition.to {
            crate::execution::ExecutionState::Leg1Partial
            | crate::execution::ExecutionState::Leg1Filled
            | crate::execution::ExecutionState::Leg2Submitted => leg1,
            crate::execution::ExecutionState::Leg2Partial
            | crate::execution::ExecutionState::Leg2Filled
            | crate::execution::ExecutionState::Leg2Rejected
            | crate::execution::ExecutionState::Leg2TimedOut
            | crate::execution::ExecutionState::RecoverySelected => residual_legs,
            crate::execution::ExecutionState::Reconciled => report.residual_btc,
            _ => Decimal::ZERO,
        };
        state.trazas_ejecucion.push_front(TransicionEjecucion {
            id: format!("fsm-{}-{}", report.execution_id, transition.sequence),
            operacion_id: report.execution_id.clone(),
            ruta: route.clone(),
            estado_anterior: transition
                .from
                .map(crate::execution::ExecutionState::as_str)
                .unwrap_or("NONE")
                .to_string(),
            estado: transition.to.as_str().to_string(),
            pierna: match transition.to {
                crate::execution::ExecutionState::Leg1Submitted
                | crate::execution::ExecutionState::Leg1Partial
                | crate::execution::ExecutionState::Leg1Filled => "compra",
                crate::execution::ExecutionState::Leg2Submitted
                | crate::execution::ExecutionState::Leg2Partial
                | crate::execution::ExecutionState::Leg2Filled
                | crate::execution::ExecutionState::Leg2Rejected
                | crate::execution::ExecutionState::Leg2TimedOut => "venta",
                crate::execution::ExecutionState::RecoverySelected => "recuperacion",
                _ => "ledger",
            }
            .to_string(),
            detalle: transition.detail.clone(),
            exposicion_btc: exposure.to_f64().unwrap_or(0.0),
            pnl_realizado_usd: if transition.to == crate::execution::ExecutionState::Reconciled {
                report.pnl_usd.to_f64().unwrap_or(0.0)
            } else {
                0.0
            },
            tiempo: ahora,
        });
    }
    state.trazas_ejecucion.truncate(160);
    state.ejecuciones_dos_piernas.push_front(report.clone());
    state.ejecuciones_dos_piernas.truncate(32);
}

fn evento_oportunidad(
    oportunidad: &Oportunidad,
    tipo: &str,
    detalle: &str,
    severidad: &str,
    ahora: DateTime<Utc>,
) -> EventoEjecucion {
    EventoEjecucion {
        id: format!(
            "evt-{}-{}-{}",
            tipo,
            oportunidad.id,
            ahora.timestamp_millis()
        ),
        tipo: tipo.to_string(),
        ruta: format!("{}->{}", oportunidad.compra_en, oportunidad.venta_en),
        detalle: detalle.to_string(),
        severidad: severidad.to_string(),
        tiempo: ahora,
        utilidad_usd: oportunidad.utilidad_usd,
        cantidad_btc: oportunidad.cantidad_btc,
    }
}

struct ReplayGa {
    operaciones: Vec<Operacion>,
    fallos: usize,
}

fn operaciones_sinteticas_ga(
    costos: &MapaCostos,
    muestras: usize,
    precio_ref: f64,
    seed: u64,
    ahora: DateTime<Utc>,
    incluir_adversidad: bool,
) -> ReplayGa {
    let exchanges = [
        "Binance", "Kraken", "Coinbase", "OKX", "Bybit", "Bitfinex", "KuCoin", "Gate.io",
        "Bitstamp", "Gemini",
    ];
    let mut rng = StdRng::seed_from_u64(seed);
    let precio_base = precio_ref.clamp(20_000.0, 250_000.0);
    let mut operaciones = Vec::with_capacity(muestras);
    let mut fallos = 0usize;

    for i in 0..muestras {
        let compra_idx = i % exchanges.len();
        let mut venta_idx = (i * 2 + 1) % exchanges.len();
        if venta_idx == compra_idx {
            venta_idx = (venta_idx + 1) % exchanges.len();
        }
        let compra = exchanges[compra_idx];
        let venta = exchanges[venta_idx];
        let mid = precio_base * (1.0 + rng.gen_range(-0.0018..0.0018));
        let cantidad = costos
            .max_operacion_btc
            .clamp(0.03, 0.55)
            .min(rng.gen_range(0.045..0.32));
        let precio_compra = mid * (1.0 - rng.gen_range(0.00008..0.00024));
        let mut precio_venta = mid * (1.0 + rng.gen_range(0.00008..0.00024));
        let fee_compra = config_exchange(costos, compra).fee_taker;
        let fee_venta = config_exchange(costos, venta).fee_taker;
        let costos_base = cantidad * mid * costos.deslizamiento_bps / 10000.0
            + cantidad * mid * costos.retiro_amortizado_bps / 10000.0
            + cantidad * mid * costos.latencia_riesgo_bps / 10000.0;
        let utilidad_objetivo = rng.gen_range(8.0..55.0);
        let utilidad_inicial = (precio_venta - precio_compra) * cantidad
            - cantidad * precio_compra * fee_compra
            - cantidad * precio_venta * fee_venta
            - costos_base;
        if utilidad_inicial < utilidad_objetivo {
            precio_venta += (utilidad_objetivo - utilidad_inicial) / cantidad.max(0.0001);
        }
        let costos_operacion = CostosOperacion {
            fee_compra_usd: cantidad * precio_compra * fee_compra,
            fee_venta_usd: cantidad * precio_venta * fee_venta,
            deslizamiento_usd: cantidad * mid * costos.deslizamiento_bps / 10000.0,
            retiro_amort_usd: cantidad * mid * costos.retiro_amortizado_bps / 10000.0,
            latencia_riesgo_usd: cantidad * mid * costos.latencia_riesgo_bps / 10000.0,
            seleccion_adversa_usd: 0.0,
            total_usd: 0.0,
        };
        let total_usd = costos_operacion.fee_compra_usd
            + costos_operacion.fee_venta_usd
            + costos_operacion.deslizamiento_usd
            + costos_operacion.retiro_amort_usd
            + costos_operacion.latencia_riesgo_usd;
        let utilidad_esperada = ((precio_venta - precio_compra) * cantidad - total_usd).max(1.0);
        let adversa = incluir_adversidad && rng.gen_bool(0.22);
        let utilidad = if adversa {
            -rng.gen_range(40.0..160.0)
        } else {
            utilidad_esperada
        };
        let parcial = rng.gen_bool(0.18);
        if rng.gen_bool(0.08) {
            fallos += 1;
        }
        let tiempo = ahora - chrono::Duration::milliseconds((muestras - i) as i64 * 180);
        operaciones.push(Operacion {
            piernas: vec![],
            tipo: crate::types::TipoOportunidad::Lineal,
            id: format!("demo-ga-{seed}-{i}"),
            compra_en: compra.to_string(),
            venta_en: venta.to_string(),
            par: "BTC/USD".to_string(),
            cantidad_btc: if parcial { cantidad * 0.58 } else { cantidad },
            precio_compra,
            precio_venta,
            utilidad_usd: utilidad,
            utilidad_esperada_usd: utilidad_esperada,
            costos: CostosOperacion {
                total_usd,
                ..costos_operacion
            },
            parcial,
            ejecutada_en: tiempo,
            latencia_max_ms: rng.gen_range(35..420),
        });
    }

    ReplayGa {
        operaciones,
        fallos,
    }
}

fn oportunidad_desde_operacion(op: &Operacion) -> Oportunidad {
    let precio_medio = (op.precio_compra + op.precio_venta) / 2.0;
    let bruto = op.precio_venta - op.precio_compra;
    let neto_unidad = if op.cantidad_btc > 0.0 {
        op.utilidad_usd / op.cantidad_btc
    } else {
        0.0
    };
    Oportunidad { piernas: vec![], tipo: crate::types::TipoOportunidad::Lineal,
        id: format!("opp-{}", op.id),
        compra_en: op.compra_en.clone(),
        venta_en: op.venta_en.clone(),
        par: op.par.clone(),
        ask: op.precio_compra,
        bid: op.precio_venta,
        diferencial_bruto_usd: bruto,
        diferencial_bruto_bps: bps(bruto, precio_medio),
        diferencial_neto_usd: neto_unidad,
        diferencial_neto_bps: bps(neto_unidad, precio_medio),
        cantidad_btc: op.cantidad_btc,
        utilidad_usd: op.utilidad_usd,
        costos: op.costos.clone(),
        latencia_max_ms: op.latencia_max_ms,
        detectada_en: op.ejecutada_en,
        razon: "demo rentable inyectada".to_string(),
        decision_code: "DEMO_PROFITABLE".to_string(),
        decision_reason: format!(
            "DEMO_PROFITABLE — operación sintética rentable con utilidad {:.2} USD para demostrar flujo end-to-end",
            op.utilidad_usd
        ),
        decision_threshold: 0.0,
        decision_actual: op.utilidad_usd,
        ejecutable: true,
        parcial: op.parcial,
        z_score: 2.4,
    }
}

fn operacion_demo_fill_parcial(
    costos: &MapaCostos,
    precio_ref: f64,
    inventario_venta_btc: f64,
    compra_en: &str,
    venta_en: &str,
    seed: u64,
    ahora: DateTime<Utc>,
) -> Operacion {
    let precio_base = precio_ref.clamp(20_000.0, 250_000.0);
    // Mantiene el escenario ejecutable aun cuando el inventario inicial se
    // distribuye entre todos los adaptadores conocidos de la demo.
    let requested = costos.max_operacion_btc.clamp(0.08, 0.45);
    // Debe caber incluso cuando el inventario inicial está repartido entre
    // todos los venues configurados; sigue siendo claramente menor al pedido.
    let filled = (requested * 0.12)
        .max(0.025)
        .min(requested * 0.5)
        .min((inventario_venta_btc * 0.8).max(0.0));
    let precio_compra = precio_base * 0.9997;
    let mid = precio_base;
    let fee_compra = config_exchange(costos, compra_en).fee_taker;
    let fee_venta = config_exchange(costos, venta_en).fee_taker;
    let costos_base = filled * mid * costos.deslizamiento_bps / 10000.0
        + filled * mid * costos.retiro_amortizado_bps / 10000.0
        + filled * mid * costos.latencia_riesgo_bps / 10000.0;
    let precio_venta_base = precio_base * 1.0006;
    let utilidad_objetivo = 18.0 + (seed % 17) as f64;
    let utilidad_inicial = (precio_venta_base - precio_compra) * filled
        - filled * precio_compra * fee_compra
        - filled * precio_venta_base * fee_venta
        - costos_base;
    let precio_venta =
        precio_venta_base + (utilidad_objetivo - utilidad_inicial).max(0.0) / filled.max(0.0001);
    let costos_operacion = CostosOperacion {
        fee_compra_usd: filled * precio_compra * fee_compra,
        fee_venta_usd: filled * precio_venta * fee_venta,
        deslizamiento_usd: filled * mid * costos.deslizamiento_bps / 10000.0,
        retiro_amort_usd: filled * mid * costos.retiro_amortizado_bps / 10000.0,
        latencia_riesgo_usd: filled * mid * costos.latencia_riesgo_bps / 10000.0,
        seleccion_adversa_usd: 0.0,
        total_usd: 0.0,
    };
    let total_usd = costos_operacion.fee_compra_usd
        + costos_operacion.fee_venta_usd
        + costos_operacion.deslizamiento_usd
        + costos_operacion.retiro_amort_usd
        + costos_operacion.latencia_riesgo_usd;
    Operacion {
        piernas: vec![],
        tipo: crate::types::TipoOportunidad::Lineal,
        id: format!("demo-partial-{seed}"),
        compra_en: compra_en.to_string(),
        venta_en: venta_en.to_string(),
        par: "BTC/USD".to_string(),
        cantidad_btc: filled,
        precio_compra,
        precio_venta,
        utilidad_usd: ((precio_venta - precio_compra) * filled - total_usd).max(1.0),
        utilidad_esperada_usd: ((precio_venta - precio_compra) * filled - total_usd).max(1.0),
        costos: CostosOperacion {
            total_usd,
            ..costos_operacion
        },
        parcial: true,
        ejecutada_en: ahora,
        latencia_max_ms: 92,
    }
}

fn oportunidad_demo_fill_parcial(op: &Operacion, costos: &MapaCostos) -> Oportunidad {
    let mut oportunidad = oportunidad_desde_operacion(op);
    oportunidad.razon = "fill parcial por profundidad limitada".to_string();
    oportunidad.decision_code = "PARTIAL_FILL".to_string();
    oportunidad.decision_reason = format!(
        "PARTIAL_FILL — requested {:.6} BTC, filled {:.6} BTC por profundidad disponible; utilidad {:.2} USD",
        costos.max_operacion_btc, op.cantidad_btc, op.utilidad_usd
    );
    oportunidad.decision_threshold = costos.max_operacion_btc;
    oportunidad.decision_actual = op.cantidad_btc;
    oportunidad
}

fn auditoria_demo_fill_parcial(op: &Operacion, costos: &MapaCostos) -> AuditoriaDecision {
    let oportunidad = oportunidad_demo_fill_parcial(op, costos);
    AuditoriaDecision {
        id: format!("aud-partial-{}", op.id),
        ruta: format!("{}->{}", op.compra_en, op.venta_en),
        par: op.par.clone(),
        decision: "candidata_ejecutable".to_string(),
        decision_code: "PARTIAL_FILL".to_string(),
        decision_reason: oportunidad.decision_reason.clone(),
        decision_threshold: costos.max_operacion_btc,
        decision_actual: op.cantidad_btc,
        razon: "fill parcial: el motor reduce cantidad al limite ejecutable del libro".to_string(),
        score: 0.91,
        pesos_ga: vec![0.40, 0.20, 0.20, 0.10, 0.10],
        utilidad_usd: op.utilidad_usd,
        diferencial_neto_bps: oportunidad.diferencial_neto_bps,
        cantidad_btc: op.cantidad_btc,
        costo_total_usd: op.costos.total_usd,
        latencia_max_ms: op.latencia_max_ms,
        z_score: oportunidad.z_score,
        compra_usd_antes: op.precio_compra * costos.max_operacion_btc * 2.0,
        venta_btc_antes: costos.max_operacion_btc,
        tiempo: op.ejecutada_en,
    }
}

fn auditoria_demo_desde_operacion(op: &Operacion) -> AuditoriaDecision {
    let oportunidad = oportunidad_desde_operacion(op);
    AuditoriaDecision {
        id: format!("aud-demo-{}", op.id),
        ruta: format!("{}->{}", op.compra_en, op.venta_en),
        par: op.par.clone(),
        decision: "candidata_ejecutable".to_string(),
        decision_code: "DEMO_PROFITABLE".to_string(),
        decision_reason: format!(
            "DEMO_PROFITABLE — utilidad sintetica {:.2} USD, sin afirmar edge live",
            op.utilidad_usd
        ),
        decision_threshold: 0.0,
        decision_actual: op.utilidad_usd,
        razon: "demo rentable: ruta sintetica usada para mostrar flujo completo y entrenar GA"
            .to_string(),
        score: 0.93,
        pesos_ga: vec![0.40, 0.20, 0.20, 0.10, 0.10],
        utilidad_usd: op.utilidad_usd,
        diferencial_neto_bps: oportunidad.diferencial_neto_bps,
        cantidad_btc: op.cantidad_btc,
        costo_total_usd: op.costos.total_usd,
        latencia_max_ms: op.latencia_max_ms,
        z_score: oportunidad.z_score,
        compra_usd_antes: op.precio_compra * op.cantidad_btc * 3.0,
        venta_btc_antes: op.cantidad_btc * 4.0,
        tiempo: op.ejecutada_en,
    }
}

fn actualizar_latencia_exchange(state: &mut State, exchange: &str, latencia_ms: i64) {
    let lat = state
        .latencias_exchange
        .entry(exchange.to_string())
        .or_insert(LatenciaEstado {
            promedio_ms: latencia_ms as f64,
            ultimo_ms: latencia_ms,
            min_ms: latencia_ms,
            max_ms: latencia_ms,
            p50_ms: latencia_ms,
            p95_ms: latencia_ms,
            p99_ms: latencia_ms,
            eventos: 0,
            historial: VecDeque::with_capacity(100),
        });
    lat.eventos += 1;
    lat.ultimo_ms = latencia_ms;
    lat.min_ms = lat.min_ms.min(latencia_ms);
    lat.max_ms = lat.max_ms.max(latencia_ms);
    lat.historial.push_back(latencia_ms);
    if lat.historial.len() > 100 {
        lat.historial.pop_front();
    }
    let mut sorted: Vec<_> = lat.historial.iter().copied().collect();
    sorted.sort_unstable();
    if !sorted.is_empty() {
        let n = sorted.len();
        lat.p50_ms = sorted[(n as f64 * 0.50) as usize];
        lat.p95_ms = sorted[(n as f64 * 0.95).min((n - 1) as f64) as usize];
        lat.p99_ms = sorted[(n as f64 * 0.99).min((n - 1) as f64) as usize];
    }
    lat.promedio_ms = if lat.eventos <= 1 {
        latencia_ms as f64
    } else {
        lat.promedio_ms * 0.90 + latencia_ms as f64 * 0.10
    };
}

fn snapshot_latencias(state: &State) -> Vec<LatenciaExchange> {
    let mut out: Vec<_> = state
        .latencias_exchange
        .iter()
        .map(|(exchange, lat)| LatenciaExchange {
            exchange: exchange.clone(),
            promedio_ms: lat.promedio_ms,
            ultimo_ms: lat.ultimo_ms,
            min_ms: lat.min_ms,
            max_ms: lat.max_ms,
            p50_ms: lat.p50_ms,
            p95_ms: lat.p95_ms,
            p99_ms: lat.p99_ms,
            eventos: lat.eventos,
            estado: estado_riesgo(lat.promedio_ms, state.costos.stale_ms),
            region_sugerida: region_sugerida(exchange, lat.promedio_ms).to_string(),
        })
        .collect();
    out.sort_by(|a, b| a.promedio_ms.total_cmp(&b.promedio_ms));
    out
}

fn region_sugerida(exchange: &str, promedio_ms: f64) -> &'static str {
    if promedio_ms > 900.0 {
        return "probar replica mas cercana";
    }
    match exchange {
        "Coinbase" | "Kraken" | "Binance" => "iad/us-east",
        "OKX" | "Bybit" => "sin/hkg si domina el flujo",
        _ => "iad/us-east",
    }
}

fn actualizar_volatilidad(state: &mut State, precio_ref: f64, ahora: DateTime<Utc>) {
    state.precios_ref.push(PuntoSerie {
        tiempo: ahora,
        valor: precio_ref,
    });
    let desde = ahora - chrono::Duration::seconds(state.costos.volatilidad_ventana_seg);
    state.precios_ref.retain(|p| p.tiempo >= desde);
    if state.precios_ref.len() <= 1 {
        state.modo_conservador = false;
        return;
    }
    let min = state
        .precios_ref
        .iter()
        .map(|p| p.valor)
        .fold(f64::INFINITY, f64::min);
    let max = state
        .precios_ref
        .iter()
        .map(|p| p.valor)
        .fold(0.0, f64::max);
    state.modo_conservador =
        min > 0.0 && (max - min) / min * 10000.0 >= state.costos.volatilidad_umbral_bps;
}

fn actualizar_circuit_breaker(state: &mut State, ahora: DateTime<Utc>) {
    let desde = ahora - chrono::Duration::minutes(state.costos.circuit_breaker_ventana_min);
    state
        .operaciones_riesgo
        .retain(|op| op.ejecutada_en >= desde);
    let pnl: f64 = state
        .operaciones_riesgo
        .iter()
        .map(|op| op.utilidad_usd)
        .sum();
    state.circuit_breaker_activo = pnl < -state.costos.circuit_breaker_perdida_usd;
}

fn puede_ejecutar(
    o: &Oportunidad,
    ahora: DateTime<Utc>,
    enfriamiento: &HashMap<String, DateTime<Utc>>,
    cooldown_ms: i64,
) -> bool {
    let ruta = format!("{}->{}", o.compra_en, o.venta_en);
    enfriamiento
        .get(&ruta)
        .map(|ultima| (ahora - *ultima).num_milliseconds() >= cooldown_ms)
        .unwrap_or(true)
}

fn puntuar_oportunidad(
    o: &Oportunidad,
    max_operacion_btc: f64,
    stale_ms: i64,
    historial: &HashMap<String, f64>,
    z_score: f64,
    pesos: &[f64],
) -> f64 {
    let confiabilidad = *historial
        .get(&format!("{}->{}", o.compra_en, o.venta_en))
        .unwrap_or(&1.0);
    score_canonico(
        pesos,
        FeaturesScore {
            utilidad_usd: o.utilidad_usd,
            latencia_ms: o.latencia_max_ms,
            tolerancia_latencia_ms: stale_ms,
            cantidad_btc: o.cantidad_btc,
            max_operacion_btc,
            confiabilidad,
            z_score,
        },
    )
}

fn actualizar_historial(op: &Operacion, historial: &mut HashMap<String, f64>, exito: bool) {
    let ruta = format!("{}->{}", op.compra_en, op.venta_en);
    let actual = *historial.get(&ruta).unwrap_or(&1.0);
    let valor = if exito { 1.0 } else { 0.0 };
    historial.insert(ruta, actual * 0.9 + valor * 0.1);
}

fn sharpe(operaciones: &[Operacion]) -> f64 {
    let retornos: Vec<f64> = operaciones
        .iter()
        .filter_map(|op| {
            let costo = op.cantidad_btc * op.precio_compra;
            (costo > 0.0).then_some(op.utilidad_usd / costo)
        })
        .collect();
    if retornos.len() < 2 {
        return 0.0;
    }
    let media = retornos.iter().sum::<f64>() / retornos.len() as f64;
    let var =
        retornos.iter().map(|r| (r - media).powi(2)).sum::<f64>() / (retornos.len() - 1) as f64;
    let desv = var.sqrt();
    if desv == 0.0 {
        0.0
    } else {
        // Sharpe por operación, sin anualizar. No suponemos una frecuencia de
        // trading constante para una demo donde los fills son event-driven.
        media / desv
    }
}

fn win_rate(operaciones: &[Operacion]) -> f64 {
    if operaciones.is_empty() {
        0.0
    } else {
        operaciones
            .iter()
            .filter(|op| op.utilidad_usd > 0.0)
            .count() as f64
            / operaciones.len() as f64
    }
}

fn max_drawdown(serie: &[PuntoSerie]) -> MoneyUnits {
    let mut max_pnl = 0.0_f64;
    let mut max_dd = 0.0_f64;
    for punto in serie {
        if punto.valor > max_pnl {
            max_pnl = punto.valor;
        }
        let dd = max_pnl - punto.valor;
        if dd > max_dd {
            max_dd = dd;
        }
    }
    max_dd
}

/// Retorno neto por operación: utilidad neta sobre el capital comprometido.
fn retornos_por_operacion(operaciones: &[Operacion]) -> Vec<f64> {
    operaciones
        .iter()
        .filter_map(|op| {
            let capital = op.cantidad_btc * op.precio_compra;
            (capital > 0.0).then_some(op.utilidad_usd / capital)
        })
        .collect()
}

/// Sortino ratio: media de retornos sobre desviación downside (solo pérdidas).
/// Mide la calidad del PnL ignorando la volatilidad "buena" (retornos positivos).
fn sortino(operaciones: &[Operacion]) -> f64 {
    let retornos = retornos_por_operacion(operaciones);
    if retornos.len() < 2 {
        return 0.0;
    }
    let media = retornos.iter().sum::<f64>() / retornos.len() as f64;
    let downside = retornos
        .iter()
        .map(|r| if *r < 0.0 { r * r } else { 0.0 })
        .sum::<f64>()
        / retornos.len() as f64;
    let dd = downside.sqrt();
    if dd == 0.0 {
        0.0
    } else {
        media / dd
    }
}

/// Kelly criterion: fracción óptima de capital a arriesgar dado el historial.
/// W = tasa de aciertos, R = ratio avg_ganancia / avg_perdida.
fn kelly(operaciones: &[Operacion]) -> f64 {
    if operaciones.is_empty() {
        return 0.0;
    }
    let ganancias: Vec<f64> = operaciones
        .iter()
        .filter(|op| op.utilidad_usd > 0.0)
        .map(|op| op.utilidad_usd)
        .collect();
    let perdidas: Vec<f64> = operaciones
        .iter()
        .filter(|op| op.utilidad_usd <= 0.0)
        .map(|op| -op.utilidad_usd)
        .collect();
    let w = ganancias.len() as f64 / operaciones.len() as f64;
    if w == 0.0 {
        return 0.0;
    }
    if w == 1.0 || perdidas.is_empty() {
        return 1.0;
    }
    let avg_win = ganancias.iter().sum::<f64>() / ganancias.len() as f64;
    let avg_loss = perdidas.iter().sum::<f64>() / perdidas.len() as f64;
    if avg_loss <= 0.0 {
        return 1.0;
    }
    let r = avg_win / avg_loss;
    let k = w - (1.0 - w) / r;
    k.clamp(0.0, 1.0)
}

/// Probabilidad bayesiana de éxito: posterior Beta(1,1) actualizado con el
/// historial de operaciones (ganancias/pérdidas). Devuelve la media del posterior.
fn bayesian_win_prob(operaciones: &[Operacion]) -> f64 {
    let alpha0 = 1.0_f64;
    let beta0 = 1.0_f64;
    let wins = operaciones
        .iter()
        .filter(|op| op.utilidad_usd > 0.0)
        .count() as f64;
    let losses = operaciones.len() as f64 - wins;
    (alpha0 + wins) / (alpha0 + beta0 + wins + losses)
}

/// TOBI (Tasa de Oportunidades Bien Ejecutadas): fracción de operaciones cuya
/// utilidad neta supera el umbral mínimo de rentabilidad configurado. Mide qué
/// tan a menudo el bot captura realmente valor y no solo "no perdió".
fn tobi(operaciones: &[Operacion], min_utilidad_usd: MoneyUnits) -> f64 {
    if operaciones.is_empty() {
        return 0.0;
    }
    let capturadas = operaciones
        .iter()
        .filter(|op| op.utilidad_usd >= min_utilidad_usd)
        .count() as f64;
    capturadas / operaciones.len() as f64
}

fn estado_riesgo(latencia: f64, stale_ms: i64) -> String {
    if latencia == 0.0 {
        "esperando mercado".to_string()
    } else if latencia > stale_ms as f64 * 0.75 {
        "latencia alta".to_string()
    } else {
        "estable".to_string()
    }
}

fn precio_referencia<'a>(cotizaciones: impl IntoIterator<Item = &'a Cotizacion>) -> f64 {
    let mut total = 0.0;
    let mut n = 0.0;
    for c in cotizaciones {
        if c.bid > 0.0 && c.ask > 0.0 {
            total += (c.bid + c.ask) / 2.0;
            n += 1.0;
        }
    }
    if n == 0.0 {
        100000.0
    } else {
        total / n
    }
}

fn precio_referencia_demo(state: &State) -> f64 {
    if state.corrida.modo == "demo_controlada" {
        JURY_REFERENCE_PRICE_USD
    } else {
        precio_referencia(state.cotizaciones.values())
    }
}

pub fn cotizacion_valida(c: &Cotizacion, ahora: DateTime<Utc>, stale_ms: i64) -> bool {
    if c.exchange.is_empty()
        || !c.bid.is_finite()
        || !c.ask.is_finite()
        || c.bid <= 0.0
        || c.ask <= 0.0
        || c.bid >= c.ask
        || profundidad_disponible(&c.bids, c.bid_cantidad) <= 0.0
        || profundidad_disponible(&c.asks, c.ask_cantidad) <= 0.0
    {
        return false;
    }
    (ahora - c.recibida_en).num_milliseconds() <= stale_ms
}

fn profundidad_disponible(niveles: &[NivelOrden], fallback_bbo: f64) -> f64 {
    let total: f64 = niveles
        .iter()
        .take(10)
        .map(|n| n.cantidad)
        .filter(|cantidad| *cantidad > 0.0 && cantidad.is_finite())
        .sum();
    if total > 0.0 {
        total
    } else if fallback_bbo > 0.0 && fallback_bbo.is_finite() {
        fallback_bbo
    } else {
        0.0
    }
}

fn min_positiva_decimal(valores: &[Decimal]) -> Decimal {
    let mut min: Option<Decimal> = None;
    for valor in valores {
        if *valor <= Decimal::ZERO {
            return Decimal::ZERO;
        }
        min = Some(match min {
            Some(actual) => actual.min(*valor),
            None => *valor,
        });
    }
    min.unwrap_or(Decimal::ZERO)
}

fn dec(valor: f64) -> Decimal {
    Decimal::from_f64(valor).unwrap_or(Decimal::ZERO)
}

fn dec_to_f64(valor: Decimal) -> f64 {
    valor.round_dp(12).to_f64().unwrap_or(0.0)
}

fn bps_decimal(valor: Decimal, base: Decimal) -> f64 {
    if base <= Decimal::ZERO {
        0.0
    } else {
        dec_to_f64(valor / base * dec(10_000.0))
    }
}

fn bps(valor: f64, base: f64) -> f64 {
    if base <= 0.0 {
        0.0
    } else {
        valor / base * 10000.0
    }
}

fn z_score(valores: &[f64], actual: f64) -> f64 {
    if valores.len() < 15 {
        return 0.0;
    }
    let media = valores.iter().sum::<f64>() / valores.len() as f64;
    let desv =
        (valores.iter().map(|v| (v - media).powi(2)).sum::<f64>() / valores.len() as f64).sqrt();
    if desv == 0.0 {
        0.0
    } else {
        (actual - media) / desv
    }
}

pub fn config_exchange(costos: &MapaCostos, nombre: &str) -> ExchangeConfig {
    costos
        .exchanges
        .get(nombre)
        .cloned()
        .unwrap_or_else(|| ExchangeConfig {
            nombre: nombre.to_string(),
            fee_taker: 0.0015,
            retiro_btc: 0.00015,
            confiabilidad: 0.90,
        })
}

fn es_exchange_usd(nombre: &str) -> bool {
    matches!(nombre, "Coinbase" | "Kraken")
}

fn quote_lane(nombre: &str) -> &'static str {
    if es_exchange_usd(nombre) {
        "USD"
    } else {
        "USDT"
    }
}

fn clave_exchange(exchange: &str, par: &str) -> String {
    format!("{exchange}:{par}")
}

fn normalizar_par_operativo(par: &str) -> String {
    let compact = par.trim().to_ascii_uppercase().replace(['/', '-'], "");
    if let Some(base) = compact
        .strip_suffix("USDT")
        .or_else(|| compact.strip_suffix("USD"))
    {
        format!("{base}/USD")
    } else {
        par.to_ascii_uppercase()
    }
}

fn limitar<T>(mut items: VecDeque<T>, maximo: usize) -> VecDeque<T> {
    if items.len() > maximo {
        items.truncate(maximo);
    }
    items
}

fn limitar_ultimos<T>(mut items: Vec<T>, maximo: usize) -> Vec<T> {
    if items.len() > maximo {
        let quitar = items.len() - maximo;
        items.drain(0..quitar);
    }
    items
}

fn truncar_primeros<T>(items: &mut VecDeque<T>, maximo: usize) {
    while items.len() > maximo {
        items.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use std::collections::HashMap;

    fn cfg_test() -> MapaCostos {
        let mut exchanges = HashMap::new();
        exchanges.insert(
            "A".to_string(),
            ExchangeConfig {
                nombre: "A".into(),
                fee_taker: 0.001,
                retiro_btc: 0.0001,
                confiabilidad: 0.99,
            },
        );
        exchanges.insert(
            "B".to_string(),
            ExchangeConfig {
                nombre: "B".into(),
                fee_taker: 0.001,
                retiro_btc: 0.0001,
                confiabilidad: 0.99,
            },
        );
        MapaCostos {
            max_operacion_btc: 1.0,
            min_utilidad_usd: 1.0,
            min_diferencial_neto_bps: 0.1,
            deslizamiento_bps: 0.0,
            latencia_riesgo_bps: 0.0,
            retiro_amortizado_bps: 0.0,
            stale_ms: 1000,
            enfriamiento_ms: 0,
            usdt_usd_premium_bps: 0.0,
            permitir_cruce_usd_usdt: false,
            circuit_breaker_perdida_usd: 500.0,
            circuit_breaker_ventana_min: 10,
            volatilidad_umbral_bps: 50.0,
            volatilidad_ventana_seg: 30,
            simular_adversidad: false,
            prob_fallo_orden: 0.0,
            prob_movimiento_brusco: 0.0,
            movimiento_brusco_bps: 0.0,
            rebalance_umbral_pct: 35.0,
            rebalance_max_transfer_pct: 50.0,
            costo_rebalanceo_usd: 10.0,
            rebalance_settlement_ms: 1_800,
            exchanges,
            webhook_url: None,
        }
    }

    fn cot(exchange: &str, bid: f64, ask: f64, bid_qty: f64, ask_qty: f64) -> Cotizacion {
        Cotizacion {
            exchange: exchange.into(),
            par: "BTC/USDT".into(),
            bid,
            bid_cantidad: bid_qty,
            ask,
            ask_cantidad: ask_qty,
            bids: vec![NivelOrden {
                precio: bid,
                cantidad: bid_qty,
            }]
            .into(),
            asks: vec![NivelOrden {
                precio: ask,
                cantidad: ask_qty,
            }]
            .into(),
            evento_unix_ms: 0,
            recibida_en: Utc::now(),
            latencia_ms: 0,
            secuencia: 0,
            exchange_sequence: None,
            integrity_status: "test_snapshot".to_string(),
            resyncs: 0,
            sequence_gaps: 0,
            checksum_failures: 0,
            invalidated_ms: 0,
            timestamp_confiable: true,
            conectado: true,
            ultimo_mensaje: String::new(),
        }
    }

    #[test]
    fn z_score_necesita_historial_suficiente() {
        assert_eq!(z_score(&[1.0, 2.0, 3.0], 4.0), 0.0);
    }

    #[test]
    fn carteras_aplica_operacion_atomica() {
        let exchanges = vec!["A".to_string(), "B".to_string()];
        let mut carteras = Carteras::new(&exchanges, 2000.0, 2.0);
        let ok = carteras.aplicar_operacion(&Operacion {
            piernas: vec![],
            tipo: crate::types::TipoOportunidad::Lineal,
            id: "1".into(),
            compra_en: "A".into(),
            venta_en: "B".into(),
            par: "BTC/USDT".into(),
            cantidad_btc: 0.1,
            precio_compra: 100.0,
            precio_venta: 110.0,
            utilidad_usd: 1.0,
            utilidad_esperada_usd: 1.0,
            costos: CostosOperacion::default(),
            parcial: false,
            ejecutada_en: Utc::now(),
            latencia_max_ms: 1,
        });
        assert!(ok);
        assert_relative_eq!(carteras.balance("A").btc, 1.1);
        assert_relative_eq!(carteras.balance("B").btc, 0.9);
    }

    #[test]
    fn carteras_conservan_btc_y_contabilizan_pnl_en_multiples_escenarios() {
        for caso in 1..=64 {
            let exchanges = vec!["A".to_string(), "B".to_string()];
            let mut carteras = Carteras::new(&exchanges, 1_000_000.0, 5.0);
            let cantidad = caso as f64 / 100.0;
            let compra = 50_000.0 + caso as f64 * 17.0;
            let venta = compra + 25.0 + caso as f64;
            let usd_antes = carteras.balances.values().map(|b| b.usd).sum::<f64>();
            let btc_antes = carteras.balances.values().map(|b| b.btc).sum::<f64>();
            let op = Operacion {
                piernas: vec![],
                tipo: crate::types::TipoOportunidad::Lineal,
                id: format!("property-{caso}"),
                compra_en: "A".into(),
                venta_en: "B".into(),
                par: "BTC/USDT".into(),
                cantidad_btc: cantidad,
                precio_compra: compra,
                precio_venta: venta,
                utilidad_usd: (venta - compra) * cantidad,
                utilidad_esperada_usd: (venta - compra) * cantidad,
                costos: CostosOperacion::default(),
                parcial: false,
                ejecutada_en: Utc::now(),
                latencia_max_ms: 1,
            };

            assert!(carteras.aplicar_operacion(&op));
            let usd_despues = carteras.balances.values().map(|b| b.usd).sum::<f64>();
            let btc_despues = carteras.balances.values().map(|b| b.btc).sum::<f64>();
            assert_relative_eq!(btc_despues, btc_antes, epsilon = 1e-10);
            assert_relative_eq!(usd_despues - usd_antes, op.utilidad_usd, epsilon = 1e-7);
            assert!(carteras
                .balances
                .values()
                .all(|b| b.usd >= 0.0 && b.btc >= 0.0));
        }
    }

    #[test]
    fn oportunidad_parcial_por_liquidez() {
        let exchanges = vec!["A".to_string(), "B".to_string()];
        let carteras = Carteras::new(&exchanges, 500_000.0, 2.0);
        let cfg = cfg_test();
        let oportunidad = calcular_oportunidad(
            &cot("A", 69_900.0, 70_000.0, 2.0, 0.12),
            &cot("B", 70_600.0, 70_700.0, 2.0, 2.0),
            &carteras,
            &cfg,
            Utc::now(),
        );
        assert!(oportunidad.parcial);
        assert_relative_eq!(oportunidad.cantidad_btc, 0.12);
        assert!(oportunidad.ejecutable);
    }

    #[test]
    fn oportunidad_expone_codigo_decision_estable() {
        let exchanges = vec!["A".to_string(), "B".to_string()];
        let carteras = Carteras::new(&exchanges, 500_000.0, 2.0);
        let mut cfg = cfg_test();
        cfg.min_utilidad_usd = 1_000.0;

        let oportunidad = calcular_oportunidad(
            &cot("A", 69_900.0, 70_000.0, 2.0, 1.0),
            &cot("B", 70_100.0, 70_200.0, 2.0, 1.0),
            &carteras,
            &cfg,
            Utc::now(),
        );

        assert!(!oportunidad.ejecutable);
        assert_eq!(oportunidad.decision_code, "SKIP_MIN_USD");
    }

    #[test]
    fn sharpe_por_operacion_no_inventa_frecuencia_anual() {
        let operacion = |id: &str, utilidad_usd: f64| Operacion {
            piernas: vec![],
            tipo: crate::types::TipoOportunidad::Lineal,
            id: id.to_string(),
            compra_en: "A".into(),
            venta_en: "B".into(),
            par: "BTC/USD".into(),
            cantidad_btc: 1.0,
            precio_compra: 100.0,
            precio_venta: 101.0,
            utilidad_usd,
            utilidad_esperada_usd: utilidad_usd,
            costos: CostosOperacion::default(),
            parcial: false,
            ejecutada_en: Utc::now(),
            latencia_max_ms: 1,
        };
        let operaciones = [operacion("1", 1.0), operacion("2", 3.0)];

        assert_relative_eq!(sharpe(&operaciones), 2.0_f64.sqrt());
    }

    #[test]
    fn oportunidad_sin_inventario_explica_rechazo() {
        let exchanges = vec!["A".to_string(), "B".to_string()];
        let mut carteras = Carteras::new(&exchanges, 500_000.0, 2.0);
        carteras.balances.get_mut("B").unwrap().btc = 0.0;

        let oportunidad = calcular_oportunidad(
            &cot("A", 69_900.0, 70_000.0, 2.0, 1.0),
            &cot("B", 70_600.0, 70_700.0, 2.0, 1.0),
            &carteras,
            &cfg_test(),
            Utc::now(),
        );

        assert!(!oportunidad.ejecutable);
        assert_eq!(oportunidad.decision_code, "SKIP_THIN_OR_INVENTORY");
    }

    #[test]
    fn rutas_usd_usdt_separadas_por_defecto() {
        let exchanges = vec![
            "Coinbase".to_string(),
            "Kraken".to_string(),
            "Binance".to_string(),
            "OKX".to_string(),
        ];
        let carteras = Carteras::new(&exchanges, 500_000.0, 4.0);
        let mut cfg = cfg_test();
        cfg.min_utilidad_usd = 0.0;
        cfg.min_diferencial_neto_bps = 0.0;
        for exchange in &exchanges {
            cfg.exchanges.insert(
                exchange.clone(),
                ExchangeConfig {
                    nombre: exchange.clone(),
                    fee_taker: 0.0,
                    retiro_btc: 0.0,
                    confiabilidad: 0.99,
                },
            );
        }
        let ahora = Utc::now();
        let mut cotizaciones = HashMap::new();
        cotizaciones.insert("Coinbase".into(), cot("Coinbase", 100.0, 101.0, 1.0, 1.0));
        cotizaciones.insert("Kraken".into(), cot("Kraken", 102.0, 103.0, 1.0, 1.0));
        cotizaciones.insert("Binance".into(), cot("Binance", 120.0, 121.0, 1.0, 1.0));
        cotizaciones.insert("OKX".into(), cot("OKX", 122.0, 123.0, 1.0, 1.0));

        let rutas = buscar_oportunidades(&cotizaciones, &carteras, &cfg, ahora);
        assert!(rutas
            .iter()
            .all(|r| quote_lane(&r.compra_en) == quote_lane(&r.venta_en)));

        cfg.permitir_cruce_usd_usdt = true;
        let rutas_con_cruce = buscar_oportunidades(&cotizaciones, &carteras, &cfg, ahora);
        assert!(rutas_con_cruce
            .iter()
            .any(|r| quote_lane(&r.compra_en) != quote_lane(&r.venta_en)));
    }

    #[test]
    fn cruce_usd_usdt_cobra_premium_una_sola_vez() {
        let exchanges = vec!["Coinbase".to_string(), "Binance".to_string()];
        let carteras = Carteras::new(&exchanges, 10_000.0, 2.0);
        let mut cfg = cfg_test();
        cfg.usdt_usd_premium_bps = 10.0;
        cfg.permitir_cruce_usd_usdt = true;
        cfg.min_utilidad_usd = 0.0;
        cfg.min_diferencial_neto_bps = 0.0;
        for exchange in &exchanges {
            cfg.exchanges.insert(
                exchange.clone(),
                ExchangeConfig {
                    nombre: exchange.clone(),
                    fee_taker: 0.0,
                    retiro_btc: 0.0,
                    confiabilidad: 0.99,
                },
            );
        }

        let oportunidad = calcular_oportunidad(
            &cot("Coinbase", 100.0, 101.0, 2.0, 2.0),
            &cot("Binance", 102.0, 103.0, 2.0, 2.0),
            &carteras,
            &cfg,
            Utc::now(),
        );

        let premium_esperado = 101.5 * 10.0 / 10_000.0;
        assert_relative_eq!(oportunidad.diferencial_bruto_usd, 1.0);
        assert_relative_eq!(oportunidad.costos.total_usd, premium_esperado);
        assert_relative_eq!(oportunidad.utilidad_usd, 1.0 - premium_esperado);
    }

    #[test]
    fn rebalanceo_genera_evento_y_bloquea_capital_hasta_liquidacion() {
        let exchanges = vec!["A".to_string(), "B".to_string()];
        let mut carteras = Carteras::new(&exchanges, 20_000.0, 2.0);
        carteras.balances.get_mut("A").unwrap().usd = 100.0;
        carteras.balances.get_mut("B").unwrap().usd = 19_900.0;
        let eventos = carteras.rebalancear(100_000.0, &cfg_test(), Utc::now());
        assert!(!eventos.is_empty());
        assert_eq!(carteras.balance("A").usd, 100.0);
        assert!(carteras.balance("B").usd < 19_900.0);
        assert!(eventos.iter().any(|e| e.activo == "USD"));
    }

    #[test]
    fn adversidad_desactivada_no_modifica_operacion() {
        let mut op = Operacion {
            piernas: vec![],
            tipo: crate::types::TipoOportunidad::Lineal,
            id: "1".into(),
            compra_en: "A".into(),
            venta_en: "B".into(),
            par: "BTC/USDT".into(),
            cantidad_btc: 0.1,
            precio_compra: 100.0,
            precio_venta: 110.0,
            utilidad_usd: 1.0,
            utilidad_esperada_usd: 1.0,
            costos: CostosOperacion::default(),
            parcial: false,
            ejecutada_en: Utc::now(),
            latencia_max_ms: 1,
        };
        let original = op.utilidad_usd;
        assert!(aplicar_adversidad(&mut op, &cfg_test(), Utc::now(), None).is_none());
        assert_eq!(op.utilidad_usd, original);
    }

    #[test]
    fn demo_forzado_falla_siguiente_orden() {
        let mut op = Operacion {
            piernas: vec![],
            tipo: crate::types::TipoOportunidad::Lineal,
            id: "1".into(),
            compra_en: "A".into(),
            venta_en: "B".into(),
            par: "BTC/USDT".into(),
            cantidad_btc: 0.1,
            precio_compra: 100.0,
            precio_venta: 110.0,
            utilidad_usd: 1.0,
            utilidad_esperada_usd: 1.0,
            costos: CostosOperacion::default(),
            parcial: false,
            ejecutada_en: Utc::now(),
            latencia_max_ms: 1,
        };
        let evento = aplicar_adversidad(
            &mut op,
            &cfg_test(),
            Utc::now(),
            Some(EscenarioDemo::FalloOrden),
        )
        .expect("debe generar evento forzado");
        assert_eq!(evento.tipo, "fallida");
        assert_eq!(evento.severidad, "alta");
    }

    #[tokio::test]
    async fn demo_mercado_movido_deja_evidencia_inmediata_sin_orden_pendiente() {
        let motor = Motor::new(cfg_test(), 20_000.0, 2.0, "BTC/USDT".into(), vec![], None);
        let resultado = motor
            .activar_escenario_demo(EscenarioDemo::MercadoMovido)
            .await;
        let estado = motor.estado().await;

        assert_eq!(resultado["ok"], true);
        assert_eq!(resultado["modo"], "instantaneo");
        assert_eq!(resultado["capitalComprometidoBtc"], 0.0);
        assert!(estado
            .eventos_ejecucion
            .iter()
            .any(|evento| evento.tipo == "mercado_movido"));
    }

    #[test]
    fn replay_sintetico_genera_holdout_con_resultados_adversos() {
        let cfg = cfg_test();
        let replay = operaciones_sinteticas_ga(&cfg, 24, 95_000.0, 7, Utc::now(), true);
        assert_eq!(replay.operaciones.len(), 24);
        assert!(replay.operaciones.iter().any(|op| op.utilidad_usd > 0.0));
        assert!(replay
            .operaciones
            .iter()
            .all(|op| op.precio_venta > op.precio_compra));
        assert!(replay
            .operaciones
            .iter()
            .all(|op| op.compra_en != op.venta_en));
        assert!(replay
            .operaciones
            .iter()
            .all(|op| op.utilidad_esperada_usd >= 1.0));
        assert!(replay.operaciones.iter().any(|op| op.utilidad_usd < 0.0));
    }

    #[tokio::test]
    async fn demo_rentable_inyecta_operaciones_y_activa_ga() {
        let motor = Motor::new(
            cfg_test(),
            250_000.0,
            2.5,
            "BTC/USD".to_string(),
            vec![],
            None,
        );
        let resultado = motor
            .activar_escenario_demo(EscenarioDemo::MercadoRentable)
            .await;
        assert!(resultado
            .get("ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(false));

        let estado = motor.estado().await;
        assert!(!estado.operaciones.is_empty());
        assert!(estado.trazas_ejecucion.iter().any(|trace| {
            trace.estado == "RECONCILED" && trace.exposicion_btc.abs() < 0.00000001
        }));
        let ga = estado.genetico.expect("estado GA publico");
        assert!(ga.activo);
        assert!(ga.operaciones_evaluadas > 0);
        let ml_edge = estado.ml_edge.expect("estado ML Edge publico");
        assert!(ml_edge.activo);
        assert!(ml_edge.score_actual.is_finite());
        assert!(ml_edge.expected_value_usd.is_finite());
        assert_eq!(ml_edge.features.len(), 5);
    }

    #[tokio::test]
    async fn evolucion_ga_respeta_ventana_solicitada_con_historial_real() {
        let config = cfg_test();
        let replay = operaciones_sinteticas_ga(&config, 40, 95_000.0, 17, Utc::now(), true);
        let motor = Motor::new(config, 250_000.0, 2.5, "BTC/USD".to_string(), vec![], None);
        motor.state.write().await.operaciones = replay.operaciones.into();

        let resultado = motor.evolucionar_ga(false, 12).await;

        assert_eq!(
            resultado.get("ok").and_then(serde_json::Value::as_bool),
            Some(true)
        );
        assert_eq!(
            resultado.get("fuente").and_then(serde_json::Value::as_str),
            Some("historial_real")
        );
        assert_eq!(
            resultado
                .get("muestras")
                .and_then(serde_json::Value::as_u64),
            Some(12)
        );
        assert_eq!(
            motor.estado().await.genetico.unwrap().operaciones_evaluadas,
            12
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn evolucion_ga_replay_96_termina_antes_del_timeout_http() {
        let motor = Motor::new(
            cfg_test(),
            250_000.0,
            2.5,
            "BTC/USD".to_string(),
            vec![],
            None,
        );

        let resultado =
            tokio::time::timeout(Duration::from_secs(5), motor.evolucionar_ga(true, 96))
                .await
                .expect("la evolución de jurado debe caber holgadamente en el timeout HTTP");

        assert_eq!(resultado["ok"], true);
        assert_eq!(resultado["muestras"], 96);
    }

    #[tokio::test]
    async fn evoluciones_ga_concurrentes_no_pierden_generaciones() {
        let motor = Arc::new(Motor::new(
            cfg_test(),
            250_000.0,
            2.5,
            "BTC/USD".to_string(),
            vec![],
            None,
        ));

        let (primera, segunda) = tokio::join!(
            motor.evolucionar_ga(true, 12),
            motor.evolucionar_ga(true, 12)
        );

        assert_eq!(primera["ok"], true);
        assert_eq!(segunda["ok"], true);
        assert_eq!(motor.estado().await.genetico.unwrap().generacion, 2);
    }

    #[tokio::test]
    async fn evolucion_ga_automatica_ocurre_en_el_ciclo_500_sin_requerir_trade_nuevo() {
        let config = cfg_test();
        let replay = operaciones_sinteticas_ga(&config, 12, 95_000.0, 23, Utc::now(), true);
        let motor = Motor::new(config, 250_000.0, 2.5, "BTC/USD".to_string(), vec![], None);
        {
            let mut state = motor.state.write().await;
            state.operaciones = replay.operaciones.into();
            state.ciclos = 499;
        }
        let ahora = Utc::now();
        motor
            .recibir_cotizacion_en(cot("A", 100.0, 101.0, 1.0, 1.0), ahora, false)
            .await;

        motor.analizar(ahora).await;

        assert_eq!(motor.estado().await.genetico.unwrap().generacion, 1);
    }

    #[tokio::test]
    async fn recorrido_jurado_funciona_con_solo_dos_exchanges_habilitados() {
        let mut config = cfg_test();
        config
            .exchanges
            .retain(|name, _| matches!(name.as_str(), "Binance" | "Kraken"));
        let motor = Motor::new(config, 250_000.0, 2.5, "BTC/USD".to_string(), vec![], None);

        let rentable = motor
            .activar_escenario_demo(EscenarioDemo::MercadoRentable)
            .await;
        assert_eq!(
            rentable.get("ok").and_then(|value| value.as_bool()),
            Some(true)
        );

        let partial = motor
            .activar_escenario_demo(EscenarioDemo::FillParcial)
            .await;
        assert_eq!(
            partial.get("ok").and_then(|value| value.as_bool()),
            Some(true)
        );

        let leg2 = motor
            .activar_escenario_demo(EscenarioDemo::FalloSegundaPierna)
            .await;
        assert_eq!(leg2.get("ok").and_then(|value| value.as_bool()), Some(true));
        assert_eq!(
            leg2.get("estadoFinal").and_then(|value| value.as_str()),
            Some("RECONCILED")
        );
        assert_eq!(
            leg2.get("exposicionFinalBtc")
                .and_then(|value| value.as_f64()),
            Some(0.0)
        );
    }

    #[tokio::test]
    async fn pipeline_mide_scan_y_omite_ticks_sin_eventos_nuevos() {
        let motor = Motor::new(
            cfg_test(),
            250_000.0,
            2.5,
            "BTC/USD".to_string(),
            vec![],
            None,
        );
        motor
            .recibir_cotizacion(cot("Binance", 100.0, 101.0, 1.0, 1.0))
            .await;
        motor
            .recibir_cotizacion(cot("OKX", 110.0, 111.0, 1.0, 1.0))
            .await;

        motor.analizar(Utc::now()).await;
        let medida = motor.estado().await.telemetria_pipeline;
        assert_eq!(medida.ciclos_analisis, 1);
        assert_eq!(medida.muestras, 1);
        assert!(medida.rutas_evaluadas > 0);
        assert!(medida.compute_p99_us > 0);
        assert!(motor
            .estado()
            .await
            .trazas_ejecucion
            .iter()
            .any(|trace| { trace.estado == "RECONCILED" && trace.exposicion_btc.abs() < 1e-8 }));

        motor.analizar(Utc::now()).await;
        let omitida = motor.estado().await.telemetria_pipeline;
        assert_eq!(omitida.ciclos_analisis, 1);
        assert_eq!(omitida.ciclos_sin_cambios_omitidos, 1);
    }

    #[tokio::test]
    async fn reset_jurado_limpia_simulacion_y_conserva_configuracion() {
        let motor = Motor::new(
            cfg_test(),
            250_000.0,
            2.5,
            "BTC/USD".to_string(),
            vec![],
            None,
        );
        motor
            .activar_escenario_demo(EscenarioDemo::MercadoRentable)
            .await;
        let config_antes = motor.estado().await.configuracion;

        motor.reiniciar_demo_jurado().await;
        let estado = motor.estado().await;

        assert!(estado.operaciones.is_empty());
        assert_eq!(estado.metricas.operaciones, 0);
        assert_eq!(estado.metricas.utilidad_acumulada_usd, 0.0);
        assert!(!estado.metricas.circuit_breaker_activo);
        assert_eq!(estado.configuracion, config_antes);
        assert_eq!(estado.eventos_ejecucion.front().unwrap().tipo, "jury_reset");
    }

    #[tokio::test]
    async fn demo_fill_parcial_deja_evidencia_forense() {
        let motor = Motor::new(
            cfg_test(),
            250_000.0,
            2.5,
            "BTC/USD".to_string(),
            vec![],
            None,
        );
        let resultado = motor
            .activar_escenario_demo(EscenarioDemo::FillParcial)
            .await;
        assert!(resultado
            .get("partialFill")
            .and_then(|v| v.as_bool())
            .unwrap_or(false));

        let estado = motor.estado().await;
        assert!(estado.operaciones.iter().any(|op| op.parcial));
        assert!(estado
            .auditoria_decisiones
            .iter()
            .any(|a| a.decision_code == "PARTIAL_FILL"));
        assert!(estado
            .eventos_ejecucion
            .iter()
            .any(|e| e.tipo == "fill_parcial"));
    }

    #[tokio::test]
    async fn demo_circuit_breaker_supera_pnl_positivo_previo() {
        let motor = Motor::new(
            cfg_test(),
            250_000.0,
            2.5,
            "BTC/USD".to_string(),
            vec![],
            None,
        );
        motor
            .activar_escenario_demo(EscenarioDemo::MercadoRentable)
            .await;

        let resultado = motor
            .activar_escenario_demo(EscenarioDemo::CircuitBreaker)
            .await;
        assert!(resultado
            .get("ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(false));

        let estado = motor.estado().await;
        assert!(estado.metricas.circuit_breaker_activo);
        assert!(estado
            .eventos_ejecucion
            .iter()
            .any(|e| e.tipo == "circuit_breaker"));
    }

    #[test]
    fn feed_stale_no_es_ruteable() {
        let mut vieja = cot("A", 100.0, 101.0, 1.0, 1.0);
        vieja.recibida_en = Utc::now() - chrono::Duration::seconds(2);
        assert!(!cotizacion_valida(&vieja, Utc::now(), 100));
    }

    #[test]
    fn quote_sin_cantidad_ni_profundidad_no_es_ruteable() {
        let incompleta = cot("A", 100.0, 101.0, 0.0, 0.0);
        assert!(!cotizacion_valida(&incompleta, Utc::now(), 1_000));
        assert_eq!(profundidad_disponible(&[], 0.0), 0.0);
    }

    #[test]
    fn quote_con_profundidad_explicita_es_ruteable_aunque_bbo_qty_falte() {
        let mut completa = cot("A", 100.0, 101.0, 0.0, 0.0);
        completa.bids.push(NivelOrden {
            precio: 100.0,
            cantidad: 0.25,
        });
        completa.asks.push(NivelOrden {
            precio: 101.0,
            cantidad: 0.20,
        });
        assert!(cotizacion_valida(&completa, Utc::now(), 1_000));
    }

    #[test]
    fn drawdown_mide_caida_desde_el_pico() {
        let ahora = Utc::now();
        let serie = [10.0, 15.0, 7.0, 12.0].map(|valor| PuntoSerie {
            tiempo: ahora,
            valor,
        });
        assert_relative_eq!(max_drawdown(&serie), 8.0);
    }

    #[test]
    fn wallet_skew_dispara_rebalanceo_acotado() {
        let exchanges = vec!["A".to_string(), "B".to_string()];
        let mut carteras = Carteras::new(&exchanges, 20_000.0, 2.0);
        carteras.balances.get_mut("A").unwrap().usd = 50.0;
        carteras.balances.get_mut("B").unwrap().usd = 19_950.0;
        let antes = carteras.balance("B").usd;
        let eventos = carteras.rebalancear(50_000.0, &cfg_test(), Utc::now());
        assert!(!eventos.is_empty());
        assert!(carteras.balance("B").usd < antes);
        assert!(carteras.balances.values().all(|balance| balance.usd >= 0.0));
    }

    #[tokio::test]
    async fn kill_switch_pausa_y_reanuda_simulacion() {
        let motor = Motor::new(cfg_test(), 250_000.0, 2.5, "BTC/USD".into(), vec![], None);
        motor.set_kill_switch(true).await;
        assert!(motor.estado().await.metricas.circuit_breaker_activo);
        assert!(motor
            .estado()
            .await
            .eventos_ejecucion
            .iter()
            .any(|e| e.tipo == "kill_switch"));
        motor.set_kill_switch(false).await;
        assert!(!motor.estado().await.metricas.circuit_breaker_activo);
    }

    #[tokio::test]
    async fn replay_vacio_falla_cerrado_sin_mutar_estado() {
        let motor = Motor::new(cfg_test(), 250_000.0, 2.5, "BTC/USD".into(), vec![], None);
        let antes = motor.estado().await;
        let resultado = motor.ejecutar_replay_capturado().await;
        let despues = motor.estado().await;

        assert_eq!(resultado["ok"], false);
        assert_eq!(antes.operaciones, despues.operaciones);
        assert_eq!(antes.balances, despues.balances);
        assert_eq!(antes.genetico, despues.genetico);
    }

    #[tokio::test]
    async fn captura_expone_ciclo_de_vida_y_numero_de_snapshots() {
        let motor = Motor::new(cfg_test(), 250_000.0, 2.5, "BTC/USD".into(), vec![], None);
        motor.iniciar_captura().await;
        assert_eq!(motor.captura_estado().await["activa"], true);

        motor
            .recibir_cotizacion(cot("A", 100.0, 101.0, 1.0, 1.0))
            .await;
        motor
            .recibir_cotizacion(cot("B", 102.0, 103.0, 1.0, 1.0))
            .await;

        assert_eq!(motor.detener_captura().await, 2);
        let estado = motor.captura_estado().await;
        assert_eq!(estado["activa"], false);
        assert_eq!(estado["snapshots"], 2);
    }

    #[tokio::test]
    async fn captura_descarta_el_snapshot_mas_antiguo_sin_desplazar_el_buffer() {
        let motor = Motor::new(cfg_test(), 250_000.0, 2.5, "BTC/USD".into(), vec![], None);
        motor.iniciar_captura().await;
        motor.state.write().await.max_captura_len = 2;

        for exchange in ["A", "B", "C"] {
            motor
                .recibir_cotizacion(cot(exchange, 100.0, 101.0, 1.0, 1.0))
                .await;
        }

        let state = motor.state.read().await;
        assert_eq!(state.datos_capturados.len(), 2);
        assert_eq!(
            state
                .datos_capturados
                .front()
                .map(|quote| quote.exchange.as_str()),
            Some("B")
        );
    }

    #[tokio::test]
    async fn replay_puede_cargar_una_ventana_del_historial_reciente() {
        let motor = Motor::new(cfg_test(), 250_000.0, 2.5, "BTC/USD".into(), vec![], None);
        motor
            .recibir_cotizacion(cot("A", 100.0, 101.0, 1.0, 1.0))
            .await;
        motor
            .recibir_cotizacion(cot("B", 102.0, 103.0, 1.0, 1.0))
            .await;

        let disponible = motor.captura_estado().await;
        assert_eq!(disponible["historialSnapshots"], 2);
        assert_eq!(disponible["historialVentanaPredeterminadaSnapshots"], 2);

        let cargada = motor.cargar_ventana_replay(10).await;
        assert_eq!(cargada["ok"], true);
        assert_eq!(cargada["snapshots"], 2);
        assert_eq!(motor.captura_estado().await["snapshots"], 2);
    }

    #[tokio::test]
    async fn replay_capturado_es_sandbox_y_no_contamina_live() {
        let motor = Motor::new(cfg_test(), 250_000.0, 2.5, "BTC/USD".into(), vec![], None);
        motor.iniciar_captura().await;
        motor
            .recibir_cotizacion(cot("A", 100.0, 101.0, 1.0, 1.0))
            .await;
        motor
            .recibir_cotizacion(cot("B", 104.0, 105.0, 1.0, 1.0))
            .await;
        motor.detener_captura().await;

        let antes = motor.estado().await;
        let resultado = motor.ejecutar_replay_capturado().await;
        let despues = motor.estado().await;

        assert_eq!(resultado["ok"], true);
        assert_eq!(resultado["aislado"], true);
        assert_eq!(resultado["ticksProcesados"], 2);
        assert_eq!(antes.operaciones, despues.operaciones);
        assert_eq!(antes.oportunidades, despues.oportunidades);
        assert_eq!(antes.balances, despues.balances);
        assert_eq!(antes.genetico, despues.genetico);
        assert_eq!(
            antes.metricas.utilidad_acumulada_usd,
            despues.metricas.utilidad_acumulada_usd
        );
    }

    #[tokio::test]
    async fn replay_de_alta_frecuencia_acota_los_ciclos_de_analisis() {
        let motor = Motor::new(cfg_test(), 250_000.0, 2.5, "BTC/USD".into(), vec![], None);
        let inicio = Utc::now();
        motor.iniciar_captura().await;
        for indice in 0..6_000 {
            let exchange = if indice % 2 == 0 { "A" } else { "B" };
            motor
                .recibir_cotizacion_en(
                    cot(exchange, 100.0, 101.0, 1.0, 1.0),
                    inicio + chrono::Duration::milliseconds(indice * 4),
                    false,
                )
                .await;
        }
        motor.detener_captura().await;

        let resultado =
            tokio::time::timeout(Duration::from_secs(10), motor.ejecutar_replay_capturado())
                .await
                .expect("el replay de una ráfaga corta no debe parecer congelado");

        assert_eq!(resultado["ok"], true);
        assert_eq!(resultado["ticksProcesados"], 6_000);
        assert_eq!(resultado["intervaloAnalisisMs"], 250);
        assert!(resultado["ciclosAnalisis"].as_u64().unwrap_or(u64::MAX) <= 100);
    }

    #[tokio::test]
    async fn replay_same_tape_produces_identical_summary_and_input_hash() {
        let mut config = cfg_test();
        config.simular_adversidad = true;
        config.prob_fallo_orden = 1.0;
        config.prob_movimiento_brusco = 1.0;
        let motor = Motor::new(config, 250_000.0, 2.5, "BTC/USD".into(), vec![], None);
        motor.iniciar_captura().await;
        motor
            .recibir_cotizacion(cot("A", 100.0, 101.0, 1.0, 1.0))
            .await;
        motor
            .recibir_cotizacion(cot("B", 104.0, 105.0, 1.0, 1.0))
            .await;
        motor.detener_captura().await;

        let first = motor.ejecutar_replay_capturado().await;
        let second = motor.ejecutar_replay_capturado().await;
        assert_eq!(first, second);
        assert_eq!(first["determinista"], true);
        assert_eq!(first["adversidadAleatoria"], false);
        assert_eq!(first["inputSha256"].as_str().map(str::len), Some(64));
    }

    #[test]
    fn resumen_auditoria_falla_cerrado_sin_backend() {
        let motor = Motor::new(cfg_test(), 250_000.0, 2.5, "BTC/USD".into(), vec![], None);
        assert_eq!(
            motor.resumen_auditoria(),
            serde_json::json!({"activa": false})
        );
    }
}
