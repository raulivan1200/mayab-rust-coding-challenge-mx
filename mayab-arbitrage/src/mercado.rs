//! Adaptadores WebSocket para feeds públicos de exchanges.
//!
//! Cada adaptador normaliza libros de órdenes a `Cotizacion`. Los parsers son
//! tolerantes a mensajes no relevantes y devuelven `None` cuando el payload no
//! contiene un snapshot útil.

use smallvec::SmallVec;
use std::{collections::BTreeMap, sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use reqwest::Client;
use serde_json::Value;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::{
    motor::Motor,
    tape::{EventKind, IntegrityState, TapeEvent, TapeSource},
    types::{Cotizacion, NivelOrden},
};

#[derive(Clone)]
struct Adaptador {
    nombre: &'static str,
    par: String,
    url: String,
    suscripcion: Option<Value>,
    parser: fn(&[u8], &mut LibroEstado) -> Option<Cotizacion>,
    rest: Option<RestFallback>,
}

#[derive(Clone)]
struct RestFallback {
    url: String,
    parser: fn(&[u8], &str) -> Option<Cotizacion>,
}

/// Contrato común para cualquier feed de exchange (WebSocket + REST fallback).
///
/// Permite agregar venues sin tocar el loop de conexión: basta implementar
/// este trait (o reutilizar [`Adaptador`], que ya lo implementa con los parsers
/// por función) y registrarlo en [`adaptadores`].
pub(crate) trait ExchangeAdapter: Send + Sync {
    /// Nombre canónico del exchange (p. ej. "Binance").
    fn nombre(&self) -> &str;
    /// Par de negociación normalizado (p. ej. "BTC/USD").
    fn par(&self) -> &str;
    /// URL del WebSocket público.
    fn ws_url(&self) -> &str;
    /// Mensaje de suscripción opcional enviado tras conectar.
    fn suscripcion(&self) -> Option<&Value>;
    /// Parsea un frame WebSocket a una [`Cotizacion`] usando el libro incremental.
    fn parse_ws(&self, bytes: &[u8], libro: &mut LibroEstado) -> Option<Cotizacion>;
    /// URL del snapshot REST público de respaldo, si existe.
    fn rest_url(&self) -> Option<&str>;
    /// Parsea un snapshot REST a una [`Cotizacion`].
    fn parse_rest(&self, bytes: &[u8], par: &str) -> Option<Cotizacion>;
    /// Indica si el adapter tiene fallback REST.
    fn tiene_rest(&self) -> bool {
        self.rest_url().is_some()
    }
    /// Clona el adapter detrás de un trait object.
    fn clonar(&self) -> Box<dyn ExchangeAdapter>;
}

impl ExchangeAdapter for Adaptador {
    fn nombre(&self) -> &str {
        self.nombre
    }
    fn par(&self) -> &str {
        &self.par
    }
    fn ws_url(&self) -> &str {
        &self.url
    }
    fn suscripcion(&self) -> Option<&Value> {
        self.suscripcion.as_ref()
    }
    fn parse_ws(&self, bytes: &[u8], libro: &mut LibroEstado) -> Option<Cotizacion> {
        (self.parser)(bytes, libro)
    }
    fn rest_url(&self) -> Option<&str> {
        self.rest.as_ref().map(|r| r.url.as_str())
    }
    fn parse_rest(&self, bytes: &[u8], par: &str) -> Option<Cotizacion> {
        self.rest.as_ref().and_then(|r| (r.parser)(bytes, par))
    }
    fn clonar(&self) -> Box<dyn ExchangeAdapter> {
        Box::new(self.clone())
    }
}

#[derive(Default)]
pub(crate) struct LibroEstado {
    par: String,
    bids: BTreeMap<BookPriceUnits, BookQtyUnits>,
    asks: BTreeMap<BookPriceUnits, BookQtyUnits>,
    ultima_secuencia: Option<u64>,
    integrity_status: String,
    resyncs: u64,
    requiere_snapshot: bool,
    timestamp_confiable: bool,
    checksum_failures: u64,
    sequence_gaps: u64,
    invalidada_desde_ms: Option<i64>,
    kraken_bids_crc: BTreeMap<BookPriceUnits, (String, String)>,
    kraken_asks_crc: BTreeMap<BookPriceUnits, (String, String)>,
}

const BOOK_SCALE: f64 = 100_000_000.0;

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
struct BookPriceUnits(i64);

impl BookPriceUnits {
    fn from_f64(value: f64) -> Self {
        Self((value * BOOK_SCALE).round() as i64)
    }

    fn to_f64(self) -> f64 {
        self.0 as f64 / BOOK_SCALE
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
struct BookQtyUnits(i64);

impl BookQtyUnits {
    fn from_f64(value: f64) -> Self {
        Self((value * BOOK_SCALE).round() as i64)
    }

    fn to_f64(self) -> f64 {
        self.0 as f64 / BOOK_SCALE
    }
}

impl LibroEstado {
    fn new(par: &str) -> Self {
        Self {
            par: normalizar_par(par),
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            ultima_secuencia: None,
            integrity_status: "esperando_snapshot".to_string(),
            resyncs: 0,
            requiere_snapshot: true,
            timestamp_confiable: false,
            checksum_failures: 0,
            sequence_gaps: 0,
            invalidada_desde_ms: None,
            kraken_bids_crc: BTreeMap::new(),
            kraken_asks_crc: BTreeMap::new(),
        }
    }

    fn reset(&mut self, par: &str) {
        self.par = normalizar_par(par);
        self.bids.clear();
        self.asks.clear();
        self.kraken_bids_crc.clear();
        self.kraken_asks_crc.clear();
    }

    fn actualizar_bids(&mut self, niveles: &[NivelOrden]) {
        actualizar_lado(&mut self.bids, niveles);
    }

    fn actualizar_asks(&mut self, niveles: &[NivelOrden]) {
        actualizar_lado(&mut self.asks, niveles);
    }

    fn snapshot_bids(&self, max: usize) -> SmallVec<[NivelOrden; 10]> {
        self.bids
            .iter()
            .rev()
            .take(max)
            .map(|(p, c)| NivelOrden {
                precio: p.to_f64(),
                cantidad: c.to_f64(),
            })
            .collect()
    }

    fn snapshot_asks(&self, max: usize) -> SmallVec<[NivelOrden; 10]> {
        self.asks
            .iter()
            .take(max)
            .map(|(p, c)| NivelOrden {
                precio: p.to_f64(),
                cantidad: c.to_f64(),
            })
            .collect()
    }

    fn cotizacion(&self, evento_unix_ms: i64) -> Option<Cotizacion> {
        let mut cotizacion = cotizacion(
            &self.par,
            self.snapshot_bids(10),
            self.snapshot_asks(10),
            evento_unix_ms,
        )?;
        cotizacion.exchange_sequence = self.ultima_secuencia;
        cotizacion.integrity_status = self.integrity_status.clone();
        cotizacion.resyncs = self.resyncs;
        cotizacion.sequence_gaps = self.sequence_gaps;
        cotizacion.checksum_failures = self.checksum_failures;
        cotizacion.invalidated_ms = self
            .invalidada_desde_ms
            .map(|desde| (Utc::now().timestamp_millis() - desde).max(0))
            .unwrap_or(0);
        cotizacion.timestamp_confiable = self.timestamp_confiable;
        Some(cotizacion)
    }

    fn registrar_snapshot(
        &mut self,
        secuencia: Option<u64>,
        integrity_status: &str,
        timestamp_confiable: bool,
    ) {
        self.ultima_secuencia = secuencia;
        self.integrity_status = integrity_status.to_string();
        self.requiere_snapshot = false;
        self.timestamp_confiable = timestamp_confiable;
        self.invalidada_desde_ms = None;
    }

    fn registrar_incremental_exacto(&mut self, secuencia: Option<u64>) -> bool {
        if self.requiere_snapshot {
            self.integrity_status = "esperando_snapshot".to_string();
            return false;
        }
        let Some(actual) = secuencia else {
            self.integrity_status = "secuencia_no_disponible".to_string();
            return true;
        };
        if let Some(anterior) = self.ultima_secuencia {
            if actual <= anterior {
                self.integrity_status = "fuera_de_orden".to_string();
                return false;
            }
            if actual != anterior.saturating_add(1) {
                self.resyncs += 1;
                self.sequence_gaps += 1;
                self.invalidada_desde_ms = Some(Utc::now().timestamp_millis());
                self.requiere_snapshot = true;
                self.integrity_status = "gap_requiere_snapshot".to_string();
                let par = self.par.clone();
                self.reset(&par);
                self.ultima_secuencia = None;
                return false;
            }
        }
        self.ultima_secuencia = Some(actual);
        self.integrity_status = "secuencia_continua".to_string();
        true
    }

    fn registrar_incremental_monotono(&mut self, secuencia: Option<u64>) -> bool {
        if self.requiere_snapshot {
            self.integrity_status = "esperando_snapshot".to_string();
            return false;
        }
        let Some(actual) = secuencia else {
            self.integrity_status = "secuencia_no_disponible".to_string();
            return true;
        };
        if self
            .ultima_secuencia
            .is_some_and(|anterior| actual < anterior)
        {
            self.integrity_status = "fuera_de_orden".to_string();
            return false;
        }
        self.ultima_secuencia = Some(actual);
        self.integrity_status = "secuencia_monotona".to_string();
        true
    }

    fn registrar_incremental_enlazado(
        &mut self,
        secuencia: Option<u64>,
        previa: Option<u64>,
    ) -> bool {
        if self.requiere_snapshot {
            self.integrity_status = "esperando_snapshot".to_string();
            return false;
        }
        let (Some(actual), Some(previa)) = (secuencia, previa) else {
            self.integrity_status = "secuencia_no_disponible".to_string();
            return true;
        };
        if self
            .ultima_secuencia
            .is_some_and(|anterior| previa != anterior || actual <= anterior)
        {
            self.resyncs += 1;
            self.sequence_gaps += 1;
            self.invalidada_desde_ms = Some(Utc::now().timestamp_millis());
            self.requiere_snapshot = true;
            self.integrity_status = "gap_requiere_snapshot".to_string();
            let par = self.par.clone();
            self.reset(&par);
            self.ultima_secuencia = None;
            return false;
        }
        self.ultima_secuencia = Some(actual);
        self.integrity_status = "secuencia_enlazada".to_string();
        true
    }

    fn validar_checksum_kraken(&mut self, esperado: u32) -> bool {
        let mut entrada = String::new();
        for (_, (precio, cantidad)) in self.kraken_asks_crc.iter().take(10) {
            entrada.push_str(precio);
            entrada.push_str(cantidad);
        }
        for (_, (precio, cantidad)) in self.kraken_bids_crc.iter().rev().take(10) {
            entrada.push_str(precio);
            entrada.push_str(cantidad);
        }
        if crc32fast::hash(entrada.as_bytes()) == esperado {
            self.integrity_status = "checksum_crc32_ok".to_string();
            true
        } else {
            self.checksum_failures += 1;
            self.resyncs += 1;
            self.invalidada_desde_ms = Some(Utc::now().timestamp_millis());
            self.requiere_snapshot = true;
            self.integrity_status = "checksum_crc32_fallido_requiere_snapshot".to_string();
            let par = self.par.clone();
            self.reset(&par);
            false
        }
    }
}

fn kraken_crc_num(value: &str) -> Option<String> {
    if value.starts_with('-') || value.contains(['e', 'E']) {
        return None;
    }
    let compact = value.replace('.', "");
    if compact.is_empty() || !compact.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let stripped = compact.trim_start_matches('0');
    Some(if stripped.is_empty() {
        "0".to_string()
    } else {
        stripped.to_string()
    })
}

/// Lanza una tarea Tokio por cada feed público configurado.
pub async fn start_feeds(motor: Arc<Motor>, par_base: String) {
    let client = Client::builder()
        .timeout(Duration::from_secs(3))
        .user_agent("mayab-arbitrage/0.1 public-market-data")
        .build()
        .unwrap_or_else(|_| Client::new());
    for adaptador in adaptadores(&par_base) {
        if adaptador.tiene_rest() && !adaptador.ws_url().starts_with("solana://") {
            let motor = motor.clone();
            let client = client.clone();
            let rest_adaptador = adaptador.clonar();
            tokio::spawn(async move {
                run_rest_fallback(rest_adaptador, motor, client).await;
            });
        }
        // Lanzar conector DEX (simulado) o WS normal
        if adaptador.ws_url().starts_with("solana://") {
            let motor = motor.clone();
            tokio::spawn(async move {
                run_solana_dex_connector(adaptador, motor).await;
            });
        } else {
            let motor = motor.clone();
            tokio::spawn(async move {
                run_feed(adaptador, motor).await;
            });
        }
    }
}

// Helper para mapear quote según exchange (USD -> USDT para venues spot que no tienen USD)
fn mapear_quote_exchange(exchange: &str, quote: &str) -> String {
    let quote = quote.to_ascii_uppercase();
    if quote == "USD" {
        match exchange {
            "Binance" | "OKX" | "Bybit" | "KuCoin" | "Gate.io" => "USDT".to_string(),
            _ => "USD".to_string(),
        }
    } else {
        quote
    }
}

fn cotizacion_desde_precio_liq(
    par: &str,
    precio: f64,
    liquidez: f64,
    es_jupiter: bool,
) -> Cotizacion {
    let spread_bps = if es_jupiter { 2.0 } else { 5.0 };
    let half_spread = precio * spread_bps / 10000.0 / 2.0;
    let bid = precio - half_spread;
    let ask = precio + half_spread;
    let qty = (liquidez / precio * 0.01).clamp(0.001, 10.0);
    Cotizacion {
        exchange: if es_jupiter { "Jupiter" } else { "Raydium" }.to_string(),
        par: normalizar_par(par),
        bid,
        bid_cantidad: qty,
        ask,
        ask_cantidad: qty,
        bids: SmallVec::from_vec(vec![NivelOrden {
            precio: bid,
            cantidad: qty,
        }]),
        asks: SmallVec::from_vec(vec![NivelOrden {
            precio: ask,
            cantidad: qty,
        }]),
        evento_unix_ms: Utc::now().timestamp_millis(),
        recibida_en: Utc::now(),
        latencia_ms: if es_jupiter { 80 } else { 120 },
        secuencia: 0,
        exchange_sequence: None,
        integrity_status: "amm_rest".to_string(),
        resyncs: 0,
        sequence_gaps: 0,
        checksum_failures: 0,
        invalidated_ms: 0,
        timestamp_confiable: true,
        conectado: false,
        ultimo_mensaje: "rest_fallback".to_string(),
    }
}

/// Tarea periódica que simula consultas RPC a DEXs Solana (Jupiter/Raydium)
/// Genera cotizaciones sintéticas con curvas AMM x*y=k
async fn run_solana_dex_connector(adaptador: Box<dyn ExchangeAdapter>, motor: Arc<Motor>) {
    let nombre = adaptador.nombre().to_string();
    let par = adaptador.par().to_string();
    let mut intervalo = tokio::time::interval(Duration::from_millis(1200));
    let mut rng = StdRng::from_entropy();
    let mut precio_base = 50000.0_f64;
    loop {
        intervalo.tick().await;
        // Simular deriva de precio +/- 0.2% por tick
        let deriva = rng.gen_range(-0.002..0.002);
        precio_base *= 1.0 + deriva;
        precio_base = precio_base.clamp(20_000.0, 200_000.0);

        // Simular order book AMM con x*y=k
        // Reserva virtual: 1000 BTC * 50M USD = 50B constante
        let reserva_btc = 1000.0_f64;
        let reserva_usd = precio_base * reserva_btc;
        // Generar niveles de order book simulando profundidad AMM
        let mut bids = Vec::new();
        let mut asks = Vec::new();
        for i in 0..10 {
            let factor = 1.0 + (i as f64 * 0.001);
            let bid_price = precio_base / factor;
            let ask_price = precio_base * factor;
            // Liquidez decreciente lejos del mid
            let liq_factor = 1.0 / (1.0 + i as f64 * 0.5);
            let bid_qty = (reserva_btc * 0.01 * liq_factor).max(0.001);
            let ask_qty = (reserva_usd * 0.01 / ask_price * liq_factor).max(0.001);
            bids.push(NivelOrden {
                precio: bid_price,
                cantidad: bid_qty,
            });
            asks.push(NivelOrden {
                precio: ask_price,
                cantidad: ask_qty,
            });
        }

        let cotizacion = Cotizacion {
            exchange: nombre.clone(),
            par: par.clone(),
            bid: bids[0].precio,
            bid_cantidad: bids[0].cantidad,
            ask: asks[0].precio,
            ask_cantidad: asks[0].cantidad,
            bids: SmallVec::from_vec(bids),
            asks: SmallVec::from_vec(asks),
            evento_unix_ms: Utc::now().timestamp_millis(),
            recibida_en: Utc::now(),
            latencia_ms: rng.gen_range(60..150),
            secuencia: 0,
            exchange_sequence: None,
            integrity_status: "amm_simulado".to_string(),
            resyncs: 0,
            sequence_gaps: 0,
            checksum_failures: 0,
            invalidated_ms: 0,
            timestamp_confiable: true,
            conectado: true,
            ultimo_mensaje: "amm_simulado".to_string(),
        };
        motor.recibir_cotizacion(cotizacion).await;
    }
}

async fn run_rest_fallback(adaptador: Box<dyn ExchangeAdapter>, motor: Arc<Motor>, client: Client) {
    let nombre = adaptador.nombre().to_string();
    let par = adaptador.par().to_string();
    let inicio_jitter = rand::thread_rng().gen_range(0..=2_000);
    tokio::time::sleep(Duration::from_millis(inicio_jitter)).await;
    let mut backoff = Duration::from_secs(5);
    loop {
        if !motor.feed_necesita_fallback(&nombre, &par).await {
            backoff = Duration::from_secs(5);
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }
        match obtener_rest(adaptador.as_ref(), &client).await {
            Ok(mut cotizacion) => {
                cotizacion.exchange = nombre.clone();
                cotizacion.recibida_en = Utc::now();
                cotizacion.conectado = false;
                cotizacion.ultimo_mensaje = "rest_fallback".to_string();
                motor.recibir_cotizacion(cotizacion).await;
                backoff = Duration::from_secs(5);
                tracing::info!(exchange = nombre, "snapshot REST usado como fallback");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            Err(err) => {
                tracing::warn!(exchange = nombre, error = %err, "fallback REST fallo");
                let jitter =
                    rand::thread_rng().gen_range(0..=backoff.as_millis().max(1) as u64 / 2);
                tokio::time::sleep(backoff + Duration::from_millis(jitter)).await;
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
        }
    }
}

async fn obtener_rest(
    adaptador: &dyn ExchangeAdapter,
    client: &Client,
) -> anyhow::Result<Cotizacion> {
    let Some(url) = adaptador.rest_url() else {
        anyhow::bail!("adaptador sin REST fallback");
    };
    let bytes = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    adaptador
        .parse_rest(&bytes, adaptador.par())
        .ok_or_else(|| anyhow::anyhow!("payload REST sin libro util"))
}

async fn run_feed(adaptador: Box<dyn ExchangeAdapter>, motor: Arc<Motor>) {
    let nombre = adaptador.nombre().to_string();
    let mut backoff = Duration::from_millis(650);
    loop {
        match conectar(adaptador.as_ref(), motor.clone()).await {
            Ok(_) => backoff = Duration::from_millis(650),
            Err(err) => {
                tracing::warn!(exchange = nombre, error = %err, "feed desconectado")
            }
        }
        let jitter = rand::thread_rng().gen_range(0..=backoff.as_millis().max(1) as u64 / 2);
        tokio::time::sleep(backoff + Duration::from_millis(jitter)).await;
        backoff = (backoff * 2).min(Duration::from_secs(8));
    }
}

async fn conectar(adaptador: &dyn ExchangeAdapter, motor: Arc<Motor>) -> anyhow::Result<()> {
    let (mut ws, _) = connect_async(adaptador.ws_url()).await?;
    let mut libro = LibroEstado::new(adaptador.par());
    tracing::info!(exchange = adaptador.nombre(), "feed conectado");
    if let Some(payload) = adaptador.suscripcion() {
        ws.send(Message::Text(payload.to_string())).await?;
    }
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    loop {
        tokio::select! {
            _ = ping.tick() => {
                ws.send(Message::Ping(Vec::new())).await?;
            }
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => recibir(adaptador, text.as_bytes(), &motor, &mut libro).await,
                    Some(Ok(Message::Binary(bytes))) => recibir(adaptador, &bytes, &motor, &mut libro).await,
                    Some(Ok(Message::Ping(payload))) => ws.send(Message::Pong(payload)).await?,
                    Some(Ok(Message::Close(_))) | None => anyhow::bail!("conexion cerrada"),
                    Some(Err(err)) => return Err(err.into()),
                    _ => {}
                }
            }
        }
    }
}

async fn recibir(
    adaptador: &dyn ExchangeAdapter,
    bytes: &[u8],
    motor: &Motor,
    libro: &mut LibroEstado,
) {
    if let Some(mut cotizacion) = adaptador.parse_ws(bytes, libro) {
        cotizacion.exchange = adaptador.nombre().to_string();
        cotizacion.recibida_en = Utc::now();
        motor.recibir_cotizacion(cotizacion).await;
    }
}

fn partes_par(par: &str) -> (String, String) {
    let binding = par.trim().to_ascii_uppercase();
    let partes: Vec<&str> = binding.split('/').collect();
    if partes.len() == 2 {
        (partes[0].to_string(), partes[1].to_string())
    } else {
        ("BTC".to_string(), "USD".to_string())
    }
}

fn adaptadores(par: &str) -> Vec<Box<dyn ExchangeAdapter>> {
    let (base, quote) = partes_par(par);
    // Usar mapeo dinámico por exchange para símbolos y URLs
    let mut lista: Vec<Adaptador> = Vec::new();

    // Binance - usa USDT para spot
    let quote_bn = mapear_quote_exchange("Binance", &quote);
    let binance_symbol = format!("{base}{quote_bn}");
    lista.push(Adaptador {
        nombre: "Binance",
        par: normalizar_par(par),
        url: format!(
            "wss://data-stream.binance.vision/ws/{}@depth10@100ms",
            binance_symbol.to_lowercase()
        ),
        suscripcion: None,
        parser: parsear_binance,
        rest: Some(RestFallback {
            url: format!("https://api.binance.com/api/v3/depth?symbol={binance_symbol}&limit=10"),
            parser: parsear_rest_binance,
        }),
    });

    // Kraken - usa USD nativamente
    let quote_kr = mapear_quote_exchange("Kraken", &quote);
    let slash_symbol = format!("{base}/{quote_kr}");
    lista.push(Adaptador {
        nombre: "Kraken",
        par: normalizar_par(par),
        url: "wss://ws.kraken.com/v2".to_string(),
        suscripcion: Some(
            serde_json::json!({"method":"subscribe","params":{"channel":"book","symbol":[slash_symbol],"depth":10,"snapshot":true}}),
        ),
        parser: parsear_kraken,
        rest: Some(RestFallback {
            url: format!("https://api.kraken.com/0/public/Depth?pair={base}{quote_kr}&count=10"),
            parser: parsear_rest_kraken,
        }),
    });

    // Coinbase - usa USD
    let quote_cb = mapear_quote_exchange("Coinbase", &quote);
    let dash_symbol = format!("{base}-{quote_cb}");
    lista.push(Adaptador {
        nombre: "Coinbase",
        par: normalizar_par(par),
        url: "wss://advanced-trade-ws.coinbase.com".to_string(),
        suscripcion: Some(
            serde_json::json!({"type":"subscribe","product_ids":[dash_symbol],"channel":"level2"}),
        ),
        parser: parsear_coinbase,
        rest: Some(RestFallback {
            url: format!("https://api.exchange.coinbase.com/products/{dash_symbol}/book?level=2"),
            parser: parsear_rest_coinbase,
        }),
    });

    // OKX - usa USDT
    let quote_okx = mapear_quote_exchange("OKX", &quote);
    let dash_symbol_okx = format!("{base}-{quote_okx}");
    lista.push(Adaptador {
        nombre: "OKX",
        par: normalizar_par(par),
        url: "wss://ws.okx.com:8443/ws/v5/public".to_string(),
        suscripcion: Some(
            serde_json::json!({"op":"subscribe","args":[{"channel":"books5","instId":&dash_symbol_okx}]}),
        ),
        parser: parsear_okx,
        rest: Some(RestFallback {
            url: format!("https://www.okx.com/api/v5/market/books?instId={dash_symbol_okx}&sz=10"),
            parser: parsear_rest_okx,
        }),
    });

    // Bybit - usa USDT
    let quote_bybit = mapear_quote_exchange("Bybit", &quote);
    let binance_symbol_bybit = format!("{base}{quote_bybit}");
    lista.push(Adaptador {
        nombre: "Bybit",
        par: normalizar_par(par),
        url: "wss://stream.bybit.com/v5/public/spot".to_string(),
        suscripcion: Some(
            serde_json::json!({"op":"subscribe","args":[format!("orderbook.50.{binance_symbol_bybit}")]}),
        ),
        parser: parsear_bybit,
        rest: Some(RestFallback {
            url: format!(
                "https://api.bybit.com/v5/market/orderbook?category=spot&symbol={binance_symbol_bybit}&limit=10"
            ),
            parser: parsear_rest_bybit,
        }),
    });

    // Bitfinex - usa USD
    let quote_bfx = mapear_quote_exchange("Bitfinex", &quote);
    lista.push(Adaptador {
        nombre: "Bitfinex",
        par: normalizar_par(par),
        url: "wss://api-pub.bitfinex.com/ws/2".to_string(),
        suscripcion: Some(
            serde_json::json!({"event":"subscribe","channel":"book","symbol":format!("t{base}{quote_bfx}"),"prec":"P0","len":10}),
        ),
        parser: parsear_bitfinex,
        rest: Some(RestFallback {
            url: format!("https://api-pub.bitfinex.com/v2/book/t{base}{quote_bfx}/P0?len=10"),
            parser: parsear_rest_bitfinex,
        }),
    });

    // KuCoin - usa USDT
    let quote_ku = mapear_quote_exchange("KuCoin", &quote);
    let dash_symbol_ku = format!("{base}-{quote_ku}");
    lista.push(Adaptador {
        nombre: "KuCoin",
        par: normalizar_par(par),
        url: "wss://ws-api-spot.kucoin.com/".to_string(),
        suscripcion: Some(
            serde_json::json!({"id":1,"type":"subscribe","topic":format!("/market/level2:{}", dash_symbol_ku),"response":true}),
        ),
        parser: parsear_kucoin,
        rest: Some(RestFallback {
            url: format!("https://api.kucoin.com/api/v1/market/orderbook/level2_10?symbol={dash_symbol_ku}"),
            parser: parsear_rest_kucoin,
        }),
    });

    // Gate.io - usa USDT
    let quote_gt = mapear_quote_exchange("Gate.io", &quote);
    let dash_symbol_gt = format!("{base}-{quote_gt}");
    lista.push(Adaptador {
        nombre: "Gate.io",
        par: normalizar_par(par),
        url: "wss://api.gateio.ws/ws/v4/".to_string(),
        suscripcion: Some(
            serde_json::json!({"id":1,"method":"order_book.subscribe","params":[dash_symbol_gt,10,100]}),
        ),
        parser: parsear_gateio,
        rest: Some(RestFallback {
            url: format!("https://api.gateio.ws/api/v4/spot/order_book?currency_pair={dash_symbol_gt}&limit=10"),
            parser: parsear_rest_gateio,
        }),
    });

    // Bitstamp - usa USD
    let quote_bs = mapear_quote_exchange("Bitstamp", &quote);
    let dash_symbol_bs = format!("{base}-{quote_bs}");
    lista.push(Adaptador {
        nombre: "Bitstamp",
        par: normalizar_par(par),
        url: "wss://ws.bitstamp.net".to_string(),
        suscripcion: Some(
            serde_json::json!({"event":"bts:subscribe","data":{"channel":format!("order_book_{}", dash_symbol_bs.to_lowercase())}}),
        ),
        parser: parsear_bitstamp,
        rest: Some(RestFallback {
            url: format!("https://www.bitstamp.net/api/v2/order_book/{}/?group=1", dash_symbol_bs.to_lowercase()),
            parser: parsear_rest_bitstamp,
        }),
    });

    // Gemini - usa USD
    let quote_gm = mapear_quote_exchange("Gemini", &quote);
    let dash_symbol_gm = format!("{base}-{quote_gm}");
    lista.push(Adaptador {
        nombre: "Gemini",
        par: normalizar_par(par),
        url: format!("wss://api.gemini.com/v1/marketdata/{}", dash_symbol_gm),
        suscripcion: None,
        parser: parsear_gemini,
        rest: Some(RestFallback {
            url: format!("https://api.gemini.com/v1/book/{dash_symbol_gm}"),
            parser: parsear_rest_gemini,
        }),
    });

    // DEX: Jupiter (Solana) - simula order book AMM
    lista.push(Adaptador {
        nombre: "Jupiter",
        par: normalizar_par(par),
        url: "solana://jupiter".to_string(), // placeholder, se usa run_solana_dex_connector
        suscripcion: None,
        parser: parsear_jupiter,
        rest: Some(RestFallback {
            url: "https://quote-api.jup.ag/v6/quote".to_string(),
            parser: parsear_rest_jupiter,
        }),
    });

    // DEX: Raydium (Solana) - simula order book AMM
    lista.push(Adaptador {
        nombre: "Raydium",
        par: normalizar_par(par),
        url: "solana://raydium".to_string(),
        suscripcion: None,
        parser: parsear_raydium,
        rest: Some(RestFallback {
            url: "https://api.raydium.io/v2/sdk/liquidity/mainnet.json".to_string(),
            parser: parsear_rest_raydium,
        }),
    });

    lista
        .into_iter()
        .map(|a| Box::new(a) as Box<dyn ExchangeAdapter>)
        .collect()
}

/// Captura los libros normalizados de los CEX solicitados sin arrancar el motor.
/// Cada frame útil se persiste como un snapshot autocontenido del top del libro;
/// esto conserva la semántica incremental del venue (secuencias e integridad),
/// pero hace que el tape siga siendo reconstruible aun si empieza a media sesión.
pub async fn capture_public_books(
    par: String,
    exchanges: Vec<String>,
    depth: usize,
    tx: tokio::sync::mpsc::Sender<TapeEvent>,
) -> anyhow::Result<()> {
    let wanted: std::collections::HashSet<String> =
        exchanges.iter().map(|v| v.to_ascii_lowercase()).collect();
    let selected: Vec<_> = adaptadores(&par)
        .into_iter()
        .filter(|a| wanted.contains(&a.nombre().to_ascii_lowercase()))
        .filter(|a| !a.ws_url().starts_with("solana://"))
        .collect();
    if selected.len() != wanted.len() {
        anyhow::bail!("uno o más exchanges no existen o no son CEX públicos");
    }
    for adapter in selected {
        let tx = tx.clone();
        tokio::spawn(async move {
            let name = adapter.nombre().to_string();
            let mut connection_epoch = 0_u64;
            loop {
                if tx.is_closed() {
                    break;
                }
                if let Err(error) =
                    capture_connection(adapter.as_ref(), depth, connection_epoch, &tx).await
                {
                    tracing::warn!(exchange = %name, %error, "captura WS reconectando");
                }
                if tx.is_closed() {
                    break;
                }
                connection_epoch = connection_epoch.saturating_add(1);
                tokio::time::sleep(Duration::from_millis(750)).await;
            }
        });
    }
    Ok(())
}

async fn capture_connection(
    adapter: &dyn ExchangeAdapter,
    depth: usize,
    connection_epoch: u64,
    tx: &tokio::sync::mpsc::Sender<TapeEvent>,
) -> anyhow::Result<()> {
    let (mut ws, _) = connect_async(adapter.ws_url()).await?;
    if let Some(payload) = adapter.suscripcion() {
        ws.send(Message::Text(payload.to_string())).await?;
    }
    let mut book = LibroEstado::new(adapter.par());
    let mut previous_sequence = None;
    let mut previous_resyncs = 0;
    let mut first_useful_event = true;
    while let Some(message) = ws.next().await {
        let bytes = match message? {
            Message::Text(text) => text.as_bytes().to_vec(),
            Message::Binary(bytes) => bytes,
            Message::Ping(payload) => {
                ws.send(Message::Pong(payload)).await?;
                continue;
            }
            Message::Close(_) => anyhow::bail!("conexión cerrada"),
            _ => continue,
        };
        let local = Utc::now();
        let Some(quote) = adapter.parse_ws(&bytes, &mut book) else {
            continue;
        };
        let exchange_ms = (quote.evento_unix_ms > 0).then_some(quote.evento_unix_ms);
        let reconnected = connection_epoch > 0 && first_useful_event;
        // Una conexión nueva rompe continuidad aunque el primer payload sea un
        // snapshot válido. Se marca como gap/resync para que el verificador no
        // compare secuencias pertenecientes a sesiones distintas.
        let gap = book.resyncs > previous_resyncs || reconnected;
        let event = TapeEvent {
            schema_version: 1,
            exchange_timestamp: exchange_ms.and_then(DateTime::<Utc>::from_timestamp_millis),
            local_timestamp: local,
            exchange: adapter.nombre().to_string(),
            pair: quote.par,
            source: TapeSource::WebSocket {
                url: adapter.ws_url().to_string(),
            },
            kind: EventKind::Snapshot,
            sequence_id: book.ultima_secuencia,
            previous_sequence,
            bids: book.snapshot_bids(depth.clamp(10, 50)).into_vec(),
            asks: book.snapshot_asks(depth.clamp(10, 50)).into_vec(),
            integrity: IntegrityState {
                status: book.integrity_status.clone(),
                gap,
                resync: gap,
                connection_epoch,
                reconnected,
            },
            observed_latency_ms: exchange_ms
                .map(|ts| local.timestamp_millis().saturating_sub(ts).max(0) as u64),
        };
        previous_sequence = book.ultima_secuencia.or(previous_sequence);
        previous_resyncs = book.resyncs;
        first_useful_event = false;
        if tx.send(event).await.is_err() {
            return Ok(());
        }
    }
    anyhow::bail!("stream terminado")
}

fn parsear_binance(bytes: &[u8], libro: &mut LibroEstado) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let bids = niveles_strings(v.get("b").or_else(|| v.get("bids"))?.as_array()?, 10);
    let asks = niveles_strings(v.get("a").or_else(|| v.get("asks"))?.as_array()?, 10);
    if v.get("lastUpdateId").is_some() {
        let par_actual = libro.par.clone();
        libro.reset(&par_actual);
        libro.registrar_snapshot(
            v.get("lastUpdateId").and_then(Value::as_u64),
            "snapshot_parcial",
            v.get("E").is_some(),
        );
    }
    libro.actualizar_bids(&bids);
    libro.actualizar_asks(&asks);
    libro.cotizacion(v.get("E").and_then(Value::as_i64).unwrap_or_default())
}

fn parsear_okx(bytes: &[u8], libro: &mut LibroEstado) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    if !v
        .pointer("/arg/channel")
        .and_then(Value::as_str)?
        .starts_with("books")
    {
        return None;
    }
    let item = v.get("data")?.as_array()?.first()?;
    let par = v
        .pointer("/arg/instId")
        .and_then(Value::as_str)
        .unwrap_or("BTC-USDT");
    if v.get("action").and_then(Value::as_str) == Some("snapshot")
        || v.pointer("/arg/channel").and_then(Value::as_str) == Some("books5")
    {
        libro.reset(par);
        libro.registrar_snapshot(item.get("seqId").and_then(parse_u64), "snapshot_seq", true);
    } else {
        libro.par = normalizar_par(par);
        if !libro.registrar_incremental_enlazado(
            item.get("seqId").and_then(parse_u64),
            item.get("prevSeqId").and_then(parse_u64),
        ) {
            return None;
        }
    }
    // Los deltas pueden modificar un solo lado del libro. Una clave ausente no
    // invalida el mensaje: equivale a "sin cambios" para ese lado.
    let bids = item
        .get("bids")
        .and_then(Value::as_array)
        .map(|niveles| niveles_strings(niveles, 10))
        .unwrap_or_default();
    let asks = item
        .get("asks")
        .and_then(Value::as_array)
        .map(|niveles| niveles_strings(niveles, 10))
        .unwrap_or_default();
    let ts = item
        .get("ts")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or_default();
    libro.actualizar_bids(&bids);
    libro.actualizar_asks(&asks);
    libro.cotizacion(ts)
}

fn parsear_bybit(bytes: &[u8], libro: &mut LibroEstado) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let topic = v.get("topic").and_then(Value::as_str)?;
    if !topic.starts_with("orderbook.") {
        return None;
    }
    let par = topic.rsplit('.').next().unwrap_or("BTCUSDT");
    let secuencia = v.pointer("/data/seq").and_then(parse_u64);
    if v.get("type").and_then(Value::as_str) == Some("snapshot") {
        libro.reset(par);
        libro.registrar_snapshot(secuencia, "snapshot_seq", true);
    } else {
        libro.par = normalizar_par(par);
        if !libro.registrar_incremental_monotono(secuencia) {
            return None;
        }
    }
    let bids = v
        .pointer("/data/b")
        .and_then(Value::as_array)
        .map(|niveles| niveles_strings(niveles, 10))
        .unwrap_or_default();
    let asks = v
        .pointer("/data/a")
        .and_then(Value::as_array)
        .map(|niveles| niveles_strings(niveles, 10))
        .unwrap_or_default();
    libro.actualizar_bids(&bids);
    libro.actualizar_asks(&asks);
    libro.cotizacion(v.get("ts").and_then(Value::as_i64).unwrap_or_default())
}

