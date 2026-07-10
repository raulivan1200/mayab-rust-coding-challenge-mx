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
    time::Duration,
};

use chrono::{DateTime, Utc};
use rand::{rngs::StdRng, Rng, SeedableRng};
use rust_decimal::{
    prelude::{FromPrimitive, ToPrimitive},
    Decimal,
};
use tokio::sync::RwLock;

use crate::{ga::EstadoGa, persistencia::Persistencia, types::*};

/// Motor central de arbitraje simulado.
///
/// El motor es seguro para compartirse entre tareas Tokio mediante `Arc<Motor>`.
/// Sus mutaciones se serializan con `RwLock` y un candado atómico evita dos
/// ejecuciones simuladas simultáneas.
pub struct Motor {
    state: RwLock<State>,
    persistencia: Option<Arc<Persistencia>>,
    eventos: AtomicU64,
    ops_ejecutadas: AtomicU64,
    ops_fallidas: AtomicU64,
    ejecucion_en_curso: AtomicBool,
}

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
    rebalanceos_total: u64,
    costo_rebalanceo_acumulado_usd: f64,
    serie_pnl: VecDeque<PuntoSerie>,
    serie_diferencial: VecDeque<PuntoSerie>,
    latencias_exchange: HashMap<String, LatenciaEstado>,
    enfriamiento: HashMap<String, DateTime<Utc>>,
    utilidad: f64,
    latencia_ewma: f64,
    precios_ref: Vec<PuntoSerie>,
    circuit_breaker_activo: bool,
    modo_conservador: bool,
    historial_rutas: HashMap<String, f64>,
    historial_spreads: HashMap<String, Vec<f64>>,
    ciclos: u64,
    ga: EstadoGa,
    exchanges_activos: HashMap<String, bool>,
    pares_activos: Vec<String>,
    demo_forzado: Option<EscenarioDemo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Escenarios controlados para demostrar robustez sin depender del mercado real.
pub enum EscenarioDemo {
    FalloOrden,
    MercadoMovido,
    LiquidezInsuficiente,
    FillParcial,
    CircuitBreaker,
    Rebalanceo,
    MercadoRentable,
}

#[derive(Clone)]
struct Carteras {
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
        persistencia: Option<Arc<Persistencia>>,
    ) -> Self {
        let exchanges: Vec<String> = ["Binance", "Kraken", "Coinbase", "OKX", "Bybit"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let carteras = Carteras::new(&exchanges, capital_inicial_usd, balance_inicial_btc);
        let exchanges_activos = exchanges.into_iter().map(|e| (e, true)).collect();
        Self {
            state: RwLock::new(State {
                costos,
                inicio: Utc::now(),
                cotizaciones: HashMap::new(),
                carteras,
                oportunidades: VecDeque::with_capacity(128),
                operaciones: VecDeque::with_capacity(128),
                operaciones_riesgo: VecDeque::with_capacity(512),
                eventos_ejecucion: VecDeque::with_capacity(128),
                auditoria_decisiones: VecDeque::with_capacity(160),
                rebalanceos: VecDeque::with_capacity(64),
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
                modo_conservador: false,
                historial_rutas: HashMap::new(),
                historial_spreads: HashMap::new(),
                ciclos: 0,
                ga: EstadoGa::default(),
                exchanges_activos,
                pares_activos: vec![normalizar_par_operativo(&par_base)],
                demo_forzado: None,
            }),
            persistencia,
            eventos: AtomicU64::new(0),
            ops_ejecutadas: AtomicU64::new(0),
            ops_fallidas: AtomicU64::new(0),
            ejecucion_en_curso: AtomicBool::new(false),
        }
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
    pub async fn recibir_cotizacion(&self, mut cotizacion: Cotizacion) {
        let ahora = Utc::now();
        cotizacion.recibida_en = ahora;
        if cotizacion.evento_unix_ms > 0 {
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
        state.cotizaciones.insert(clave, cotizacion);
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
        let (cotizaciones, costos, carteras, activo, historial, enfriamiento, pesos) = {
            let mut state = self.state.write().await;
            state.ciclos += 1;

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
                        eventos.iter().map(|e| e.costo_usd).sum::<f64>();
                    for e in eventos.into_iter().rev() {
                        state.rebalanceos.push_front(e);
                    }
                    state.rebalanceos.truncate(64);
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
                state.circuit_breaker_activo,
                state.historial_rutas.clone(),
                state.enfriamiento.clone(),
                pesos,
            )
        };

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
                    "SKIP_CIRCUIT_BREAKER — ejecuciones pausadas; perdida maxima configurada {:.2} USD",
                    costos.circuit_breaker_perdida_usd
                );
            }
        }
        if oportunidades.is_empty() {
            let mut state = self.state.write().await;
            state.oportunidades.clear();
            return;
        }
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
                "ruta descartada: ya hay una operacion simulada en validacion/ejecucion",
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

        let exito = state.carteras.aplicar_operacion(&op);
        actualizar_historial(&op, &mut state.historial_rutas, exito);
        if exito {
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
                "saldo insuficiente al confirmar ejecucion",
                "alta",
                ahora,
            );
            self.persistir_evento(&evento);
            state.eventos_ejecucion.push_front(evento);
            state.eventos_ejecucion.truncate(128);
            self.ops_fallidas.fetch_add(1, Ordering::SeqCst);
            tracing::warn!(ruta = %format!("{}->{}", op.compra_en, op.venta_en), cantidad = op.cantidad_btc, "operacion simulada fallida por saldo insuficiente");
        }
        if state.ciclos % 500 == 0 {
            let mut operaciones = state.operaciones.clone();
            let fallos = self.ops_fallidas.load(Ordering::SeqCst) as usize;
            state.ga.evolucionar(operaciones.make_contiguous(), fallos);
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
            (capital_actual - capital_inicial) / capital_inicial * 10000.0
        } else {
            0.0
        };
        let operaciones_totales = self.ops_ejecutadas.load(Ordering::SeqCst) as usize;
        EstadoPublico {
            generado_en: Utc::now(),
            cotizaciones,
            oportunidades: state.oportunidades.clone(),
            operaciones: state.operaciones.clone(),
            eventos_ejecucion: state.eventos_ejecucion.clone(),
            rebalanceos: state.rebalanceos.clone(),
            auditoria_decisiones: state.auditoria_decisiones.clone(),
            balances: state.carteras.snapshot(),
            latencias_exchange: snapshot_latencias(&state),
            serie_pnl: state.serie_pnl.clone(),
            serie_diferencial: state.serie_diferencial.clone(),
            metricas: Metricas {
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
                sharpe_ratio: sharpe(state.operaciones.clone().make_contiguous()),
                win_rate: win_rate(state.operaciones.clone().make_contiguous()),
                max_drawdown_usd: max_drawdown(state.serie_pnl.clone().make_contiguous()),
                operaciones_totales,
                operaciones_fallidas: self.ops_fallidas.load(Ordering::SeqCst),
                rebalanceos_totales: state.rebalanceos_total as usize,
                costo_rebalanceo_acumulado_usd: state.costo_rebalanceo_acumulado_usd,
                circuit_breaker_activo: state.circuit_breaker_activo,
                modo_conservador: state.modo_conservador,
                ejecucion_en_curso: self.ejecucion_en_curso.load(Ordering::SeqCst),
            },
            configuracion: state.costos.clone(),
            genetico: state.ga.public(),
            ml_edge: construir_ml_edge(&state),
            persistencia: self.persistencia.as_ref().map(|p| p.estado()),
            exchanges_activos: state.exchanges_activos.clone(),
            pares_activos: state.pares_activos.clone(),
        }
    }

    fn persistir_operacion(&self, op: &Operacion) {
        if let Some(persistencia) = &self.persistencia {
            let persistencia = persistencia.clone();
            let op = op.clone();
            std::mem::drop(tokio::task::spawn_blocking(move || {
                if let Err(err) = persistencia.registrar_operacion(&op) {
                    tracing::warn!(error = %err, id = %op.id, "no se pudo persistir operacion");
                }
            }));
        }
    }

    fn persistir_evento(&self, evento: &EventoEjecucion) {
        if let Some(persistencia) = &self.persistencia {
            let persistencia = persistencia.clone();
            let evento = evento.clone();
            std::mem::drop(tokio::task::spawn_blocking(move || {
                if let Err(err) = persistencia.registrar_evento(&evento) {
                    tracing::warn!(error = %err, id = %evento.id, "no se pudo persistir evento");
                }
            }));
        }
    }

    fn persistir_rebalanceo(&self, rebalanceo: &Rebalanceo) {
        if let Some(persistencia) = &self.persistencia {
            let persistencia = persistencia.clone();
            let rebalanceo = rebalanceo.clone();
            std::mem::drop(tokio::task::spawn_blocking(move || {
                if let Err(err) = persistencia.registrar_rebalanceo(&rebalanceo) {
                    tracing::warn!(error = %err, id = %rebalanceo.id, "no se pudo persistir rebalanceo");
                }
            }));
        }
    }

    fn persistir_oportunidades(&self, oportunidades: &[Oportunidad]) {
        if oportunidades.is_empty() {
            return;
        }
        if let Some(persistencia) = &self.persistencia {
            let persistencia = persistencia.clone();
            let oportunidades = oportunidades.to_vec();
            std::mem::drop(tokio::task::spawn_blocking(move || {
                if let Err(err) = persistencia.registrar_oportunidades(&oportunidades) {
                    tracing::warn!(error = %err, total = oportunidades.len(), "no se pudieron persistir oportunidades");
                }
            }));
        }
    }

    fn persistir_auditorias(&self, auditorias: &[AuditoriaDecision]) {
        if auditorias.is_empty() {
            return;
        }
        if let Some(persistencia) = &self.persistencia {
            let persistencia = persistencia.clone();
            let auditorias = auditorias.to_vec();
            std::mem::drop(tokio::task::spawn_blocking(move || {
                if let Err(err) = persistencia.registrar_auditorias(&auditorias) {
                    tracing::warn!(error = %err, total = auditorias.len(), "no se pudieron persistir auditorias");
                }
            }));
        }
    }

    /// Reemplaza la configuración de costos y riesgo del motor.
    pub async fn actualizar_config(&self, cfg: MapaCostos) {
        self.state.write().await.costos = cfg;
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

    /// Fuerza una evolución del GA con historial real o replay sintético.
    pub async fn evolucionar_ga(
        &self,
        usar_replay_si_vacio: bool,
        muestras: usize,
    ) -> serde_json::Value {
        let mut state = self.state.write().await;
        let mut fuente = "historial_real";
        let mut operaciones = state.operaciones.clone();
        let mut fallos = self.ops_fallidas.load(Ordering::SeqCst) as usize;
        if operaciones.is_empty() && usar_replay_si_vacio {
            let seed = (state.ga.generacion as u64)
                .wrapping_mul(1_103_515_245)
                .wrapping_add(0x4d41594142);
            let replay = operaciones_sinteticas_ga(
                &state.costos,
                muestras.clamp(12, 240),
                precio_referencia(state.cotizaciones.values()),
                seed,
                Utc::now(),
            );
            operaciones = replay.operaciones.into();
            fallos = replay.fallos;
            fuente = "replay_sintetico";
        }
        let muestras = operaciones.len();
        state.ga.evolucionar(operaciones.make_contiguous(), fallos);
        let ga = state.ga.api_estado();
        tracing::debug!(
            fuente,
            muestras,
            fallos,
            generacion = state.ga.generacion,
            fitness = state.ga.mejor_fitness,
            "ga evolucionado"
        );
        serde_json::json!({
            "ok": true,
            "generacion": state.ga.generacion,
            "fuente": fuente,
            "muestras": muestras,
            "fallos": fallos,
            "ga": ga,
        })
    }

    /// Activa un escenario controlado de demostración.
    pub async fn activar_escenario_demo(&self, escenario: EscenarioDemo) -> serde_json::Value {
        let ahora = Utc::now();
        let mut state = self.state.write().await;
        match escenario {
            EscenarioDemo::FalloOrden | EscenarioDemo::MercadoMovido => {
                state.demo_forzado = Some(escenario);
                let detalle = match escenario {
                    EscenarioDemo::FalloOrden => {
                        "demo armado: la siguiente orden ejecutable sera rechazada"
                    }
                    EscenarioDemo::MercadoMovido => {
                        "demo armado: la siguiente orden ejecutable sufrira shock de precio"
                    }
                    _ => unreachable!(),
                };
                insertar_evento_sistema(&mut state, "demo_armado", detalle, "media", ahora);
                if let Some(evento) = state.eventos_ejecucion.front() {
                    self.persistir_evento(evento);
                }
                serde_json::json!({ "ok": true, "modo": "pendiente", "detalle": detalle })
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
                let precio = precio_referencia(state.cotizaciones.values());
                let op = operacion_demo_fill_parcial(
                    &state.costos,
                    precio,
                    ahora.timestamp_millis() as u64,
                    ahora,
                );
                let exito = state.carteras.aplicar_operacion(&op);
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
                        id: format!("demo-circuit-{}", ahora.timestamp_millis()),
                        compra_en: "sistema".to_string(),
                        venta_en: "sistema".to_string(),
                        par: "BTC/USD".to_string(),
                        cantidad_btc: 0.0,
                        precio_compra: 0.0,
                        precio_venta: 0.0,
                        utilidad_usd: perdida_demo,
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
                    "demo: ejecuciones pausadas por perdida acumulada simulada",
                    "alta",
                    ahora,
                );
                if let Some(evento) = state.eventos_ejecucion.front() {
                    self.persistir_evento(evento);
                }
                serde_json::json!({ "ok": true, "modo": "instantaneo" })
            }
            EscenarioDemo::Rebalanceo => {
                let precio = precio_referencia(state.cotizaciones.values());
                let evento = state.carteras.forzar_rebalanceo_demo(precio, ahora);
                self.persistir_rebalanceo(&evento);
                state.rebalanceos_total += 1;
                state.rebalanceos.push_front(evento.clone());
                state.rebalanceos.truncate(64);
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

                let precio = precio_referencia(state.cotizaciones.values());
                let replay = operaciones_sinteticas_ga(
                    &state.costos,
                    18,
                    precio,
                    ahora.timestamp_millis() as u64 ^ 0x5155_4d45_5243_4144,
                    ahora,
                );
                let mut insertadas = 0usize;
                for op in replay.operaciones {
                    let exito = state.carteras.aplicar_operacion(&op);
                    if !exito {
                        continue;
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
                    let oportunidad = oportunidad_desde_operacion(&op);
                    let auditoria = auditoria_demo_desde_operacion(&op);
                    let evento = evento_operacion(
                        &op,
                        "demo_rentable",
                        "operacion sintetica rentable inyectada para demostrar flujo end-to-end",
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
                truncar_primeros(&mut state.serie_pnl, 240);

                let mut operaciones = state.operaciones.clone();
                let fallos = self.ops_fallidas.load(Ordering::SeqCst) as usize;
                state.ga.evolucionar(operaciones.make_contiguous(), fallos);
                tracing::debug!(
                    operaciones_insertadas = insertadas,
                    generacion = state.ga.generacion,
                    pnl = state.utilidad,
                    "demo rentable aplicada"
                );
                insertar_evento_sistema(
                    &mut state,
                    "demo_rentable",
                    "demo: se inyectaron operaciones rentables y se entreno el GA con ese historial",
                    "normal",
                    ahora,
                );
                if let Some(evento) = state.eventos_ejecucion.front() {
                    self.persistir_evento(evento);
                }
                serde_json::json!({
                    "ok": true,
                    "modo": "instantaneo",
                    "operacionesInsertadas": insertadas,
                    "generacionGa": state.ga.generacion,
                })
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
                usd,
                btc,
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

    fn aplicar_operacion(&mut self, op: &Operacion) -> bool {
        if op.cantidad_btc <= 0.0 || op.precio_compra <= 0.0 || op.precio_venta <= 0.0 {
            return false;
        }
        let compra_snapshot = match self.balances.get(&op.compra_en) {
            Some(b) => b.clone(),
            None => return false,
        };
        let venta_snapshot = match self.balances.get(&op.venta_en) {
            Some(b) => b.clone(),
            None => return false,
        };
        let cantidad = dec(op.cantidad_btc);
        let costos_extra = (dec(op.costos.total_usd)
            - dec(op.costos.fee_compra_usd)
            - dec(op.costos.fee_venta_usd))
        .max(Decimal::ZERO);
        let costo_compra =
            cantidad * dec(op.precio_compra) + dec(op.costos.fee_compra_usd) + costos_extra;
        let ingreso_venta = cantidad * dec(op.precio_venta) - dec(op.costos.fee_venta_usd);
        if dec(compra_snapshot.usd) < costo_compra || dec(venta_snapshot.btc) < cantidad {
            return false;
        }
        if let Some(compra) = self.balances.get_mut(&op.compra_en) {
            compra.usd = dec_to_f64(dec(compra.usd) - costo_compra);
            compra.btc = dec_to_f64(dec(compra.btc) + cantidad);
        }
        if let Some(venta) = self.balances.get_mut(&op.venta_en) {
            venta.usd = dec_to_f64(dec(venta.usd) + ingreso_venta);
            venta.btc = dec_to_f64(dec(venta.btc) - cantidad);
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
            if init.usd > 0.0 && actual.usd < (1.0 - umbral) * init.usd {
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
                    if let Some(dst_bal) = self.balances.get_mut(&name) {
                        dst_bal.usd += (amount - costos.costo_rebalanceo_usd).max(0.0);
                    }
                    eventos.push(Rebalanceo {
                        id: format!("reb-usd-{}-{}-{}", src, name, ahora.timestamp_millis()),
                        desde: src,
                        hacia: name.clone(),
                        activo: "USD".to_string(),
                        cantidad: amount,
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
            if init.btc > 0.0 && actual.btc < (1.0 - umbral) * init.btc {
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
                if let Some((src, surplus)) = src.filter(|(_, s)| *s > 0.001) {
                    let fee = config_exchange(costos, &src).retiro_btc.max(0.00005);
                    let amount = ((init.btc - actual.btc) * max_transfer).min(surplus);
                    if amount > fee {
                        if let Some(src_bal) = self.balances.get_mut(&src) {
                            src_bal.btc -= amount;
                        }
                        if let Some(dst_bal) = self.balances.get_mut(&name) {
                            dst_bal.btc += amount - fee;
                        }
                        eventos.push(Rebalanceo {
                            id: format!("reb-btc-{}-{}-{}", src, name, ahora.timestamp_millis()),
                            desde: src,
                            hacia: name.clone(),
                            activo: "BTC".to_string(),
                            cantidad: amount - fee,
                            costo_usd: fee * precio_ref,
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
        if let Some(dst) = self.balances.get_mut(&hacia) {
            dst.btc += (cantidad - fee).max(0.0);
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

    fn capital_inicial_usd(&self, precio_btc: f64) -> f64 {
        self.inicial
            .values()
            .map(|b| b.usd + b.btc * precio_btc)
            .sum()
    }

    fn capital_actual_usd(&self, precio_btc: f64) -> f64 {
        self.balances
            .values()
            .map(|b| b.usd + b.btc * precio_btc)
            .sum()
    }
}

fn buscar_oportunidades(
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
    let mut ask_ajustado = dec(compra.ask);
    let mut bid_ajustado = dec(venta.bid);
    if es_exchange_usd(&compra.exchange) != es_exchange_usd(&venta.exchange) {
        let premium = Decimal::ONE + dec(costos.usdt_usd_premium_bps) / dec(10_000.0);
        if es_exchange_usd(&compra.exchange) {
            ask_ajustado *= premium;
        } else {
            bid_ajustado *= premium;
        }
    }
    let diferencial_bruto = bid_ajustado - ask_ajustado;
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
    let costo = calcular_costos(cantidad, compra, venta, latencia_max, costos);
    let utilidad_dec = (bid_ajustado - ask_ajustado) * cantidad_dec - dec(costo.total_usd);
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
        decision_threshold = costos
            .max_operacion_btc
            .min(liquidez_compra.to_f64().unwrap_or(0.0));
        decision_actual = cantidad;
        decision_reason = format!(
            "SKIP_THIN_OR_INVENTORY — cantidad ejecutable {:.8} BTC; compra liquidez {:.8}, venta liquidez {:.8}, USD compra {:.2}, BTC venta {:.8}",
            cantidad,
            liquidez_compra.to_f64().unwrap_or(0.0),
            liquidez_venta.to_f64().unwrap_or(0.0),
            balance_compra.usd,
            balance_venta.btc
        );
    } else if utilidad_dec < dec(costos.min_utilidad_usd) {
        ejecutable = false;
        razon = "utilidad menor al minimo configurado".to_string();
        decision_code = "SKIP_MIN_USD".to_string();
        decision_threshold = costos.min_utilidad_usd;
        decision_actual = utilidad;
        decision_reason = format!(
            "SKIP_MIN_USD — utilidad {:.2} USD < min {:.2} USD despues de costos",
            utilidad, costos.min_utilidad_usd
        );
    } else if dec(diferencial_neto_bps) < dec(costos.min_diferencial_neto_bps) {
        ejecutable = false;
        razon = "diferencial neto bajo despues de costos".to_string();
        decision_code = "SKIP_NET_BPS".to_string();
        decision_threshold = costos.min_diferencial_neto_bps;
        decision_actual = diferencial_neto_bps;
        decision_reason = format!(
            "SKIP_NET_BPS — net {:.2} bps < min {:.2} bps despues de fees, slippage y latencia",
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
        id: oportunidad.id.clone(),
        compra_en: actual.compra_en,
        venta_en: actual.venta_en,
        par: actual.par,
        cantidad_btc: actual.cantidad_btc,
        precio_compra: actual.ask,
        precio_venta: actual.bid,
        utilidad_usd: actual.utilidad_usd,
        costos: actual.costos,
        parcial: actual.parcial,
        ejecutada_en: ahora,
        latencia_max_ms: actual.latencia_max_ms,
    })
}

fn calcular_costos(
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
            "precio se movio por escenario demo controlado",
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
            "precio se movio entre deteccion y ejecucion",
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
        modelo: "Mayab ML Edge GA/EV".to_string(),
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
) -> ReplayGa {
    let exchanges = ["Binance", "Kraken", "Coinbase", "OKX", "Bybit"];
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
            total_usd: 0.0,
        };
        let total_usd = costos_operacion.fee_compra_usd
            + costos_operacion.fee_venta_usd
            + costos_operacion.deslizamiento_usd
            + costos_operacion.retiro_amort_usd
            + costos_operacion.latencia_riesgo_usd;
        let utilidad = ((precio_venta - precio_compra) * cantidad - total_usd).max(1.0);
        let parcial = rng.gen_bool(0.18);
        if rng.gen_bool(0.08) {
            fallos += 1;
        }
        let tiempo = ahora - chrono::Duration::milliseconds((muestras - i) as i64 * 180);
        operaciones.push(Operacion {
            id: format!("demo-ga-{seed}-{i}"),
            compra_en: compra.to_string(),
            venta_en: venta.to_string(),
            par: "BTC/USD".to_string(),
            cantidad_btc: if parcial { cantidad * 0.58 } else { cantidad },
            precio_compra,
            precio_venta,
            utilidad_usd: utilidad,
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
    Oportunidad {
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
            "DEMO_PROFITABLE — operacion sintetica rentable con utilidad {:.2} USD para demostrar flujo end-to-end",
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
    seed: u64,
    ahora: DateTime<Utc>,
) -> Operacion {
    let precio_base = precio_ref.clamp(20_000.0, 250_000.0);
    let requested = costos.max_operacion_btc.clamp(0.08, 0.55);
    let filled = (requested * 0.37).max(0.025);
    let precio_compra = precio_base * 0.9997;
    let mid = precio_base;
    let fee_compra = config_exchange(costos, "Binance").fee_taker;
    let fee_venta = config_exchange(costos, "OKX").fee_taker;
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
        total_usd: 0.0,
    };
    let total_usd = costos_operacion.fee_compra_usd
        + costos_operacion.fee_venta_usd
        + costos_operacion.deslizamiento_usd
        + costos_operacion.retiro_amort_usd
        + costos_operacion.latencia_riesgo_usd;
    Operacion {
        id: format!("demo-partial-{seed}"),
        compra_en: "Binance".to_string(),
        venta_en: "OKX".to_string(),
        par: "BTC/USD".to_string(),
        cantidad_btc: filled,
        precio_compra,
        precio_venta,
        utilidad_usd: ((precio_venta - precio_compra) * filled - total_usd).max(1.0),
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
    let mut w = [0.40, 0.20, 0.20, 0.10, 0.10];
    if pesos.len() >= 5 {
        let total: f64 = pesos.iter().take(5).sum();
        if total > 0.0 {
            for i in 0..5 {
                w[i] = pesos[i] / total;
            }
        }
    }
    let utilidad = (o.utilidad_usd / 100.0).clamp(0.0, 1.0);
    let frescura = if stale_ms > 0 {
        (1.0 - o.latencia_max_ms as f64 / stale_ms as f64).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let liquidez = if max_operacion_btc > 0.0 {
        (o.cantidad_btc / max_operacion_btc).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let confiabilidad = *historial
        .get(&format!("{}->{}", o.compra_en, o.venta_en))
        .unwrap_or(&1.0);
    let z = (z_score / 3.0).clamp(0.0, 1.0);
    w[0] * utilidad + w[1] * frescura + w[2] * liquidez + w[3] * confiabilidad + w[4] * z
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
        media / desv * (252.0_f64 * 24.0 * 60.0).sqrt()
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

fn max_drawdown(serie: &[PuntoSerie]) -> f64 {
    let mut max_pnl = 0.0;
    let mut max_dd = 0.0;
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

fn cotizacion_valida(c: &Cotizacion, ahora: DateTime<Utc>, stale_ms: i64) -> bool {
    if c.exchange.is_empty() || c.bid <= 0.0 || c.ask <= 0.0 || c.bid >= c.ask {
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

fn config_exchange(costos: &MapaCostos, nombre: &str) -> ExchangeConfig {
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
            exchanges,
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
            }],
            asks: vec![NivelOrden {
                precio: ask,
                cantidad: ask_qty,
            }],
            evento_unix_ms: 0,
            recibida_en: Utc::now(),
            latencia_ms: 0,
            secuencia: 0,
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
            id: "1".into(),
            compra_en: "A".into(),
            venta_en: "B".into(),
            par: "BTC/USDT".into(),
            cantidad_btc: 0.1,
            precio_compra: 100.0,
            precio_venta: 110.0,
            utilidad_usd: 1.0,
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
    fn rebalanceo_genera_evento_y_mueve_saldo() {
        let exchanges = vec!["A".to_string(), "B".to_string()];
        let mut carteras = Carteras::new(&exchanges, 20_000.0, 2.0);
        carteras.balances.get_mut("A").unwrap().usd = 100.0;
        carteras.balances.get_mut("B").unwrap().usd = 19_900.0;
        let eventos = carteras.rebalancear(100_000.0, &cfg_test(), Utc::now());
        assert!(!eventos.is_empty());
        assert!(carteras.balance("A").usd > 100.0);
        assert!(eventos.iter().any(|e| e.activo == "USD"));
    }

    #[test]
    fn adversidad_desactivada_no_modifica_operacion() {
        let mut op = Operacion {
            id: "1".into(),
            compra_en: "A".into(),
            venta_en: "B".into(),
            par: "BTC/USDT".into(),
            cantidad_btc: 0.1,
            precio_compra: 100.0,
            precio_venta: 110.0,
            utilidad_usd: 1.0,
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
            id: "1".into(),
            compra_en: "A".into(),
            venta_en: "B".into(),
            par: "BTC/USDT".into(),
            cantidad_btc: 0.1,
            precio_compra: 100.0,
            precio_venta: 110.0,
            utilidad_usd: 1.0,
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

    #[test]
    fn replay_sintetico_genera_muestras_rentables_para_ga() {
        let cfg = cfg_test();
        let replay = operaciones_sinteticas_ga(&cfg, 24, 95_000.0, 7, Utc::now());
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
        assert!(replay.operaciones.iter().all(|op| op.utilidad_usd >= 1.0));
    }

    #[tokio::test]
    async fn demo_rentable_inyecta_operaciones_y_activa_ga() {
        let motor = Motor::new(cfg_test(), 250_000.0, 2.5, "BTC/USD".to_string(), None);
        let resultado = motor
            .activar_escenario_demo(EscenarioDemo::MercadoRentable)
            .await;
        assert!(resultado
            .get("ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(false));

        let estado = motor.estado().await;
        assert!(!estado.operaciones.is_empty());
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
    async fn demo_fill_parcial_deja_evidencia_forense() {
        let motor = Motor::new(cfg_test(), 250_000.0, 2.5, "BTC/USD".to_string(), None);
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
        let motor = Motor::new(cfg_test(), 250_000.0, 2.5, "BTC/USD".to_string(), None);
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
}