fn parsear_kraken(bytes: &[u8], libro: &mut LibroEstado) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    if v.get("channel").and_then(Value::as_str)? != "book" {
        return None;
    }
    let item = v.get("data")?.as_array()?.first()?;
    let par = item
        .get("symbol")
        .and_then(Value::as_str)
        .unwrap_or("BTC/USD");
    if item.get("type").and_then(Value::as_str) == Some("snapshot")
        || v.get("type").and_then(Value::as_str) == Some("snapshot")
    {
        libro.reset(par);
        libro.registrar_snapshot(None, "checksum_pendiente", true);
    } else {
        if libro.requiere_snapshot {
            libro.integrity_status = "esperando_snapshot".to_string();
            return None;
        }
        libro.par = normalizar_par(par);
        libro.timestamp_confiable = true;
    }
    let bids_raw = item
        .get("bids")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let asks_raw = item
        .get("asks")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let bids = niveles_kraken(bids_raw, &mut libro.kraken_bids_crc)?;
    let asks = niveles_kraken(asks_raw, &mut libro.kraken_asks_crc)?;
    let ts = item
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(rfc3339_ms)
        .unwrap_or_default();
    libro.actualizar_bids(&bids);
    libro.actualizar_asks(&asks);
    if let Some(checksum) = item.get("checksum").and_then(parse_u64) {
        if !libro.validar_checksum_kraken(checksum as u32) {
            return None;
        }
    } else {
        libro.checksum_failures += 1;
        libro.resyncs += 1;
        libro.invalidada_desde_ms = Some(Utc::now().timestamp_millis());
        libro.requiere_snapshot = true;
        libro.integrity_status = "checksum_ausente_requiere_snapshot".to_string();
        let par = libro.par.clone();
        libro.reset(&par);
        return None;
    }
    libro.cotizacion(ts)
}

fn parsear_coinbase(bytes: &[u8], libro: &mut LibroEstado) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    if !matches!(
        v.get("channel").and_then(Value::as_str)?,
        "level2" | "l2_data"
    ) {
        return None;
    }
    let ts = v
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(rfc3339_ms)
        .unwrap_or_default();
    let secuencia = v.get("sequence_num").and_then(parse_u64);
    let es_snapshot = v
        .get("events")?
        .as_array()?
        .iter()
        .any(|event| event.get("type").and_then(Value::as_str) == Some("snapshot"));
    if es_snapshot {
        libro.registrar_snapshot(secuencia, "snapshot_seq", true);
    } else if !libro.registrar_incremental_exacto(secuencia) {
        return None;
    }
    for event in v.get("events")?.as_array()? {
        if let Some(product_id) = event.get("product_id").and_then(Value::as_str) {
            if event.get("type").and_then(Value::as_str) == Some("snapshot") {
                libro.reset(product_id);
            } else {
                libro.par = normalizar_par(product_id);
            }
        }
        for update in event.get("updates")?.as_array()? {
            let product_id = update
                .get("product_id")
                .or_else(|| event.get("product_id"))
                .and_then(Value::as_str)
                .unwrap_or("BTC-USD");
            libro.par = normalizar_par(product_id);
            let precio = update
                .get("price_level")
                .or_else(|| update.get("price"))
                .and_then(parse_num)?;
            let cantidad = update
                .get("new_quantity")
                .or_else(|| update.get("quantity"))
                .and_then(parse_num)?;
            let nivel = NivelOrden { precio, cantidad };
            match update.get("side").and_then(Value::as_str) {
                Some("bid") | Some("BUY") | Some("buy") => libro.actualizar_bids(&[nivel]),
                Some("offer") | Some("ask") | Some("SELL") | Some("sell") => {
                    libro.actualizar_asks(&[nivel])
                }
                _ => {}
            }
        }
    }
    libro.cotizacion(ts)
}

fn parsear_rest_binance(bytes: &[u8], par: &str) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let bids = niveles_strings(v.get("bids")?.as_array()?, 10);
    let asks = niveles_strings(v.get("asks")?.as_array()?, 10);
    marcar_rest(cotizacion(par, bids.into(), asks.into(), 0), false)
}

fn parsear_rest_kraken(bytes: &[u8], par: &str) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    if !v.get("error")?.as_array()?.is_empty() {
        return None;
    }
    let book = v.get("result")?.as_object()?.values().next()?;
    let bids = niveles_strings(book.get("bids")?.as_array()?, 10);
    let asks = niveles_strings(book.get("asks")?.as_array()?, 10);
    marcar_rest(cotizacion(par, bids.into(), asks.into(), 0), false)
}

fn parsear_rest_coinbase(bytes: &[u8], par: &str) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let bids = niveles_strings(v.get("bids")?.as_array()?, 10);
    let asks = niveles_strings(v.get("asks")?.as_array()?, 10);
    let ts = v
        .get("time")
        .and_then(Value::as_str)
        .and_then(rfc3339_ms)
        .unwrap_or_default();
    marcar_rest(cotizacion(par, bids.into(), asks.into(), ts), ts > 0)
}

fn parsear_rest_okx(bytes: &[u8], par: &str) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    if v.get("code").and_then(Value::as_str) != Some("0") {
        return None;
    }
    let book = v.get("data")?.as_array()?.first()?;
    let bids = niveles_strings(book.get("bids")?.as_array()?, 10);
    let asks = niveles_strings(book.get("asks")?.as_array()?, 10);
    let ts = book
        .get("ts")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or_else(|| Utc::now().timestamp_millis());
    marcar_rest(cotizacion(par, bids.into(), asks.into(), ts), true)
}

fn parsear_rest_bybit(bytes: &[u8], par: &str) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    if v.get("retCode").and_then(Value::as_i64) != Some(0) {
        return None;
    }
    let result = v.get("result")?;
    let bids = niveles_strings(result.get("b")?.as_array()?, 10);
    let asks = niveles_strings(result.get("a")?.as_array()?, 10);
    let ts = result
        .get("ts")
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
        })
        .or_else(|| v.get("time").and_then(Value::as_i64))
        .unwrap_or_else(|| Utc::now().timestamp_millis());
    marcar_rest(cotizacion(par, bids.into(), asks.into(), ts), true)
}

fn parsear_bitfinex(bytes: &[u8], libro: &mut LibroEstado) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    if v.is_array() {
        let arr = v.as_array()?;
        if arr.len() < 2 {
            return None;
        }
        let _channel_id = arr[0].as_i64()?;
        let data = &arr[1];
        if data.is_string() && data.as_str() == Some("hb") {
            return None;
        }
        if let Some(snapshot_data) = data.as_array() {
            if snapshot_data.is_empty() {
                return None;
            }
            let is_snapshot = snapshot_data[0].as_str() == Some("snapshot");
            if is_snapshot {
                libro.registrar_snapshot(None, "snapshot", true);
                for level in snapshot_data.iter().skip(1) {
                    let level = level.as_array()?;
                    let price = parse_num(level.first()?)?;
                    let amount = parse_num(level.get(2)?)?;
                    if amount > 0.0 {
                        libro.actualizar_bids(&[NivelOrden {
                            precio: price,
                            cantidad: amount,
                        }]);
                    } else {
                        libro.actualizar_asks(&[NivelOrden {
                            precio: price,
                            cantidad: amount.abs(),
                        }]);
                    }
                }
            } else {
                // Delta: [price, count, amount]
                let price = parse_num(snapshot_data.first()?)?;
                let count = snapshot_data.get(1)?.as_i64()?;
                let amount = parse_num(snapshot_data.get(2)?)?;
                if count == 0 {
                    if amount == 1.0 {
                        libro.actualizar_bids(&[]);
                    } else {
                        libro.actualizar_asks(&[]);
                    }
                } else if amount > 0.0 {
                    libro.actualizar_bids(&[NivelOrden {
                        precio: price,
                        cantidad: amount,
                    }]);
                } else {
                    libro.actualizar_asks(&[NivelOrden {
                        precio: price,
                        cantidad: amount.abs(),
                    }]);
                }
            }
            libro.cotizacion(Utc::now().timestamp_millis())
        } else {
            None
        }
    } else {
        None
    }
}

fn parsear_rest_bitfinex(bytes: &[u8], par: &str) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let arr = v.as_array()?;
    let mut bids = Vec::new();
    let mut asks = Vec::new();
    for level in arr {
        let level = level.as_array()?;
        let price = parse_num(level.first()?)?;
        let amount = parse_num(level.get(2)?)?;
        if amount > 0.0 {
            bids.push(NivelOrden {
                precio: price,
                cantidad: amount,
            });
        } else {
            asks.push(NivelOrden {
                precio: price,
                cantidad: amount.abs(),
            });
        }
    }
    bids.sort_by(|a, b| b.precio.partial_cmp(&a.precio).unwrap());
    asks.sort_by(|a, b| a.precio.partial_cmp(&b.precio).unwrap());
    marcar_rest(
        cotizacion(
            par,
            bids.into_iter().take(10).collect(),
            asks.into_iter().take(10).collect(),
            0,
        ),
        false,
    )
}

fn parsear_kucoin(bytes: &[u8], libro: &mut LibroEstado) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    if v.get("type").and_then(Value::as_str) == Some("welcome") {
        return None;
    }
    let subject = v.get("subject").and_then(Value::as_str)?;
    let data = v.get("data")?;
    let secuencia = data.get("sequence").and_then(Value::as_u64);
    let is_snapshot = subject == "trade.l2snapshot";
    if is_snapshot {
        libro.reset("BTC/USD");
        libro.registrar_snapshot(secuencia, "snapshot_seq", true);
    } else if !libro.registrar_incremental_monotono(secuencia) {
        return None;
    }
    let bids = niveles_strings(data.get("bids")?.as_array()?, 10);
    let asks = niveles_strings(data.get("asks")?.as_array()?, 10);
    libro.actualizar_bids(&bids);
    libro.actualizar_asks(&asks);
    libro.cotizacion(
        data.get("timestamp")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
    )
}

fn parsear_rest_kucoin(bytes: &[u8], par: &str) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    if v.get("code").and_then(Value::as_str) != Some("200000") {
        return None;
    }
    let data = v.get("data")?;
    let bids = niveles_strings(data.get("bids")?.as_array()?, 10);
    let asks = niveles_strings(data.get("asks")?.as_array()?, 10);
    let ts = data.get("time").and_then(Value::as_i64).unwrap_or_default();
    marcar_rest(cotizacion(par, bids.into(), asks.into(), ts), ts > 0)
}

fn parsear_gateio(bytes: &[u8], libro: &mut LibroEstado) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    if v.get("method").and_then(Value::as_str) != Some("order_book.update") {
        return None;
    }
    let data = v.get("params")?.as_array()?.get(3)?.as_object()?;
    let bids = niveles_strings(data.get("bids")?.as_array()?, 10);
    let asks = niveles_strings(data.get("asks")?.as_array()?, 10);
    let ts = v
        .get("params")?
        .as_array()?
        .get(1)?
        .as_i64()
        .unwrap_or_else(|| Utc::now().timestamp_millis());
    libro.actualizar_bids(&bids);
    libro.actualizar_asks(&asks);
    libro.cotizacion(ts)
}

fn parsear_rest_gateio(bytes: &[u8], par: &str) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let data = v.as_array()?.first()?;
    let bids = niveles_strings(data.get("bids")?.as_array()?, 10);
    let asks = niveles_strings(data.get("asks")?.as_array()?, 10);
    let ts = data.get("t").and_then(Value::as_i64).unwrap_or_default();
    marcar_rest(cotizacion(par, bids.into(), asks.into(), ts), ts > 0)
}

fn parsear_bitstamp(bytes: &[u8], libro: &mut LibroEstado) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let event = v.get("event").and_then(Value::as_str)?;
    if event == "bts:subscription_succeeded" {
        return None;
    }
    let data = v.get("data")?;
    let bids = niveles_strings(data.get("bids")?.as_array()?, 10);
    let asks = niveles_strings(data.get("asks")?.as_array()?, 10);
    let ts = data
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or_else(|| Utc::now().timestamp_millis());
    libro.actualizar_bids(&bids);
    libro.actualizar_asks(&asks);
    libro.cotizacion(ts)
}

fn parsear_rest_bitstamp(bytes: &[u8], par: &str) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let bids = niveles_strings(v.get("bids")?.as_array()?, 10);
    let asks = niveles_strings(v.get("asks")?.as_array()?, 10);
    let ts = v
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or_else(|| Utc::now().timestamp_millis());
    marcar_rest(cotizacion(par, bids.into(), asks.into(), ts), ts > 0)
}

fn parsear_gemini(bytes: &[u8], libro: &mut LibroEstado) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let events = v.as_array()?;
    for event in events {
        let event_type = event.get("type").and_then(Value::as_str)?;
        let price = parse_num(event.get("price")?)?;
        let amount = parse_num(event.get("remaining")?)?;
        let side = event.get("side").and_then(Value::as_str)?;
        if side == "bid" {
            libro.actualizar_bids(&[NivelOrden {
                precio: price,
                cantidad: amount,
            }]);
        } else if side == "ask" {
            libro.actualizar_asks(&[NivelOrden {
                precio: price,
                cantidad: amount,
            }]);
        }
        if event_type == "update" || event_type == "snapshot" {
            // El heartbeat no contiene datos de mercado.
        }
    }
    libro.cotizacion(Utc::now().timestamp_millis())
}

fn parsear_rest_gemini(bytes: &[u8], par: &str) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let bids = niveles_strings(v.get("bids")?.as_array()?, 10);
    let asks = niveles_strings(v.get("asks")?.as_array()?, 10);
    let ts = Utc::now().timestamp_millis();
    marcar_rest(cotizacion(par, bids.into(), asks.into(), ts), false)
}

/// Parser REST para Jupiter (Simulado - API real: quote-api.jup.ag)
fn parsear_rest_jupiter(bytes: &[u8], par: &str) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let price = v.get("data")?.get("price")?.as_str()?.parse::<f64>().ok()?;
    let liq = v
        .get("data")?
        .get("liquidity")?
        .as_str()?
        .parse::<f64>()
        .ok()?;
    Some(cotizacion_desde_precio_liq(par, price, liq, true))
}

/// Parser REST para Raydium (Simulado - API real: api.raydium.io)
fn parsear_rest_raydium(bytes: &[u8], par: &str) -> Option<Cotizacion> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let pools = v.get("official")?.as_array()?;
    for pool in pools {
        let mint_a = pool.get("mintA")?.get("address")?.as_str()?;
        let mint_b = pool.get("mintB")?.get("address")?.as_str()?;
        if mint_a.contains("BTC") || mint_b.contains("BTC") {
            let price = pool.get("price")?.as_f64()?;
            let liq = pool.get("liquidity")?.as_f64()?;
            return Some(cotizacion_desde_precio_liq(par, price, liq, false));
        }
    }
    None
}

/// Parser WS placeholder para Jupiter (usa run_solana_dex_connector en su lugar)
fn parsear_jupiter(_bytes: &[u8], _libro: &mut LibroEstado) -> Option<Cotizacion> {
    None
}

/// Parser WS placeholder para Raydium (usa run_solana_dex_connector en su lugar)
fn parsear_raydium(_bytes: &[u8], _libro: &mut LibroEstado) -> Option<Cotizacion> {
    None
}

fn marcar_rest(cotizacion: Option<Cotizacion>, timestamp_confiable: bool) -> Option<Cotizacion> {
    cotizacion.map(|mut cotizacion| {
        cotizacion.integrity_status = "rest_snapshot".to_string();
        cotizacion.timestamp_confiable = timestamp_confiable;
        cotizacion
    })
}

fn niveles_strings(items: &[Value], max: usize) -> Vec<NivelOrden> {
    items
        .iter()
        .take(max)
        .filter_map(|item| {
            let arr = item.as_array()?;
            let precio = parse_num(arr.first()?)?;
            let cantidad = parse_num(arr.get(1)?)?;
            (precio > 0.0).then_some(NivelOrden { precio, cantidad })
        })
        .collect()
}

fn niveles_kraken(
    items: &[Value],
    crc: &mut BTreeMap<BookPriceUnits, (String, String)>,
) -> Option<Vec<NivelOrden>> {
    let mut niveles = Vec::with_capacity(items.len());
    for item in items {
        let price = item.get("price")?;
        let qty = item.get("qty")?;
        let price_text = decimal_text(price)?;
        let qty_text = decimal_text(qty)?;
        let precio = parse_num(price)?;
        let cantidad = parse_num(qty)?;
        if precio <= 0.0 {
            continue;
        }
        let key = BookPriceUnits::from_f64(precio);
        if cantidad <= 0.0 {
            crc.remove(&key);
        } else {
            crc.insert(
                key,
                (kraken_crc_num(&price_text)?, kraken_crc_num(&qty_text)?),
            );
        }
        niveles.push(NivelOrden { precio, cantidad });
    }
    Some(niveles)
}

fn decimal_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn parse_num(v: &Value) -> Option<f64> {
    match v {
        Value::String(s) => s.parse::<f64>().ok(),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

fn parse_u64(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_i64().and_then(|n| u64::try_from(n).ok()))
        .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
}

fn actualizar_lado(lado: &mut BTreeMap<BookPriceUnits, BookQtyUnits>, niveles: &[NivelOrden]) {
    for nivel in niveles {
        if !nivel.precio.is_finite() {
            continue;
        }
        let precio = BookPriceUnits::from_f64(nivel.precio);
        if precio.0 <= 0 {
            continue;
        }
        if nivel.cantidad <= 0.0 || !nivel.cantidad.is_finite() {
            lado.remove(&precio);
        } else {
            lado.insert(precio, BookQtyUnits::from_f64(nivel.cantidad));
        }
    }
}

fn cotizacion(
    par: &str,
    bids: SmallVec<[NivelOrden; 10]>,
    asks: SmallVec<[NivelOrden; 10]>,
    evento_unix_ms: i64,
) -> Option<Cotizacion> {
    let bid = bids.first()?;
    let ask = asks.first()?;
    Some(Cotizacion {
        exchange: String::new(),
        par: normalizar_par(par),
        bid: (bid.precio),
        bid_cantidad: (bid.cantidad),
        ask: (ask.precio),
        ask_cantidad: (ask.cantidad),
        bids,
        asks,
        evento_unix_ms,
        recibida_en: Utc::now(),
        latencia_ms: 0,
        secuencia: 0,
        exchange_sequence: None,
        integrity_status: "sin_validar".to_string(),
        resyncs: 0,
        sequence_gaps: 0,
        checksum_failures: 0,
        invalidated_ms: 0,
        timestamp_confiable: evento_unix_ms > 0,
        conectado: true,
        ultimo_mensaje: String::new(),
    })
}

fn normalizar_par(par: &str) -> String {
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

fn rfc3339_ms(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|t| t.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_binance_depth() {
        let msg = br#"{"E":1710000000000,"b":[["100.0","2.0"]],"a":[["101.0","1.5"]]}"#;
        let mut libro = LibroEstado::new("BTC/USD");
        let c = parsear_binance(msg, &mut libro).unwrap();
        assert_eq!(c.par, "BTC/USD");
        assert_eq!(c.bid, (100.0));
        assert_eq!(c.ask, (101.0));
    }

    #[test]
    fn parsea_coinbase_l2_data() {
        let msg = br#"{"channel":"l2_data","timestamp":"2024-03-09T00:00:00Z","events":[{"type":"snapshot","product_id":"BTC-USD","updates":[{"side":"bid","price_level":"100.0","new_quantity":"2.0"},{"side":"offer","price_level":"101.0","new_quantity":"1.5"}]}]}"#;
        let mut libro = LibroEstado::new("BTC/USD");
        let c = parsear_coinbase(msg, &mut libro).unwrap();
        assert_eq!(c.par, "BTC/USD");
        assert_eq!(c.bid, (100.0));
        assert_eq!(c.ask, (101.0));
    }

    #[test]
    fn parsea_bybit_orderbook_con_profundidad_en_topic() {
        let msg = br#"{"topic":"orderbook.1.BTCUSDT","type":"snapshot","ts":1710000000000,"data":{"b":[["100.0","2.0"]],"a":[["101.0","1.5"]]}}"#;
        let mut libro = LibroEstado::new("BTC/USD");
        let c = parsear_bybit(msg, &mut libro).unwrap();
        assert_eq!(c.par, "BTC/USD");
        assert_eq!(c.bid, (100.0));
        assert_eq!(c.ask, (101.0));
    }

    #[test]
    fn bybit_conserva_ask_en_delta_solo_bid() {
        let snapshot = br#"{"topic":"orderbook.50.BTCUSDT","type":"snapshot","ts":1710000000000,"data":{"b":[["100.0","2.0"]],"a":[["101.0","1.5"]]}}"#;
        let delta = br#"{"topic":"orderbook.50.BTCUSDT","type":"delta","ts":1710000000001,"data":{"b":[["100.5","3.0"]]}}"#;
        let mut libro = LibroEstado::new("BTC/USD");
        parsear_bybit(snapshot, &mut libro).unwrap();

        let c = parsear_bybit(delta, &mut libro).unwrap();

        assert_eq!(c.bid, (100.5));
        assert_eq!(c.ask, (101.0));
    }

    #[test]
    fn bybit_descarta_cross_sequence_fuera_de_orden() {
        let snapshot = br#"{"topic":"orderbook.50.BTCUSDT","type":"snapshot","ts":1710000000000,"data":{"seq":100,"b":[["100.0","2.0"]],"a":[["101.0","1.5"]]}}"#;
        let atrasado = br#"{"topic":"orderbook.50.BTCUSDT","type":"delta","ts":1710000000001,"data":{"seq":99,"b":[["100.5","3.0"]]}}"#;
        let mut libro = LibroEstado::new("BTC/USD");
        parsear_bybit(snapshot, &mut libro).unwrap();

        assert!(parsear_bybit(atrasado, &mut libro).is_none());
        assert_eq!(libro.integrity_status, "fuera_de_orden");
    }

    #[test]
    fn coinbase_gap_bloquea_deltas_hasta_nuevo_snapshot() {
        let snapshot = br#"{"channel":"l2_data","sequence_num":10,"timestamp":"2024-03-09T00:00:00Z","events":[{"type":"snapshot","product_id":"BTC-USD","updates":[{"side":"bid","price_level":"100.0","new_quantity":"2.0"},{"side":"offer","price_level":"101.0","new_quantity":"1.5"}]}]}"#;
        let gap = br#"{"channel":"l2_data","sequence_num":12,"timestamp":"2024-03-09T00:00:00.001Z","events":[{"type":"update","product_id":"BTC-USD","updates":[{"side":"bid","price_level":"100.5","new_quantity":"2.0"}]}]}"#;
        let mut libro = LibroEstado::new("BTC/USD");
        parsear_coinbase(snapshot, &mut libro).unwrap();

        assert!(parsear_coinbase(gap, &mut libro).is_none());
        assert_eq!(libro.integrity_status, "gap_requiere_snapshot");
        assert_eq!(libro.resyncs, 1);
        assert!(libro.bids.is_empty());
    }

    #[test]
    fn kraken_conserva_bid_en_delta_solo_ask() {
        let snapshot_crc = crc32fast::hash(b"101015100020");
        let delta_crc = crc32fast::hash(b"100810101015100020");
        let snapshot = format!(
            r#"{{"channel":"book","type":"snapshot","data":[{{"symbol":"BTC/USD","bids":[{{"price":100.0,"qty":2.0}}],"asks":[{{"price":101.0,"qty":1.5}}],"checksum":{snapshot_crc},"timestamp":"2024-03-09T00:00:00Z"}}]}}"#
        );
        let delta = format!(
            r#"{{"channel":"book","type":"update","data":[{{"symbol":"BTC/USD","asks":[{{"price":100.8,"qty":1.0}}],"checksum":{delta_crc},"timestamp":"2024-03-09T00:00:00.001Z"}}]}}"#
        );
        let mut libro = LibroEstado::new("BTC/USD");
        parsear_kraken(snapshot.as_bytes(), &mut libro).unwrap();

        let c = parsear_kraken(delta.as_bytes(), &mut libro).unwrap();

        assert_eq!(c.bid, (100.0));
        assert_eq!(c.ask, (100.8));
    }

    #[test]
    fn kraken_valida_crc32_y_resincroniza_si_falla() {
        let mut esperado = LibroEstado::new("BTC/USD");
        esperado.actualizar_bids(&[NivelOrden {
            precio: 100.0,
            cantidad: 2.0,
        }]);
        esperado.actualizar_asks(&[NivelOrden {
            precio: 101.0,
            cantidad: 1.5,
        }]);
        let mut entrada = String::new();
        entrada.push_str(&kraken_crc_num("101.0").unwrap());
        entrada.push_str(&kraken_crc_num("1.5").unwrap());
        entrada.push_str(&kraken_crc_num("100.0").unwrap());
        entrada.push_str(&kraken_crc_num("2.0").unwrap());
        let checksum = crc32fast::hash(entrada.as_bytes());
        let snapshot = format!(
            r#"{{"channel":"book","type":"snapshot","data":[{{"symbol":"BTC/USD","bids":[{{"price":100.0,"qty":2.0}}],"asks":[{{"price":101.0,"qty":1.5}}],"checksum":{checksum},"timestamp":"2024-03-09T00:00:00Z"}}]}}"#
        );
        let mut libro = LibroEstado::new("BTC/USD");
        let cotizacion = parsear_kraken(snapshot.as_bytes(), &mut libro).unwrap();
        assert_eq!(cotizacion.integrity_status, "checksum_crc32_ok");

        let malo = snapshot.replace(&checksum.to_string(), &checksum.wrapping_add(1).to_string());
        assert!(parsear_kraken(malo.as_bytes(), &mut libro).is_none());
        assert_eq!(libro.checksum_failures, 1);
        assert!(libro.requiere_snapshot);
        assert!(libro.bids.is_empty());
    }

    #[test]
    fn kraken_rechaza_checksum_ausente() {
        let snapshot = br#"{"channel":"book","type":"snapshot","data":[{"symbol":"BTC/USD","bids":[{"price":100.0,"qty":2.0}],"asks":[{"price":101.0,"qty":1.5}],"timestamp":"2024-03-09T00:00:00Z"}]}"#;
        let mut libro = LibroEstado::new("BTC/USD");

        assert!(parsear_kraken(snapshot, &mut libro).is_none());
        assert_eq!(libro.checksum_failures, 1);
        assert!(libro.requiere_snapshot);
        assert!(libro.bids.is_empty());
    }

    #[test]
    fn kraken_crc_preserva_ceros_y_coincide_con_vector_oficial() {
        let entrada = "45285210000045286415457195345286615457110945289615456091145290215890660452918154553491452947445474945296135380000452975994554245299518772827452835100000004528341545820154528211000000045281010000000452803154592586452790799000045277633101034527753000000045277315460273745276615445238";
        assert_eq!(crc32fast::hash(entrada.as_bytes()), 3_310_070_434);
        assert_eq!(kraken_crc_num("0.00100000").as_deref(), Some("100000"));
    }

    #[test]
    fn okx_conserva_ask_en_delta_solo_bid() {
        let snapshot = br#"{"arg":{"channel":"books","instId":"BTC-USDT"},"action":"snapshot","data":[{"bids":[["100.0","2.0"]],"asks":[["101.0","1.5"]],"ts":"1710000000000"}]}"#;
        let delta = br#"{"arg":{"channel":"books","instId":"BTC-USDT"},"action":"update","data":[{"bids":[["100.5","3.0"]],"ts":"1710000000001"}]}"#;
        let mut libro = LibroEstado::new("BTC/USD");
        parsear_okx(snapshot, &mut libro).unwrap();

        let c = parsear_okx(delta, &mut libro).unwrap();

        assert_eq!(c.bid, (100.5));
        assert_eq!(c.ask, (101.0));
    }

    #[test]
    fn okx_prev_seq_invalido_fuerza_resync() {
        let snapshot = br#"{"arg":{"channel":"books","instId":"BTC-USDT"},"action":"snapshot","data":[{"seqId":10,"bids":[["100.0","2.0"]],"asks":[["101.0","1.5"]],"ts":"1710000000000"}]}"#;
        let gap = br#"{"arg":{"channel":"books","instId":"BTC-USDT"},"action":"update","data":[{"seqId":12,"prevSeqId":9,"bids":[["100.5","3.0"]],"ts":"1710000000001"}]}"#;
        let mut libro = LibroEstado::new("BTC/USD");
        parsear_okx(snapshot, &mut libro).unwrap();

        assert!(parsear_okx(gap, &mut libro).is_none());
        assert_eq!(libro.integrity_status, "gap_requiere_snapshot");
        assert_eq!(libro.resyncs, 1);
    }

    #[test]
    fn parsea_rest_depth_binance() {
        let msg = br#"{"lastUpdateId":1,"bids":[["100.0","2.0"]],"asks":[["101.0","1.5"]]}"#;
        let c = parsear_rest_binance(msg, "BTC/USD").unwrap();
        assert_eq!(c.bid_cantidad, (2.0));
        assert_eq!(c.ask_cantidad, (1.5));
    }

    #[test]
    fn parsea_rest_depth_kraken() {
        let msg = br#"{"error":[],"result":{"XXBTZUSD":{"bids":[["100.0","2.0","1"]],"asks":[["101.0","1.5","1"]]}}}"#;
        let c = parsear_rest_kraken(msg, "BTC/USD").unwrap();
        assert_eq!(c.bid, (100.0));
        assert_eq!(c.ask, (101.0));
    }

    #[test]
    fn parsea_rest_book_coinbase() {
        let msg = br#"{"bids":[["100.0","2.0",1]],"asks":[["101.0","1.5",1]],"time":"2024-03-09T00:00:00Z"}"#;
        let c = parsear_rest_coinbase(msg, "BTC/USD").unwrap();
        assert_eq!(c.par, "BTC/USD");
        assert_eq!(c.bid, (100.0));
    }

    #[test]
    fn parsea_rest_books_okx() {
        let msg = br#"{"code":"0","data":[{"bids":[["100.0","2.0","0","1"]],"asks":[["101.0","1.5","0","1"]],"ts":"1710000000000"}]}"#;
        let c = parsear_rest_okx(msg, "BTC/USD").unwrap();
        assert_eq!(c.bid, (100.0));
        assert_eq!(c.ask, (101.0));
    }

    #[test]
    fn parsea_rest_orderbook_bybit() {
        let msg = br#"{"retCode":0,"result":{"b":[["100.0","2.0"]],"a":[["101.0","1.5"]],"ts":"1710000000000"}}"#;
        let c = parsear_rest_bybit(msg, "BTC/USD").unwrap();
        assert_eq!(c.bid, (100.0));
        assert_eq!(c.ask, (101.0));
    }

    #[test]
    fn parsea_bitfinex_ws_snapshot() {
        let msg = br#"[1,["snapshot",[100.0,2,2.0],[100.5,3,-1.5]]]"#;
        let mut libro = LibroEstado::new("BTC/USD");
        let c = parsear_bitfinex(msg, &mut libro).unwrap();
        assert_eq!(c.par, "BTC/USD");
        assert_eq!(c.bid, (100.0));
        assert_eq!(c.ask, (100.5));
    }

    #[test]
    fn parsea_bitfinex_ws_heartbeat_ignorado() {
        let msg = br#"[1,"hb"]"#;
        let mut libro = LibroEstado::new("BTC/USD");
        assert!(parsear_bitfinex(msg, &mut libro).is_none());
    }

    #[test]
    fn parsea_kucoin_ws_snapshot() {
        let msg = br#"{"type":"message","subject":"trade.l2snapshot","topic":"/market/level2:BTC-USDT","data":{"sequence":"1","bids":[["100.0","2.0"]],"asks":[["101.0","1.5"]],"timestamp":1710000000000}}"#;
        let mut libro = LibroEstado::new("BTC/USD");
        let c = parsear_kucoin(msg, &mut libro).unwrap();
        assert_eq!(c.bid, (100.0));
        assert_eq!(c.ask, (101.0));
    }

    #[test]
    fn parsea_kucoin_ws_welcome_ignorado() {
        let msg = br#"{"type":"welcome","id":"abc"}"#;
        let mut libro = LibroEstado::new("BTC/USD");
        assert!(parsear_kucoin(msg, &mut libro).is_none());
    }

    #[test]
    fn parsea_gateio_ws_snapshot() {
        let msg = br#"{"method":"order_book.update","params":[null,null,false,{"bids":[["100.0","2.0"]],"asks":[["101.0","1.5"]]}],"id":1}"#;
        let mut libro = LibroEstado::new("BTC/USD");
        let c = parsear_gateio(msg, &mut libro).unwrap();
        assert_eq!(c.bid, (100.0));
        assert_eq!(c.ask, (101.0));
    }

    #[test]
    fn parsea_gateio_ws_ignora_otros_metodos() {
        let msg = br#"{"method":"channel.subscribe","params":["book.BTC_USDT"],"id":1}"#;
        let mut libro = LibroEstado::new("BTC/USD");
        assert!(parsear_gateio(msg, &mut libro).is_none());
    }

    #[test]
    fn parsea_bitstamp_ws_snapshot() {
        let msg = br#"{"event":"data","data":{"bids":[["100.0","2.0"]],"asks":[["101.0","1.5"]],"timestamp":"1710000000000"}}"#;
        let mut libro = LibroEstado::new("BTC/USD");
        let c = parsear_bitstamp(msg, &mut libro).unwrap();
        assert_eq!(c.bid, (100.0));
        assert_eq!(c.ask, (101.0));
    }

    #[test]
    fn parsea_bitstamp_ws_subscription_ignorada() {
        let msg = br#"{"event":"bts:subscription_succeeded"}"#;
        let mut libro = LibroEstado::new("BTC/USD");
        assert!(parsear_bitstamp(msg, &mut libro).is_none());
    }

    #[test]
    fn parsea_gemini_ws_snapshot() {
        let msg = br#"[{"type":"snapshot","event_id":1,"price":"100.0","remaining":"2.0","side":"bid"},{"type":"snapshot","event_id":2,"price":"101.0","remaining":"1.5","side":"ask"}]"#;
        let mut libro = LibroEstado::new("BTC/USD");
        let c = parsear_gemini(msg, &mut libro).unwrap();
        assert_eq!(c.bid, (100.0));
        assert_eq!(c.ask, (101.0));
    }

    #[test]
    fn parsea_rest_bitfinex_devuelve_cotizacion() {
        let msg = br#"[[100.0,2,2.0],[101.0,3,-1.5]]"#;
        let c = parsear_rest_bitfinex(msg, "BTC/USD").unwrap();
        assert_eq!(c.bid, (100.0));
        assert_eq!(c.ask, (101.0));
    }

    #[test]
    fn parsea_rest_kucoin_devuelve_cotizacion() {
        let msg = br#"{"code":"200000","data":{"bids":[["100.0","2.0"]],"asks":[["101.0","1.5"]],"time":1710000000000}}"#;
        let c = parsear_rest_kucoin(msg, "BTC/USD").unwrap();
        assert_eq!(c.bid, (100.0));
        assert_eq!(c.ask, (101.0));
    }

    #[test]
    fn parsea_rest_gateio_devuelve_cotizacion() {
        let msg = br#"[{"bids":[["100.0","2.0"]],"asks":[["101.0","1.5"]],"t":1710000000000}]"#;
        let c = parsear_rest_gateio(msg, "BTC/USD").unwrap();
        assert_eq!(c.bid, (100.0));
        assert_eq!(c.ask, (101.0));
    }

    #[test]
    fn parsea_rest_bitstamp_devuelve_cotizacion() {
        let msg =
            br#"{"bids":[["100.0","2.0"]],"asks":[["101.0","1.5"]],"timestamp":"1710000000000"}"#;
        let c = parsear_rest_bitstamp(msg, "BTC/USD").unwrap();
        assert_eq!(c.bid, (100.0));
        assert_eq!(c.ask, (101.0));
    }

    #[test]
    fn parsea_rest_gemini_devuelve_cotizacion() {
        let msg = br#"{"bids":[["100.0","2.0"]],"asks":[["101.0","1.5"]]}"#;
        let c = parsear_rest_gemini(msg, "BTC/USD").unwrap();
        assert_eq!(c.bid, (100.0));
        assert_eq!(c.ask, (101.0));
    }

    #[test]
    fn adaptadores_exportan_trait_object_por_venue() {
        let lista = adaptadores("BTC/USD");
        // 12 venues: 10 CEX + 2 DEX (Jupiter, Raydium)
        assert_eq!(lista.len(), 12);
        let nombres: Vec<&str> = lista.iter().map(|a| a.nombre()).collect();
        assert!(nombres.contains(&"Binance"));
        assert!(nombres.contains(&"Gate.io"));
        assert!(nombres.contains(&"Gemini"));
        assert!(nombres.contains(&"Jupiter"));
        assert!(nombres.contains(&"Raydium"));
        for a in &lista {
            assert!(!a.ws_url().is_empty());
            assert!(a.tiene_rest(), "{} debe tener REST fallback", a.nombre());
            assert!(a
                .parse_ws(br#"{"ignorado":true}"#, &mut LibroEstado::new("BTC/USD"))
                .is_none());
        }
    }

    #[test]
    fn libro_escalado_preserva_orden_y_elimina_niveles_en_64_casos() {
        let mut libro = LibroEstado::new("BTC/USD");
        for caso in 1..=64 {
            let base = 50_000.0 + caso as f64 / 100.0;
            libro.actualizar_bids(&[
                NivelOrden {
                    precio: base,
                    cantidad: 0.25,
                },
                NivelOrden {
                    precio: base + 0.01,
                    cantidad: 0.50,
                },
            ]);
            libro.actualizar_asks(&[
                NivelOrden {
                    precio: base + 1.00,
                    cantidad: 0.30,
                },
                NivelOrden {
                    precio: base + 1.01,
                    cantidad: 0.60,
                },
            ]);
            let snapshot = libro.cotizacion(1).expect("snapshot ordenado");
            assert!((snapshot.bid - (base + 0.01)).abs() < 1e-8);
            assert!((snapshot.ask - (base + 1.00)).abs() < 1e-8);

            libro.actualizar_bids(&[NivelOrden {
                precio: base + 0.01,
                cantidad: 0.0,
            }]);
            let sin_nivel = libro.cotizacion(2).expect("snapshot tras borrar nivel");
            assert!((sin_nivel.bid - base).abs() < 1e-8);
            libro.reset("BTC/USD");
        }
    }
}
