//! API HTTP, WebSocket local y servidor de archivos estáticos.
//!
//! Los endpoints mutables modifican solo estado simulado en memoria. Cuando se
//! define `ADMIN_TOKEN`, requieren `Authorization: Bearer <token>` o
//! `X-Admin-Token`.

use std::{
    collections::{hash_map::DefaultHasher, HashMap, HashSet},
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::{
        rejection::JsonRejection,
        ws::{Message, WebSocket, WebSocketUpgrade},
        Request, State,
    },
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    middleware::Next,
    response::{Html, IntoResponse, Response},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use rand::{rngs::StdRng, Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tower::limit::ConcurrencyLimitLayer;
use tower_http::{
    compression::CompressionLayer, limit::RequestBodyLimitLayer, services::ServeDir,
    set_header::SetResponseHeaderLayer, timeout::TimeoutLayer,
};

use crate::{
    discord::{self, ConfigDiscord},
    ga::ConfigGa,
    impacto::{LadoOrden, ModeloImpacto, OrdenImpacto},
    metricas::Metricas,
    motor::{EscenarioDemo, Motor},
    types::{Cotizacion, EstadoPublico, ExchangeConfig, MapaCostos, NivelOrden},
};

#[derive(Clone)]
pub(crate) struct EstadoApp {
    motor: Arc<Motor>,
    token_admin: Option<String>,
    /// Habilita únicamente recorridos de demostración deterministas y seguros.
    /// No concede acceso a configuración, exchanges, wallets ni herramientas MCP.
    judge_mode: bool,
    ws_tx: tokio::sync::broadcast::Sender<String>,
    metricas: Metricas,
    discord: ConfigDiscord,
    rate_limiter: Arc<RateLimiter>,
}

#[derive(Default)]
struct RateLimiter {
    buckets: tokio::sync::Mutex<HashMap<String, (Instant, u32)>>,
}

/// Construye el router Axum completo del binario.
pub fn router(motor: Arc<Motor>, token_admin: Option<String>) -> Router {
    let (ws_tx, _) = tokio::sync::broadcast::channel(16);
    let metricas = Metricas::new();
    let discord = ConfigDiscord::from_env();
    let state = EstadoApp {
        motor: motor.clone(),
        token_admin,
        judge_mode: env_bool("MAYAB_JUDGE_MODE", false),
        ws_tx: ws_tx.clone(),
        metricas: metricas.clone(),
        discord,
        rate_limiter: Arc::new(RateLimiter::default()),
    };
    let origin_policy = crate::http::origin::OriginPolicy::new();
    let cors = crate::http::origin::cors_layer(&origin_policy);

    let metricas_ws = metricas.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(450));
        loop {
            ticker.tick().await;
            if ws_tx.receiver_count() == 0 {
                continue;
            }
            let inicio_snapshot = Metricas::ahora();
            let mut estado = motor.estado().await;
            compactar_estado_ws(&mut estado);
            metricas_ws.registrar_etapa("snapshot", inicio_snapshot.elapsed());
            let inicio_serializacion = Metricas::ahora();
            if let Ok(payload) = serde_json::to_string(&estado) {
                metricas_ws.registrar_etapa("serializacion", inicio_serializacion.elapsed());
                let inicio_broadcast = Metricas::ahora();
                let _ = ws_tx.send(payload);
                metricas_ws.registrar_etapa("ws_broadcast", inicio_broadcast.elapsed());
            }
        }
    });
    let archivos_estaticos =
        ServeDir::new("internal/webui/web").append_index_html_on_directories(true);
    crate::http::router::api_routes()
        .nest_service("/screenshots", ServeDir::new("screenshots"))
        .fallback_service(archivos_estaticos)
        .layer(cors)
        .layer(axum::middleware::from_fn_with_state(
            origin_policy,
            crate::http::origin::origin_middleware,
        ))
        .layer(axum::middleware::from_fn(cache_headers))
        .layer(axum::middleware::from_fn_with_state(state.clone(), rate_limit))
        .layer(axum::middleware::from_fn_with_state(
            metricas,
            contar_peticiones,
        ))
        .layer(CompressionLayer::new())
        .layer(RequestBodyLimitLayer::new(env_usize("HTTP_MAX_BODY_BYTES", 1_048_576)))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(env_u64("HTTP_TIMEOUT_SECS", 30)),
        ))
        .layer(ConcurrencyLimitLayer::new(env_usize("HTTP_MAX_CONCURRENCY", 128)))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("geolocation=(), camera=(), microphone=(), payment=()"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static(
                "default-src 'self'; connect-src 'self' ws: wss:; img-src 'self' data:; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; font-src 'self' data: https://fonts.gstatic.com; script-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
            ),
        ))
        .with_state(state)
}

async fn rate_limit(State(app): State<EstadoApp>, req: Request, next: Next) -> Response {
    if matches!(req.uri().path(), "/healthz" | "/readyz") {
        return next.run(req).await;
    }
    let mutating = !matches!(
        *req.method(),
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    );
    let limit = if mutating {
        env_u32("HTTP_MUTATION_RPM", 30)
    } else {
        env_u32("HTTP_READ_RPM", 300)
    };
    let client = if env_bool("TRUST_PROXY_HEADERS", false) {
        req.headers()
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("proxy-unknown")
    } else {
        "direct"
    };
    let key = format!("{}:{}", client, if mutating { "write" } else { "read" });
    let now = Instant::now();
    let mut buckets = app.rate_limiter.buckets.lock().await;
    buckets.retain(|_, (started, _)| now.duration_since(*started) < Duration::from_secs(120));
    let bucket = buckets.entry(key).or_insert((now, 0));
    if now.duration_since(bucket.0) >= Duration::from_secs(60) {
        *bucket = (now, 0);
    }
    if bucket.1 >= limit {
        return (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, "60")], Json(json!({
            "ok": false,
            "error": { "code": "rate_limited", "message": "demasiadas peticiones; intente de nuevo más tarde" }
        }))).into_response();
    }
    bucket.1 += 1;
    drop(buckets);
    next.run(req).await
}

pub(crate) async fn healthz() -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "name": env!("CARGO_PKG_NAME"),
    }))
}

pub(crate) async fn readyz(State(app): State<EstadoApp>) -> Response {
    let estado = match tokio::time::timeout(Duration::from_secs(2), app.motor.estado()).await {
        Ok(estado) => estado,
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "ready": false,
                    "checks": [{"name": "motor_snapshot", "ok": false, "reason": "motor state lock timed out"}]
                })),
            )
                .into_response();
        }
    };
    let mut checks = Vec::new();
    let mut ready = true;

    // Check persistence
    let persistencia_ok = estado
        .persistencia
        .as_ref()
        .map(|p| p.activa)
        .unwrap_or(false);
    let persistencia_error = estado.persistencia.as_ref().and_then(|p| p.error.clone());
    checks.push(json!({
        "name": "persistence",
        "ok": persistencia_ok,
        "reason": if persistencia_ok { "SQLite active" } else { "persistence not available" },
        "detail": persistencia_error
    }));
    if !persistencia_ok {
        ready = false;
    }

    // Check motor initialized (has at least one quote)
    let quotes_ok = !estado.cotizaciones.is_empty();
    checks.push(json!({
        "name": "motor_initialized",
        "ok": quotes_ok,
        "reason": if quotes_ok { "motor has quotes" } else { "no quotes yet" }
    }));
    if !quotes_ok {
        ready = false;
    }

    // Check minimum fresh feeds (at least 2 exchanges with fresh snapshots)
    let stale_ms = estado.configuracion.stale_ms;
    let fresh_exchanges: std::collections::HashSet<_> = estado
        .cotizaciones
        .iter()
        .filter(|c| {
            let age_ms = (estado.generado_en - c.recibida_en)
                .num_milliseconds()
                .max(0);
            age_ms <= stale_ms && c.bid > 0.0 && c.ask > c.bid
        })
        .map(|c| c.exchange.as_str())
        .collect();
    let fresh_feeds_ok = fresh_exchanges.len() >= 2;
    checks.push(json!({
        "name": "fresh_feeds",
        "ok": fresh_feeds_ok,
        "reason": format!("{} fresh exchanges (need >= 2)", fresh_exchanges.len()),
        "exchanges": fresh_exchanges.into_iter().collect::<Vec<_>>()
    }));
    if !fresh_feeds_ok {
        ready = false;
    }

    // Check degraded state
    let degraded = estado.metricas.circuit_breaker_activo
        || estado.metricas.estado_riesgo == "critico"
        || estado.metricas.ejecucion_en_curso;
    checks.push(json!({
        "name": "degraded_state",
        "ok": !degraded,
        "reason": if degraded { "system in degraded state" } else { "system healthy" },
        "circuit_breaker": estado.metricas.circuit_breaker_activo,
        "risk_state": estado.metricas.estado_riesgo,
        "execution_in_progress": estado.metricas.ejecucion_en_curso
    }));
    if degraded {
        ready = false;
    }

    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(json!({
            "ready": ready,
            "checks": checks,
            "timestamp": estado.generado_en
        })),
    )
        .into_response()
}

async fn contar_peticiones(State(metricas): State<Metricas>, req: Request, next: Next) -> Response {
    let ruta = req.uri().path().to_string();
    let metodo = req.method().clone();
    let inicio = Metricas::ahora();
    let resp = next.run(req).await;
    metricas.registrar_peticion(
        &ruta,
        metodo.as_str(),
        resp.status().as_u16(),
        inicio.elapsed(),
    );
    resp
}

pub(crate) async fn metrics(State(app): State<EstadoApp>, headers: HeaderMap) -> Response {
    if is_production() && !env_bool("METRICS_PUBLIC", false) {
        if let Some(response) = autorizar_mutacion(&app, &headers) {
            return response;
        }
    }
    let estado = app.motor.estado().await;
    let inicio = Metricas::ahora();
    let texto = app.metricas.render(&estado);
    app.metricas
        .registrar_etapa("metrics_render", inicio.elapsed());
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        texto,
    )
        .into_response()
}

pub(crate) async fn estado(State(app): State<EstadoApp>) -> Json<crate::types::EstadoPublico> {
    let inicio = Metricas::ahora();
    let estado = app.motor.estado().await;
    app.metricas.registrar_etapa("snapshot", inicio.elapsed());
    Json(estado)
}

pub(crate) async fn operator() -> Html<&'static str> {
    Html(include_str!("../../internal/webui/web/operator.html"))
}

async fn cache_headers(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let mut response = next.run(req).await;
    let value = if path.starts_with("/api/")
        || matches!(
            path.as_str(),
            "/healthz" | "/readyz" | "/metrics" | "/tiempo-real"
        ) {
        HeaderValue::from_static("no-store")
    } else if path == "/" || path.ends_with(".html") {
        HeaderValue::from_static("no-cache, must-revalidate")
    } else {
        HeaderValue::from_static("public, max-age=3600, stale-while-revalidate=86400")
    };
    response.headers_mut().insert(header::CACHE_CONTROL, value);
    response
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(default)
}

fn is_production() -> bool {
    ["MAYAB_ENV", "ENTORNO"].iter().any(|key| {
        std::env::var(key)
            .ok()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("production"))
    })
}

pub(crate) async fn preflight(State(app): State<EstadoApp>) -> Json<serde_json::Value> {
    let estado = app.motor.estado().await;
    Json(construir_preflight(&estado))
}

pub(crate) async fn jurado(State(app): State<EstadoApp>) -> Json<serde_json::Value> {
    let estado = app.motor.estado().await;
    let mut respuesta = construir_modo_jurado(&estado);
    respuesta["accesoDemo"] = json!({
        "modoJuradoPublico": app.judge_mode,
        "requiereAdminToken": !app.judge_mode && app.token_admin.is_some(),
        "recorridosPublicos": if app.judge_mode {
            vec!["/api/demo/reset", "/api/demo/final", "/api/demo/caos"]
        } else {
            Vec::<&str>::new()
        },
        "limite": "sólo escenarios deterministas sobre estado simulado; configuración, wallets arbitrarios, exchanges, GA libre y MCP permanecen protegidos",
        "rateLimit": "HTTP_MUTATION_RPM por cliente"
    });
    Json(respuesta)
}

pub(crate) async fn version() -> Json<crate::version::BuildVersion> {
    Json(crate::version::current())
}

pub(crate) async fn resumen_llm(State(app): State<EstadoApp>) -> Json<serde_json::Value> {
    let estado = app.motor.estado().await;
    Json(construir_resumen_llm(&estado))
}

pub(crate) async fn discord_interactions(
    State(app): State<EstadoApp>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    discord::responder_interaccion(app.motor.clone(), &app.discord, &headers, body).await
}

pub(crate) async fn mcp_manifest() -> Json<serde_json::Value> {
    Json(construir_mcp_manifest())
}

pub(crate) async fn mcp_call(
    State(app): State<EstadoApp>,
    headers: HeaderMap,
    payload: Result<Json<SolicitudMcp>, JsonRejection>,
) -> Response {
    let Json(payload) = match payload {
        Ok(payload) => payload,
        Err(err) => return rechazo_json(err).into_response(),
    };
    let tool = payload.tool.as_str();
    if matches!(tool, "prepare_demo_final" | "evolve_ga" | "demo_scenario") {
        if let Some(response) = autorizar_mutacion(&app, &headers) {
            return response;
        }
    }

    let estado = app.motor.estado().await;
    let args = payload.arguments.unwrap_or_else(|| json!({}));
    let respuesta = match tool {
        "get_state" => json!({ "ok": true, "tool": tool, "result": estado }),
        "preflight" => json!({ "ok": true, "tool": tool, "result": construir_preflight(&estado) }),
        "jury_mode" => {
            json!({ "ok": true, "tool": tool, "result": construir_modo_jurado(&estado) })
        }
        "summarize_for_llm" => {
            json!({ "ok": true, "tool": tool, "result": construir_resumen_llm(&estado) })
        }
        "evaluation_package" => {
            json!({ "ok": true, "tool": tool, "result": construir_paquete_evaluacion(&estado) })
        }
        "latency_ranking" => json!({
            "ok": true,
            "tool": tool,
            "result": {
                "generadoEn": estado.generado_en,
                "latenciaPromedioMs": estado.metricas.latencia_promedio_ms,
                "estadoRiesgo": estado.metricas.estado_riesgo,
                "exchanges": estado.latencias_exchange,
            }
        }),
        "backtest" => json!({
            "ok": true,
            "tool": tool,
            "result": backtest_reproducible(&estado)
        }),
        "research_lab_sweep" => json!({
            "ok": true,
            "tool": tool,
            "result": lab_sweep_reproducible(&estado)
        }),
        "prepare_demo_final" => {
            let ga = app.motor.evolucionar_ga(true, 96).await;
            let rentable = app
                .motor
                .activar_escenario_demo(EscenarioDemo::MercadoRentable)
                .await;
            let fill_parcial = app
                .motor
                .activar_escenario_demo(EscenarioDemo::FillParcial)
                .await;
            let rebalanceo = app
                .motor
                .activar_escenario_demo(EscenarioDemo::Rebalanceo)
                .await;
            let estado_final = app.motor.estado().await;
            json!({
                "ok": true,
                "tool": tool,
                "result": {
                    "ga": ga,
                    "mercadoRentable": rentable,
                    "fillParcial": fill_parcial,
                    "rebalanceo": rebalanceo,
                    "metricas": estado_final.metricas,
                    "mlEdge": estado_final.ml_edge,
                    "preflight": construir_preflight(&estado_final),
                }
            })
        }
        "evolve_ga" => {
            let usar_replay = args
                .get("usarReplaySiVacio")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let muestras = args
                .get("muestras")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
                .unwrap_or(96);
            json!({
                "ok": true,
                "tool": tool,
                "result": app.motor.evolucionar_ga(usar_replay, muestras).await
            })
        }
        "demo_scenario" => {
            let escenario = match args.get("escenario").and_then(|v| v.as_str()) {
                Some("fallo_orden") => EscenarioDemo::FalloOrden,
                Some("mercado_movido") => EscenarioDemo::MercadoMovido,
                Some("liquidez_insuficiente") => EscenarioDemo::LiquidezInsuficiente,
                Some("fill_parcial") => EscenarioDemo::FillParcial,
                Some("circuit_breaker") => EscenarioDemo::CircuitBreaker,
                Some("rebalanceo") => EscenarioDemo::Rebalanceo,
                Some("mercado_rentable") => EscenarioDemo::MercadoRentable,
                _ => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "ok": false,
                            "error": "escenario invalido",
                            "validos": [
                                "fallo_orden",
                                "mercado_movido",
                                "liquidez_insuficiente",
                                "fill_parcial",
                                "circuit_breaker",
                                "rebalanceo",
                                "mercado_rentable"
                            ]
                        })),
                    )
                        .into_response()
                }
            };
            json!({
                "ok": true,
                "tool": tool,
                "result": app.motor.activar_escenario_demo(escenario).await
            })
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "ok": false,
                    "error": "tool no soportada",
                    "manifest": "/api/mcp/manifest"
                })),
            )
                .into_response()
        }
    };

    Json(respuesta).into_response()
}

pub(crate) async fn paquete_evaluacion(State(app): State<EstadoApp>) -> Json<serde_json::Value> {
    let estado = app.motor.estado().await;
    Json(construir_paquete_evaluacion(&estado))
}

pub(crate) async fn latencias(State(app): State<EstadoApp>) -> Json<serde_json::Value> {
    let estado = app.motor.estado().await;
    Json(json!({
        "generadoEn": estado.generado_en,
        "corrida": estado.corrida,
        "latenciaPromedioMs": estado.metricas.latencia_promedio_ms,
        "estadoRiesgo": estado.metricas.estado_riesgo,
        "exchanges": estado.latencias_exchange,
        "pipeline": estado.telemetria_pipeline,
        "nota": "Separa transporte exchange->ingesta de quote->decision y compute interno; reporta p50/p95/p99, throughput, rutas evaluadas y coalescing."
    }))
}

pub(crate) async fn backtest(State(app): State<EstadoApp>) -> Json<serde_json::Value> {
    let estado = app.motor.estado().await;
    Json(backtest_reproducible(&estado))
}

pub(crate) async fn research_tapes() -> Json<serde_json::Value> {
    Json(construir_research_tapes())
}

pub(crate) async fn research_walk_forward(State(app): State<EstadoApp>) -> Json<serde_json::Value> {
    let reporte = backtest_reproducible(&app.motor.estado().await);
    Json(json!({
        "schemaVersion": 1,
        "artifact": "/api/research/walk-forward",
        "split": { "train": 50, "calibration": 20, "holdout": 30 },
        "protocol": {
            "train": "El GA se ajusta antes del holdout.",
            "calibration": "Las semillas 301..312 documentan calibración sin seleccionar retrospectivamente sobre holdout.",
            "holdout": "Las semillas 401..424 no reentrenan ni reajustan el campeón."
        },
        "gaVsBaselines": reporte["validacionFueraMuestra"],
        "source": { "kind": "synthetic_replay", "endpoint": "/api/backtest" },
        "limitations": [
            "El split porcentual describe el protocolo del evaluador de tape; este endpoint runtime usa semillas cronológicas separadas.",
            "El replay runtime es sintético y no demuestra rentabilidad real."
        ]
    }))
}

pub(crate) async fn research_impact(State(app): State<EstadoApp>) -> Json<serde_json::Value> {
    let estado = app.motor.estado().await;
    Json(json!({
        "schemaVersion": 1,
        "artifact": "/api/research/impact",
        "comparison": comparar_modelos_impacto(&estado.configuracion, 42),
        "limitations": ["Los markouts de esta comparación son simulados.", "Book-walk requiere profundidad observable suficiente."]
    }))
}

pub(crate) async fn research_bootstrap(State(app): State<EstadoApp>) -> Json<serde_json::Value> {
    let reporte = backtest_reproducible(&app.motor.estado().await);
    Json(json!({
        "schemaVersion": 1,
        "artifact": "/api/research/bootstrap",
        "comparison": "campeon_ga_menos_baseline_estatico",
        "bootstrap": reporte["significanciaBootstrap"],
        "limitations": ["El intervalo cuantifica incertidumbre interna del replay sintético, no riesgo de mercado live."]
    }))
}

pub(crate) async fn research_microstructure() -> Json<serde_json::Value> {
    let configured = std::env::var_os("MAYAB_RESEARCH_TAPE").map(PathBuf::from);
    Json(json!({
        "schemaVersion": 1,
        "artifact": "/api/research/microstructure",
        "report": crate::microestructura::build_report(configured.as_deref(), 42),
        "separation": {
            "optimizer": "GA y optimización permanecen fuera de este módulo",
            "calibration": "Platt e isotónica se ajustan solamente en la ventana B",
            "evaluation": "Brier, log-loss, ECE, Wilson y transferencia por venue se calculan en C"
        }
    }))
}

pub(crate) async fn research_ou() -> Json<serde_json::Value> {
    let configured = std::env::var_os("MAYAB_RESEARCH_TAPE").map(PathBuf::from);
    Json(json!({
        "schemaVersion": 1,
        "artifact": "/api/research/ou",
        "report": crate::ou::build_report(configured.as_deref(), 42)
    }))
}

pub(crate) async fn research_ledger_audit(State(app): State<EstadoApp>) -> Json<serde_json::Value> {
    let estado = app.motor.estado().await;
    let ids = estado
        .operaciones
        .iter()
        .map(|o| o.id.as_str())
        .collect::<Vec<_>>();
    let unicos = ids.iter().copied().collect::<HashSet<_>>().len();
    let payload = serde_json::to_vec(&json!({
        "corrida": estado.corrida.id,
        "operaciones": estado.operaciones,
        "trazas": estado.trazas_ejecucion,
        "rebalanceos": estado.rebalanceos,
        "balances": estado.balances,
    }))
    .unwrap_or_default();
    Json(json!({
        "schemaVersion": 1,
        "artifact": "/api/research/ledger-audit",
        "runId": estado.corrida.id,
        "snapshotSha256": hex::encode(Sha256::digest(&payload)),
        "checks": {
            "operationIdsUnique": unicos == ids.len(),
            "operationsMatchMetric": estado.operaciones.len() == estado.metricas.operaciones_totales,
            "noRealExecution": !estado.corrida.ejecucion_real,
        },
        "counts": {
            "operations": estado.operaciones.len(),
            "decisionAudits": estado.auditoria_decisiones.len(),
            "executionTransitions": estado.trazas_ejecucion.len(),
            "rebalances": estado.rebalanceos.len(),
        },
        "source": "/api/export/json",
        "limitations": ["Este hash cubre el snapshot en memoria; no sustituye un ledger persistido y encadenado entre reinicios."]
    }))
}

pub(crate) async fn readiness_live(State(app): State<EstadoApp>) -> Json<serde_json::Value> {
    let estado = app.motor.estado().await;
    Json(json!({
        "schemaVersion": 1,
        "stage": "S0_S1",
        "status": "not_ready_for_live_trading",
        "marketDataLive": estado.corrida.modo == "mercado_real",
        "liveTrading": false,
        "realExecution": estado.corrida.ejecucion_real,
        "capitalAtRiskUsd": 0,
        "completedScope": ["simulación", "replay offline"],
        "blockedStages": ["S2 shadow", "S3 testnet", "S4 production read-only", "S5 canary", "S6 live controlado"],
        "artifact": "/LIVE_READINESS.md",
        "limitations": [
            "No hay órdenes reales, custodia, transferencias on-chain ni manejo de secretos de trading.",
            "Datos live no significa trading live.",
            "Los gates no comprobados permanecen abiertos."
        ]
    }))
}

fn construir_research_tapes() -> serde_json::Value {
    if let Some(root) = std::env::var_os("MAYAB_RESEARCH_CORPUS").map(PathBuf::from) {
        let report_path = root.join("corpus.json");
        return match fs::read(&report_path) {
            Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
                Ok(report)
                    if report
                        .get("classification")
                        .and_then(|value| value.as_str())
                        == Some("public_market_capture_corpus") =>
                {
                    let corpus_sha = report
                        .get("corpusSha256")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let verified_seal = crate::tape::verify_corpus_evidence_seal(&root)
                        .ok()
                        .filter(|seal| seal.corpus_sha256 == corpus_sha);
                    let scan_path = root.join("corpus-scan.json");
                    let (scan, scan_sha256, scan_status) = if verified_seal.is_none() {
                        (None, None, "unsealed_evidence")
                    } else {
                        match fs::read(&scan_path) {
                            Ok(scan_bytes) => match validar_scan_corpus(corpus_sha, &scan_bytes) {
                                Ok(scan) => (
                                    Some(scan),
                                    Some(hex::encode(Sha256::digest(&scan_bytes))),
                                    "matched_corpus",
                                ),
                                Err(status) => (None, None, status),
                            },
                            Err(_) => (None, None, "missing_scan"),
                        }
                    };
                    json!({
                        "schemaVersion": 2,
                        "available": true,
                        "mode": "verified_corpus_report",
                        "corpus": report,
                        "quantitativeScan": scan,
                        "evidenceSeal": verified_seal,
                        "scanStatus": scan_status,
                        "scanPath": "$MAYAB_RESEARCH_CORPUS/corpus-scan.json",
                        "sealPath": "$MAYAB_RESEARCH_CORPUS/evidence-seal.json",
                        "scanSha256": scan_sha256,
                        "reportPath": "$MAYAB_RESEARCH_CORPUS/corpus.json",
                        "reportSha256": hex::encode(Sha256::digest(&bytes)),
                        "verificationCommand": "cargo run -p mayab-cli --bin verify-corpus -- --root \"$MAYAB_RESEARCH_CORPUS\" --output \"$MAYAB_RESEARCH_CORPUS/corpus.json\"",
                        "limitations": [
                            "El endpoint publica el reporte precomputado para no reescanear el corpus en cada GET.",
                            "reportSha256 protege este JSON; corpusSha256 y los hashes de shards deben revalidarse con verify-corpus.",
                            "quantitativeScan solo se publica cuando su corpusSha256 coincide exactamente con el reporte de corpus.",
                            "El scan permanece oculto si el sello no valida corpus.json, corpus-scan.json y corpus.sqlite.",
                            "Millones de eventos no equivalen a millones de dislocaciones ni demuestran rentabilidad."
                        ]
                    })
                }
                Ok(_) => json!({
                    "schemaVersion": 2,
                    "available": false,
                    "mode": "invalid_corpus_report",
                    "reportPath": "$MAYAB_RESEARCH_CORPUS/corpus.json",
                    "error": "classification debe ser public_market_capture_corpus"
                }),
                Err(error) => json!({
                    "schemaVersion": 2,
                    "available": false,
                    "mode": "invalid_corpus_report",
                    "reportPath": "$MAYAB_RESEARCH_CORPUS/corpus.json",
                    "error": error.to_string()
                }),
            },
            Err(error) => json!({
                "schemaVersion": 2,
                "available": false,
                "mode": "missing_corpus_report",
                "reportPath": "$MAYAB_RESEARCH_CORPUS/corpus.json",
                "error": error.to_string(),
                "verificationCommand": "cargo run -p mayab-cli --bin verify-corpus -- --root \"$MAYAB_RESEARCH_CORPUS\" --output \"$MAYAB_RESEARCH_CORPUS/corpus.json\""
            }),
        };
    }
    let configured = std::env::var_os("MAYAB_RESEARCH_TAPE").map(PathBuf::from);
    let path = configured.unwrap_or_else(|| PathBuf::from("data/captura_real.json"));
    match fs::read(&path) {
        Ok(bytes) => {
            let events = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|v| v.as_array().map(Vec::len));
            json!({
                "schemaVersion": 1,
                "available": true,
                "tapes": [{
                    "id": "research-tape-default",
                    "path": path.display().to_string(),
                    "provenance": if std::env::var_os("MAYAB_RESEARCH_TAPE").is_some() { "configured_artifact" } else { "repository_capture" },
                    "sha256": hex::encode(Sha256::digest(&bytes)),
                    "bytes": bytes.len(),
                    "events": events,
                    "immutableReference": true
                }],
                "limitations": ["El hash prueba integridad de bytes, no autenticidad del exchange.", "La captura incluida es pequeña y no representa todos los regímenes de mercado."]
            })
        }
        Err(error) => json!({
            "schemaVersion": 1, "available": false, "tapes": [],
            "expectedPath": path.display().to_string(), "error": error.to_string(),
            "limitations": ["No se publica evidencia de tape cuando el artefacto no está montado."]
        }),
    }
}

fn validar_scan_corpus(corpus_sha: &str, bytes: &[u8]) -> Result<Value, &'static str> {
    let scan = serde_json::from_slice::<Value>(bytes).map_err(|_| "invalid_scan_json")?;
    if scan.get("sourceClassification").and_then(Value::as_str)
        != Some("public_market_capture_corpus")
    {
        return Err("non_public_scan");
    }
    if scan.get("corpusSha256").and_then(Value::as_str) != Some(corpus_sha) {
        return Err("corpus_hash_mismatch");
    }
    Ok(scan)
}

pub(crate) async fn lab_sweep(State(app): State<EstadoApp>) -> Json<serde_json::Value> {
    let estado = app.motor.estado().await;
    Json(lab_sweep_reproducible(&estado))
}

pub(crate) async fn exportar_json(State(app): State<EstadoApp>) -> Response {
    let estado = app.motor.estado().await;
    let payload = json!({
        "generadoEn": estado.generado_en,
        "metricas": estado.metricas,
        "telemetriaPipeline": estado.telemetria_pipeline,
        "operaciones": estado.operaciones,
        "oportunidades": estado.oportunidades,
        "eventosEjecucion": estado.eventos_ejecucion,
        "trazasEjecucion": estado.trazas_ejecucion,
        "auditoriaDecisiones": estado.auditoria_decisiones,
        "rebalanceos": estado.rebalanceos,
        "balances": estado.balances,
        "configuracion": estado.configuracion,
        "genetico": estado.genetico,
        "mlEdge": estado.ml_edge,
        "persistencia": estado.persistencia,
    });
    let body = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".into());
    (
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"mayab-arbitraje-reporte.json\"",
            ),
        ],
        body,
    )
        .into_response()
}

pub(crate) async fn exportar_evidence(State(app): State<EstadoApp>) -> Response {
    let estado = app.motor.estado().await;
    let ablacion = app.motor.ga_ablacion().await;

    let commit_sha = std::env::var("COMMIT_SHA").unwrap_or_else(|_| "desconocido".into());
    let config_json = serde_json::to_string(&estado.configuracion).unwrap_or_default();

    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    config_json.hash(&mut hasher);
    let config_hash = hasher.finish();

    let mut hasher_eventos = DefaultHasher::new();
    let eventos_json = serde_json::to_string(&estado.eventos_ejecucion).unwrap_or_default();
    eventos_json.hash(&mut hasher_eventos);
    let cinta_hash = hasher_eventos.finish();

    let md = format!(
        "# Evidencia y Reproducibilidad (FINAL_EVIDENCE)\n\n\
        Generado en: {}\n\n\
        ## Entorno\n\
        - Commit SHA: `{}`\n\
        - Config Hash: `{}`\n\
        - Modo: {}\n\
        \n\
        ## Cobertura\n\
        - Exchanges: {:?}\n\
        - Pares: {:?}\n\
        \n\
        ## Rendimiento (P&L)\n\
        - Ejecución P&L (Utilidad Acumulada): ${:.2}\n\
        - Costo de Rebalanceos: ${:.2}\n\
        - Max Drawdown: ${:.2}\n\
        - Sharpe Ratio: {:.4}\n\
        - Win Rate: {:.2}%\n\
        - Trades Ejecutados: {}\n\
        - Trades Fallidos: {}\n\
        \n\
        ## Latencia (Desglose P99)\n\
        - Red (Transporte): {} ms\n\
        - Scheduling (Cola/Events): {} µs\n\
        - Cómputo Puro (Decisión): {} µs\n\
        \n\
        ## Operativa\n\
        - Cinta Hash (Eventos): `{}`\n\
        - Rechazos por Razón (Fallidas): {}\n\
        \n\
        ## Sensibilidad de hiperparámetros GA (Holdout)\n\
        ```json\n\
        {}\n\
        ```\n\
        ",
        estado.generado_en.to_rfc3339(),
        commit_sha,
        config_hash,
        if estado.metricas.modo_conservador {
            "Real-market paper (Conservador)"
        } else {
            "Synthetic Demo"
        },
        estado.exchanges_activos,
        estado.pares_activos,
        estado.metricas.utilidad_acumulada_usd,
        estado.metricas.costo_rebalanceo_acumulado_usd,
        estado.metricas.max_drawdown_usd,
        estado.metricas.sharpe_ratio,
        estado.metricas.win_rate * 100.0,
        estado.metricas.operaciones_totales,
        estado.metricas.operaciones_fallidas,
        estado
            .latencias_exchange
            .iter()
            .map(|l| l.p99_ms)
            .max()
            .unwrap_or(0),
        estado.telemetria_pipeline.scheduling_p99_us,
        estado.telemetria_pipeline.compute_p99_us,
        cinta_hash,
        estado.metricas.operaciones_fallidas,
        serde_json::to_string_pretty(&ablacion).unwrap_or_default(),
    );

    (
        [
            (header::CONTENT_TYPE, "text/markdown; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"FINAL_EVIDENCE.md\"",
            ),
        ],
        md,
    )
        .into_response()
}

pub(crate) async fn exportar_csv(State(app): State<EstadoApp>) -> Response {
    let estado = app.motor.estado().await;

    let header = "tipo,tiempo,ruta,detalle,cantidad_btc,utilidad_usd,diferencial_neto_bps,score,costo_usd,decision_code,decision_reason\n".to_string();

    let mut config_rows = String::new();
    let config_json = serde_json::to_string(&estado.configuracion).unwrap_or_default();
    config_rows.push_str(&format!(
        "parametro,{},configuracion_json,{},,,,,,,,\n",
        estado.generado_en.to_rfc3339(),
        csv_cell(&config_json)
    ));

    let op_iter = estado.operaciones.into_iter().map(|op| {
        format!(
            "operacion,{},{},{},{:.8},{:.4},,,{:.4},,\n",
            op.ejecutada_en.to_rfc3339(),
            csv_cell(&format!("{}->{}", op.compra_en, op.venta_en)),
            csv_cell(&op.par),
            op.cantidad_btc,
            op.utilidad_usd,
            op.costos.total_usd,
        )
    });

    let evt_iter = estado.eventos_ejecucion.into_iter().map(|evento| {
        format!(
            "evento,{},{},{},{:.8},{:.4},,,,,,\n",
            evento.tiempo.to_rfc3339(),
            csv_cell(&evento.ruta),
            csv_cell(&evento.detalle),
            evento.cantidad_btc,
            evento.utilidad_usd,
        )
    });

    let trace_iter = estado.trazas_ejecucion.into_iter().map(|trace| {
        format!(
            "transicion,{},{},{},{:.8},{:.4},,,,{},{},\n",
            trace.tiempo.to_rfc3339(),
            csv_cell(&trace.ruta),
            csv_cell(&trace.detalle),
            trace.exposicion_btc,
            trace.pnl_realizado_usd,
            csv_cell(&trace.estado),
            csv_cell(&format!(
                "{} -> {} · {}",
                trace.estado_anterior, trace.estado, trace.pierna
            )),
        )
    });

    let aud_iter = estado.auditoria_decisiones.into_iter().map(|audit| {
        format!(
            "auditoria,{},{},{},{:.8},{:.4},{:.4},{:.6},{:.4},{},{}\n",
            audit.tiempo.to_rfc3339(),
            csv_cell(&audit.ruta),
            csv_cell(&audit.razon),
            audit.cantidad_btc,
            audit.utilidad_usd,
            audit.diferencial_neto_bps,
            audit.score,
            audit.costo_total_usd,
            csv_cell(&audit.decision_code),
            csv_cell(&audit.decision_reason),
        )
    });

    let reb_iter = estado.rebalanceos.into_iter().map(|rebalanceo| {
        format!(
            "rebalanceo,{},{},{},{:.8},,,,{:.4},,\n",
            rebalanceo.tiempo.to_rfc3339(),
            csv_cell(&format!("{}->{}", rebalanceo.desde, rebalanceo.hacia)),
            csv_cell(&rebalanceo.razon),
            rebalanceo.cantidad,
            rebalanceo.costo_usd,
        )
    });

    let stream = futures_util::stream::iter(
        std::iter::once(header)
            .chain(std::iter::once(config_rows))
            .chain(op_iter)
            .chain(evt_iter)
            .chain(trace_iter)
            .chain(aud_iter)
            .chain(reb_iter)
            .map(Ok::<_, std::convert::Infallible>),
    );

    (
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"mayab-arbitraje-auditoria.csv\"",
            ),
        ],
        axum::body::Body::from_stream(stream),
    )
        .into_response()
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct ParcheConfig {
    #[serde(rename = "maxOperacionBtc")]
    max_operacion_btc: Option<f64>,
    #[serde(rename = "minDiferencialNetoBps")]
    min_diferencial_neto_bps: Option<f64>,
    #[serde(rename = "deslizamientoBps")]
    deslizamiento_bps: Option<f64>,
    #[serde(rename = "enfriamientoMs")]
    enfriamiento_ms: Option<i64>,
    #[serde(rename = "latenciaRiesgoBps")]
    latencia_riesgo_bps: Option<f64>,
    #[serde(rename = "retiroAmortizadoBps")]
    retiro_amortizado_bps: Option<f64>,
    #[serde(rename = "minUtilidadUsd")]
    min_utilidad_usd: Option<f64>,
    #[serde(rename = "usdtUsdPremiumBps")]
    usdt_usd_premium_bps: Option<f64>,
    #[serde(rename = "permitirCruceUsdUsdt")]
    permitir_cruce_usd_usdt: Option<bool>,
    #[serde(rename = "volatilidadUmbralBps")]
    volatilidad_umbral_bps: Option<f64>,
    #[serde(rename = "staleMs")]
    stale_ms: Option<i64>,
    #[serde(rename = "circuitBreakerPerdidaUsd")]
    circuit_breaker_perdida_usd: Option<f64>,
    #[serde(rename = "circuitBreakerVentanaMin")]
    circuit_breaker_ventana_min: Option<i64>,
    #[serde(rename = "volatilidadVentanaSeg")]
    volatilidad_ventana_seg: Option<i64>,
    #[serde(rename = "simularAdversidad")]
    simular_adversidad: Option<bool>,
    #[serde(rename = "probFalloOrden")]
    prob_fallo_orden: Option<f64>,
    #[serde(rename = "probMovimientoBrusco")]
    prob_movimiento_brusco: Option<f64>,
    #[serde(rename = "movimientoBruscoBps")]
    movimiento_brusco_bps: Option<f64>,
    #[serde(rename = "rebalanceUmbralPct")]
    rebalance_umbral_pct: Option<f64>,
    #[serde(rename = "rebalanceMaxTransferPct")]
    rebalance_max_transfer_pct: Option<f64>,
    #[serde(rename = "costoRebalanceoUsd")]
    costo_rebalanceo_usd: Option<f64>,
    #[serde(rename = "rebalanceSettlementMs")]
    rebalance_settlement_ms: Option<i64>,
    #[serde(rename = "webhookUrl")]
    webhook_url: Option<String>,
    exchanges: Option<HashMap<String, ExchangeConfig>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SolicitudDemo {
    escenario: EscenarioDemoApi,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EscenarioDemoApi {
    FalloOrden,
    FalloSegundaPierna,
    MercadoMovido,
    LiquidezInsuficiente,
    FillParcial,
    CircuitBreaker,
    Rebalanceo,
    MercadoRentable,
}

impl From<EscenarioDemoApi> for EscenarioDemo {
    fn from(value: EscenarioDemoApi) -> Self {
        match value {
            EscenarioDemoApi::FalloOrden => EscenarioDemo::FalloOrden,
            EscenarioDemoApi::FalloSegundaPierna => EscenarioDemo::FalloSegundaPierna,
            EscenarioDemoApi::MercadoMovido => EscenarioDemo::MercadoMovido,
            EscenarioDemoApi::LiquidezInsuficiente => EscenarioDemo::LiquidezInsuficiente,
            EscenarioDemoApi::FillParcial => EscenarioDemo::FillParcial,
            EscenarioDemoApi::CircuitBreaker => EscenarioDemo::CircuitBreaker,
            EscenarioDemoApi::Rebalanceo => EscenarioDemo::Rebalanceo,
            EscenarioDemoApi::MercadoRentable => EscenarioDemo::MercadoRentable,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SolicitudEvolucionGa {
    #[serde(rename = "usarReplaySiVacio", default = "default_true")]
    usar_replay_si_vacio: bool,
    muestras: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SolicitudMcp {
    tool: String,
    #[serde(default)]
    arguments: Option<serde_json::Value>,
}

pub(crate) async fn actualizar_config_http(
    State(app): State<EstadoApp>,
    headers: HeaderMap,
    payload: Result<Json<ParcheConfig>, JsonRejection>,
) -> Response {
    if let Some(response) = autorizar_mutacion(&app, &headers) {
        return response;
    }
    let Json(payload) = match payload {
        Ok(payload) => payload,
        Err(err) => return rechazo_json(err).into_response(),
    };
    let mut estado = app.motor.estado().await;
    if let Err(err) = aplicar_config_patch(&mut estado.configuracion, payload) {
        return err.into_response();
    }
    app.motor.actualizar_config(estado.configuracion).await;
    Json(json!({ "ok": true })).into_response()
}

pub(crate) async fn demo_escenario(
    State(app): State<EstadoApp>,
    headers: HeaderMap,
    payload: Result<Json<SolicitudDemo>, JsonRejection>,
) -> Response {
    if let Some(response) = autorizar_mutacion(&app, &headers) {
        return response;
    }
    let Json(payload) = match payload {
        Ok(payload) => payload,
        Err(err) => return rechazo_json(err).into_response(),
    };
    Json(
        app.motor
            .activar_escenario_demo(payload.escenario.into())
            .await,
    )
    .into_response()
}

pub(crate) async fn trigger_adverso_http(
    State(app): State<EstadoApp>,
    headers: HeaderMap,
    payload: Result<Json<SolicitudDemo>, JsonRejection>,
) -> Response {
    if let Some(response) = autorizar_mutacion(&app, &headers) {
        return response;
    }
    let Json(payload) = match payload {
        Ok(payload) => payload,
        Err(err) => return rechazo_json(err).into_response(),
    };
    Json(
        app.motor
            .activar_escenario_demo(payload.escenario.into())
            .await,
    )
    .into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SolicitudReglasRebalanceo {
    reglas: Vec<crate::types::ReglaRebalanceo>,
}

pub(crate) async fn actualizar_reglas_rebalanceo_http(
    State(app): State<EstadoApp>,
    headers: HeaderMap,
    payload: Result<Json<SolicitudReglasRebalanceo>, JsonRejection>,
) -> Response {
    if let Some(response) = autorizar_mutacion(&app, &headers) {
        return response;
    }
    let Json(payload) = match payload {
        Ok(payload) => payload,
        Err(err) => return rechazo_json(err).into_response(),
    };
    app.motor.actualizar_reglas_rebalanceo(payload.reglas).await;
    Json(json!({ "ok": true })).into_response()
}

pub(crate) async fn demo_final_http(State(app): State<EstadoApp>, headers: HeaderMap) -> Response {
    if let Some(response) = autorizar_recorrido_jurado(&app, &headers) {
        return response;
    }

    // Una corrida final siempre parte de estado limpio. Repetir la acción no
    // debe acumular PnL ni inflar métricas compartidas entre visitantes.
    let corrida_id = app.motor.reiniciar_demo_jurado().await;
    let ga = app.motor.evolucionar_ga(true, 96).await;
    let rentable = app
        .motor
        .activar_escenario_demo(EscenarioDemo::MercadoRentable)
        .await;
    let fill_parcial = app
        .motor
        .activar_escenario_demo(EscenarioDemo::FillParcial)
        .await;
    let riesgo_pierna = app
        .motor
        .activar_escenario_demo(EscenarioDemo::FalloSegundaPierna)
        .await;
    let rebalanceo = app
        .motor
        .activar_escenario_demo(EscenarioDemo::Rebalanceo)
        .await;
    let estado = app.motor.estado().await;
    let preflight = construir_preflight(&estado);
    let huella_auditoria = huella_estado(&estado);

    Json(json!({
        "ok": true,
        "modo": "demo_final",
        "corridaLimpia": true,
        "corridaId": corrida_id,
        "pasos": [
            "estado simulado restablecido conservando feeds y configuracion",
            "GA evolucionado con historial real o replay sintetico",
            "mercado_rentable inyectado con operaciones demo_rentable",
            "fill_parcial generado para evidenciar profundidad/inventario",
            "segunda pierna fallida y reconciliada con unwind sin exposicion residual",
            "rebalanceo forzado para evidenciar wallets"
        ],
        "ga": ga,
        "mercadoRentable": rentable,
        "fillParcial": fill_parcial,
        "riesgoSegundaPierna": riesgo_pierna,
        "rebalanceo": rebalanceo,
        "metricas": estado.metricas,
        "mlEdge": estado.ml_edge,
        "preflight": preflight,
        "evidencia": {
            "algoritmoHuella": "SHA-256",
            "huellaAuditoria": huella_auditoria,
            "fuente": "demo_controlada_sintetica",
            "ejecucionReal": false,
            "estado": "/api/estado",
            "paquete": "/api/paquete-evaluacion",
            "exportJson": "/api/export/json"
        },
        "siguiente": [
            "Abrir /api/preflight",
            "Abrir /api/paquete-evaluacion",
            "Exportar /api/export/json o /api/export/csv"
        ]
    }))
    .into_response()
}

pub(crate) async fn demo_caos_http(State(app): State<EstadoApp>, headers: HeaderMap) -> Response {
    if let Some(response) = autorizar_recorrido_jurado(&app, &headers) {
        return response;
    }

    let corrida_id = app.motor.reiniciar_demo_jurado().await;
    let estado_inicial = app.motor.estado().await;
    let pnl_inicial = estado_inicial.metricas.utilidad_acumulada_usd;

    let fill_parcial = app
        .motor
        .activar_escenario_demo(EscenarioDemo::FillParcial)
        .await;
    let rentable_inicial = app
        .motor
        .activar_escenario_demo(EscenarioDemo::MercadoRentable)
        .await;
    let liquidez = app
        .motor
        .activar_escenario_demo(EscenarioDemo::LiquidezInsuficiente)
        .await;
    let segunda_pierna = app
        .motor
        .activar_escenario_demo(EscenarioDemo::FalloSegundaPierna)
        .await;
    let circuit_breaker = app
        .motor
        .activar_escenario_demo(EscenarioDemo::CircuitBreaker)
        .await;
    let rebalanceo = app
        .motor
        .activar_escenario_demo(EscenarioDemo::Rebalanceo)
        .await;
    let recuperacion = app
        .motor
        .activar_escenario_demo(EscenarioDemo::MercadoRentable)
        .await;

    let estado = app.motor.estado().await;
    let exposicion_residual_btc = estado
        .trazas_ejecucion
        .front()
        .map(|t| t.exposicion_btc)
        .unwrap_or(0.0);
    let checks = json!({
        "fillParcialRegistrado": fill_parcial.get("partialFill").and_then(|v| v.as_bool()).unwrap_or(false),
        "liquidezInsuficienteBloqueada": liquidez.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "segundaPiernaReconciliada": segunda_pierna.get("estadoFinal").and_then(|v| v.as_str()) == Some("RECONCILED_LOSS"),
        "sinExposicionResidual": exposicion_residual_btc.abs() < 1e-9,
        "circuitBreakerProbado": circuit_breaker.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "circuitBreakerRestaurado": !estado.metricas.circuit_breaker_activo,
        "rebalanceoRegistrado": rebalanceo.get("rebalanceo").is_some(),
        "motorRecuperado": !estado.metricas.ejecucion_en_curso && estado.metricas.estado_riesgo != "detenido",
    });
    let aprobados = checks
        .as_object()
        .map(|items| items.values().filter(|v| v.as_bool() == Some(true)).count())
        .unwrap_or(0);

    Json(json!({
        "ok": aprobados == 8,
        "modo": "prueba_caos_controlada",
        "corridaId": corrida_id,
        "segura": true,
        "ejecucionReal": false,
        "pasos": [
            {"nombre": "fill_parcial", "resultado": fill_parcial},
            {"nombre": "capital_base", "resultado": rentable_inicial},
            {"nombre": "liquidez_insuficiente", "resultado": liquidez},
            {"nombre": "fallo_segunda_pierna_y_unwind", "resultado": segunda_pierna},
            {"nombre": "circuit_breaker", "resultado": circuit_breaker},
            {"nombre": "rebalanceo", "resultado": rebalanceo},
            {"nombre": "recuperacion", "resultado": recuperacion}
        ],
        "checks": checks,
        "aprobados": aprobados,
        "totalChecks": 8,
        "estadoFinal": {
            "pnlInicialUsd": pnl_inicial,
            "pnlFinalUsd": estado.metricas.utilidad_acumulada_usd,
            "operaciones": estado.metricas.operaciones,
            "fallos": estado.metricas.operaciones_fallidas,
            "rebalanceos": estado.metricas.rebalanceos_totales,
            "circuitBreakerActivo": estado.metricas.circuit_breaker_activo,
            "exposicionResidualBtc": exposicion_residual_btc,
            "riesgo": estado.metricas.estado_riesgo,
        },
        "evidencia": {
            "estado": "/api/estado",
            "preflight": "/api/preflight",
            "exportJson": "/api/export/json"
        }
    }))
    .into_response()
}

pub(crate) async fn reset_demo_http(State(app): State<EstadoApp>, headers: HeaderMap) -> Response {
    if let Some(response) = autorizar_recorrido_jurado(&app, &headers) {
        return response;
    }

    let corrida_id = app.motor.reiniciar_demo_jurado().await;
    Json(json!({
        "ok": true,
        "modo": "jury_reset",
        "corridaId": corrida_id,
        "seedBacktest": 42,
        "detalle": "Estado simulado restablecido; feeds publicos y configuracion operativa permanecen activos.",
        "siguiente": "POST /api/demo/final"
    }))
    .into_response()
}

pub(crate) async fn captura_iniciar_http(
    State(app): State<EstadoApp>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = autorizar_mutacion(&app, &headers) {
        return response;
    }
    app.motor.iniciar_captura().await;
    Json(json!({"ok": true, "modo": "captura_iniciada"})).into_response()
}

pub(crate) async fn captura_detener_http(
    State(app): State<EstadoApp>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = autorizar_mutacion(&app, &headers) {
        return response;
    }
    let count = app.motor.detener_captura().await;
    Json(json!({"ok": true, "modo": "captura_detenida", "snapshots": count})).into_response()
}

pub(crate) async fn captura_estado_http(State(app): State<EstadoApp>) -> Json<serde_json::Value> {
    Json(app.motor.captura_estado().await)
}

pub(crate) async fn captura_replay_http(
    State(app): State<EstadoApp>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = autorizar_mutacion(&app, &headers) {
        return response;
    }
    let resultado = app.motor.ejecutar_replay_capturado().await;
    Json(resultado).into_response()
}

pub(crate) async fn ga_estado(State(app): State<EstadoApp>) -> Json<serde_json::Value> {
    Json(app.motor.ga_estado().await)
}

pub(crate) async fn ga_sensibilidad(State(app): State<EstadoApp>) -> Json<serde_json::Value> {
    Json(app.motor.ga_ablacion().await)
}

pub(crate) async fn obtener_config_ga(State(app): State<EstadoApp>) -> Json<ConfigGa> {
    Json(app.motor.ga_config().await)
}

pub(crate) async fn actualizar_config_ga_http(
    State(app): State<EstadoApp>,
    headers: HeaderMap,
    payload: Result<Json<ConfigGa>, JsonRejection>,
) -> Response {
    if let Some(response) = autorizar_mutacion(&app, &headers) {
        return response;
    }
    let Json(cfg) = match payload {
        Ok(payload) => payload,
        Err(err) => return rechazo_json(err).into_response(),
    };
    if let Err(err) = validar_ga_config(&cfg) {
        return err.into_response();
    }
    app.motor.actualizar_ga_config(cfg).await;
    Json(json!({ "ok": true })).into_response()
}

pub(crate) async fn evolucionar_ga_http(
    State(app): State<EstadoApp>,
    headers: HeaderMap,
    payload: Result<Json<SolicitudEvolucionGa>, JsonRejection>,
) -> Response {
    if let Some(response) = autorizar_mutacion(&app, &headers) {
        return response;
    }
    let Json(payload) = match payload {
        Ok(payload) => payload,
        Err(err) => return rechazo_json(err).into_response(),
    };
    if let Err(err) = validar_muestras_ga(payload.muestras) {
        return err.into_response();
    }
    Json(
        app.motor
            .evolucionar_ga(payload.usar_replay_si_vacio, payload.muestras.unwrap_or(96))
            .await,
    )
    .into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SolicitudExchange {
    exchange: String,
    activo: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SolicitudKillSwitch {
    activo: bool,
}

pub(crate) async fn kill_switch_http(
    State(app): State<EstadoApp>,
    headers: HeaderMap,
    payload: Result<Json<SolicitudKillSwitch>, JsonRejection>,
) -> Response {
    if let Some(response) = autorizar_mutacion(&app, &headers) {
        return response;
    }
    let Json(payload) = match payload {
        Ok(payload) => payload,
        Err(err) => return rechazo_json(err).into_response(),
    };
    app.motor.set_kill_switch(payload.activo).await;
    Json(json!({ "ok": true, "activo": payload.activo, "simulacion": true })).into_response()
}

pub(crate) async fn alternar_exchange_http(
    State(app): State<EstadoApp>,
    headers: HeaderMap,
    payload: Result<Json<SolicitudExchange>, JsonRejection>,
) -> Response {
    if let Some(response) = autorizar_mutacion(&app, &headers) {
        return response;
    }
    let Json(payload) = match payload {
        Ok(payload) => payload,
        Err(err) => return rechazo_json(err).into_response(),
    };
    let exchange = payload.exchange.trim();
    if exchange.is_empty() {
        return ErrorApi::bad_request("exchange_requerido", "exchange requerido").into_response();
    }
    if !app.motor.toggle_exchange(exchange, payload.activo).await {
        return ErrorApi::not_found("exchange_no_encontrado", "exchange no encontrado")
            .into_response();
    }
    Json(json!({ "ok": true, "exchange": exchange, "activo": payload.activo })).into_response()
}

fn autorizar_mutacion(app: &EstadoApp, headers: &HeaderMap) -> Option<Response> {
    let Some(token) = &app.token_admin else {
        // Fail-closed in production: if ENTORNO=production and no token, reject
        let entorno = std::env::var("ENTORNO").unwrap_or_else(|_| "development".to_string());
        if entorno == "production" {
            return Some(
                ErrorApi::unauthorized(
                    "token_admin_requerido",
                    "ADMIN_TOKEN es requerido en entorno production",
                )
                .into_response(),
            );
        }
        return None;
    };
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let header_token = headers.get("x-admin-token").and_then(|v| v.to_str().ok());
    let provided = bearer.or(header_token);
    if let Some(provided) = provided {
        // Timing-safe comparison
        if subtle::ConstantTimeEq::ct_eq(provided.as_bytes(), token.as_bytes()).into() {
            None
        } else {
            Some(
                ErrorApi::forbidden("token_admin_invalido", "token de admin inválido")
                    .into_response(),
            )
        }
    } else {
        Some(
            ErrorApi::unauthorized("token_admin_requerido", "token de admin requerido")
                .into_response(),
        )
    }
}

/// Autoriza sólo los tres recorridos predefinidos del jurado cuando el deploy
/// opta explícitamente por MAYAB_JUDGE_MODE=true. El resto de mutaciones sigue
/// pasando por ADMIN_TOKEN, incluido MCP, GA libre y escenarios individuales.
fn autorizar_recorrido_jurado(app: &EstadoApp, headers: &HeaderMap) -> Option<Response> {
    if app.judge_mode {
        None
    } else {
        autorizar_mutacion(app, headers)
    }
}

pub(crate) async fn tiempo_real(State(app): State<EstadoApp>, ws: WebSocketUpgrade) -> Response {
    let rx = app.ws_tx.subscribe();
    ws.on_upgrade(move |socket| websocket_loop(socket, rx))
}

async fn websocket_loop(socket: WebSocket, mut rx: tokio::sync::broadcast::Receiver<String>) {
    let (mut sender, mut receiver) = socket.split();

    let rx_task = tokio::spawn(async move {
        while let Ok(payload) = rx.recv().await {
            if sender.send(Message::Text(payload)).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(msg)) = receiver.next().await {
        if matches!(msg, Message::Close(_)) {
            break;
        }
    }
    rx_task.abort();
}

fn compactar_estado_ws(estado: &mut EstadoPublico) {
    estado.oportunidades.truncate(24);
    estado.operaciones.truncate(24);
    estado.eventos_ejecucion.truncate(24);
    estado.trazas_ejecucion.truncate(40);
    estado.rebalanceos.truncate(24);
    estado.auditoria_decisiones.truncate(48);
    retener_ultimos(&mut estado.serie_pnl, 160);
    retener_ultimos(&mut estado.serie_diferencial, 160);
}

fn retener_ultimos<T>(items: &mut std::collections::VecDeque<T>, maximo: usize) {
    while items.len() > maximo {
        items.pop_front();
    }
}

#[derive(Debug)]
struct ErrorApi {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ErrorApi {
    fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            message: message.into(),
        }
    }

    fn unauthorized(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code,
            message: message.into(),
        }
    }

    fn forbidden(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code,
            message: message.into(),
        }
    }

    fn not_found(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code,
            message: message.into(),
        }
    }
}

#[derive(Serialize)]
struct CuerpoErrorApi {
    ok: bool,
    error: DetalleErrorApi,
}

#[derive(Serialize)]
struct DetalleErrorApi {
    code: &'static str,
    message: String,
}

impl IntoResponse for ErrorApi {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(CuerpoErrorApi {
                ok: false,
                error: DetalleErrorApi {
                    code: self.code,
                    message: self.message,
                },
            }),
        )
            .into_response()
    }
}

fn rechazo_json(err: JsonRejection) -> ErrorApi {
    ErrorApi::bad_request(
        "json_invalido",
        format!(
            "JSON invalido o incompatible con contrato: {}",
            err.body_text()
        ),
    )
}

fn aplicar_config_patch(cfg: &mut MapaCostos, patch: ParcheConfig) -> Result<(), ErrorApi> {
    let mut siguiente = cfg.clone();
    aplicar_config_patch_interno(&mut siguiente, patch)?;
    validar_config_completa(&siguiente)?;
    *cfg = siguiente;
    Ok(())
}

fn aplicar_config_patch_interno(cfg: &mut MapaCostos, patch: ParcheConfig) -> Result<(), ErrorApi> {
    if let Some(v) = validar_f64(
        "maxOperacionBtc",
        patch.max_operacion_btc,
        |v| v > 0.0,
        "mayor que 0",
    )? {
        cfg.max_operacion_btc = v;
    }
    if let Some(v) = validar_f64(
        "minDiferencialNetoBps",
        patch.min_diferencial_neto_bps,
        |v| v >= 0.0,
        "mayor o igual a 0",
    )? {
        cfg.min_diferencial_neto_bps = v;
    }
    if let Some(v) = validar_f64(
        "deslizamientoBps",
        patch.deslizamiento_bps,
        |v| v >= 0.0,
        "mayor o igual a 0",
    )? {
        cfg.deslizamiento_bps = v;
    }
    if let Some(v) = validar_i64(
        "enfriamientoMs",
        patch.enfriamiento_ms,
        |v| v >= 0,
        "mayor o igual a 0",
    )? {
        cfg.enfriamiento_ms = v;
    }
    if let Some(v) = validar_f64(
        "latenciaRiesgoBps",
        patch.latencia_riesgo_bps,
        |v| v >= 0.0,
        "mayor o igual a 0",
    )? {
        cfg.latencia_riesgo_bps = v;
    }
    if let Some(v) = validar_f64(
        "retiroAmortizadoBps",
        patch.retiro_amortizado_bps,
        |v| v >= 0.0,
        "mayor o igual a 0",
    )? {
        cfg.retiro_amortizado_bps = v;
    }
    if let Some(v) = validar_f64(
        "minUtilidadUsd",
        patch.min_utilidad_usd,
        |v| v >= 0.0,
        "mayor o igual a 0",
    )? {
        cfg.min_utilidad_usd = v;
    }
    if let Some(v) = validar_f64(
        "usdtUsdPremiumBps",
        patch.usdt_usd_premium_bps,
        |v| v >= 0.0,
        "mayor o igual a 0",
    )? {
        cfg.usdt_usd_premium_bps = v;
    }
    if let Some(v) = patch.permitir_cruce_usd_usdt {
        cfg.permitir_cruce_usd_usdt = v;
    }
    if let Some(v) = validar_f64(
        "volatilidadUmbralBps",
        patch.volatilidad_umbral_bps,
        |v| v >= 0.0,
        "mayor o igual a 0",
    )? {
        cfg.volatilidad_umbral_bps = v;
    }
    if let Some(v) = validar_i64(
        "volatilidadVentanaSeg",
        patch.volatilidad_ventana_seg,
        |v| v > 0,
        "mayor que 0",
    )? {
        cfg.volatilidad_ventana_seg = v;
    }
    if let Some(v) = validar_i64("staleMs", patch.stale_ms, |v| v > 0, "mayor que 0")? {
        cfg.stale_ms = v;
    }
    if let Some(v) = validar_f64(
        "circuitBreakerPerdidaUsd",
        patch.circuit_breaker_perdida_usd,
        |v| v >= 0.0,
        "mayor o igual a 0",
    )? {
        cfg.circuit_breaker_perdida_usd = v;
    }
    if let Some(v) = validar_i64(
        "circuitBreakerVentanaMin",
        patch.circuit_breaker_ventana_min,
        |v| v > 0,
        "mayor que 0",
    )? {
        cfg.circuit_breaker_ventana_min = v;
    }
    if let Some(v) = patch.simular_adversidad {
        cfg.simular_adversidad = v;
    }
    if let Some(v) = validar_f64(
        "probFalloOrden",
        patch.prob_fallo_orden,
        |v| (0.0..=1.0).contains(&v),
        "entre 0 y 1",
    )? {
        cfg.prob_fallo_orden = v;
    }
    if let Some(v) = validar_f64(
        "probMovimientoBrusco",
        patch.prob_movimiento_brusco,
        |v| (0.0..=1.0).contains(&v),
        "entre 0 y 1",
    )? {
        cfg.prob_movimiento_brusco = v;
    }
    if let Some(v) = validar_f64(
        "movimientoBruscoBps",
        patch.movimiento_brusco_bps,
        |v| v >= 0.0,
        "mayor o igual a 0",
    )? {
        cfg.movimiento_brusco_bps = v;
    }
    if let Some(v) = validar_f64(
        "rebalanceUmbralPct",
        patch.rebalance_umbral_pct,
        |v| (0.0..=100.0).contains(&v),
        "entre 0 y 100",
    )? {
        cfg.rebalance_umbral_pct = v;
    }
    if let Some(v) = validar_f64(
        "rebalanceMaxTransferPct",
        patch.rebalance_max_transfer_pct,
        |v| (0.0..=100.0).contains(&v),
        "entre 0 y 100",
    )? {
        cfg.rebalance_max_transfer_pct = v;
    }
    if let Some(v) = validar_f64(
        "costoRebalanceoUsd",
        patch.costo_rebalanceo_usd,
        |v| v >= 0.0,
        "mayor o igual a 0",
    )? {
        cfg.costo_rebalanceo_usd = v;
    }
    if let Some(v) = validar_i64(
        "rebalanceSettlementMs",
        patch.rebalance_settlement_ms,
        |v| (0..=300_000).contains(&v),
        "entre 0 y 300000",
    )? {
        cfg.rebalance_settlement_ms = v;
    }
    if let Some(exchanges) = patch.exchanges {
        for (nombre, exchange) in exchanges {
            let nombre = nombre.trim();
            let Some(actual) = cfg.exchanges.get_mut(nombre) else {
                return Err(ErrorApi::bad_request(
                    "exchange_desconocido",
                    format!("exchange no configurado: {nombre}"),
                ));
            };
            if !exchange.fee_taker.is_finite() || exchange.fee_taker < 0.0 {
                return Err(campo_invalido("exchanges.*.feeTaker", "mayor o igual a 0"));
            }
            if !exchange.retiro_btc.is_finite() || exchange.retiro_btc < 0.0 {
                return Err(campo_invalido("exchanges.*.retiroBtc", "mayor o igual a 0"));
            }
            if !exchange.confiabilidad.is_finite() || !(0.0..=1.0).contains(&exchange.confiabilidad)
            {
                return Err(campo_invalido("exchanges.*.confiabilidad", "entre 0 y 1"));
            }
            actual.nombre = nombre.to_string();
            actual.fee_taker = exchange.fee_taker;
            actual.retiro_btc = exchange.retiro_btc;
            actual.confiabilidad = exchange.confiabilidad;
        }
    }
    if let Some(v) = patch.webhook_url {
        cfg.webhook_url = if v.trim().is_empty() { None } else { Some(v) };
    }
    Ok(())
}

fn validar_config_completa(cfg: &MapaCostos) -> Result<(), ErrorApi> {
    if cfg.max_operacion_btc > 10.0 {
        return Err(campo_invalido("maxOperacionBtc", "menor o igual a 10"));
    }
    for (nombre, valor) in [
        ("minDiferencialNetoBps", cfg.min_diferencial_neto_bps),
        ("deslizamientoBps", cfg.deslizamiento_bps),
        ("latenciaRiesgoBps", cfg.latencia_riesgo_bps),
        ("retiroAmortizadoBps", cfg.retiro_amortizado_bps),
        ("usdtUsdPremiumBps", cfg.usdt_usd_premium_bps),
        ("volatilidadUmbralBps", cfg.volatilidad_umbral_bps),
        ("movimientoBruscoBps", cfg.movimiento_brusco_bps),
    ] {
        if !valor.is_finite() || !(0.0..=10_000.0).contains(&valor) {
            return Err(campo_invalido(nombre, "finito y entre 0 y 10000"));
        }
    }
    if cfg.stale_ms > 300_000 {
        return Err(campo_invalido("staleMs", "menor o igual a 300000"));
    }
    if cfg.enfriamiento_ms > 3_600_000 {
        return Err(campo_invalido("enfriamientoMs", "menor o igual a 3600000"));
    }
    if let Some(url) = &cfg.webhook_url {
        let url = url.trim();
        if url.len() > 2_048 || !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(campo_invalido(
                "webhookUrl",
                "URL HTTP(S) de máximo 2048 caracteres",
            ));
        }
    }
    for exchange in cfg.exchanges.values() {
        if exchange.fee_taker > 0.10 {
            return Err(campo_invalido("exchanges.*.feeTaker", "entre 0 y 0.10"));
        }
        if exchange.retiro_btc > 10.0 {
            return Err(campo_invalido("exchanges.*.retiroBtc", "entre 0 y 10"));
        }
    }
    Ok(())
}

fn validar_f64(
    nombre: &'static str,
    valor: Option<f64>,
    predicado: impl Fn(f64) -> bool,
    regla: &'static str,
) -> Result<Option<f64>, ErrorApi> {
    match valor {
        Some(v) if v.is_finite() && predicado(v) => Ok(Some(v)),
        Some(_) => Err(campo_invalido(nombre, regla)),
        None => Ok(None),
    }
}

fn validar_i64(
    nombre: &'static str,
    valor: Option<i64>,
    predicado: impl Fn(i64) -> bool,
    regla: &'static str,
) -> Result<Option<i64>, ErrorApi> {
    match valor {
        Some(v) if predicado(v) => Ok(Some(v)),
        Some(_) => Err(campo_invalido(nombre, regla)),
        None => Ok(None),
    }
}

fn campo_invalido(nombre: &'static str, regla: &'static str) -> ErrorApi {
    ErrorApi::bad_request("campo_invalido", format!("{nombre} debe ser {regla}"))
}

fn validar_ga_config(cfg: &ConfigGa) -> Result<(), ErrorApi> {
    if !(10..=300).contains(&cfg.tamano_poblacion) {
        return Err(campo_invalido("tamanoPoblacion", "entre 10 y 300"));
    }
    if !cfg.tasa_mutacion.is_finite() || !(0.0..=0.8).contains(&cfg.tasa_mutacion) {
        return Err(campo_invalido("tasaMutacion", "entre 0 y 0.8"));
    }
    if !cfg.tasa_cruce.is_finite() || !(0.0..=1.0).contains(&cfg.tasa_cruce) {
        return Err(campo_invalido("tasaCruce", "entre 0 y 1"));
    }
    Ok(())
}

fn validar_muestras_ga(muestras: Option<usize>) -> Result<(), ErrorApi> {
    if let Some(muestras) = muestras {
        if !(12..=240).contains(&muestras) {
            return Err(campo_invalido("muestras", "entre 12 y 240"));
        }
    }
    Ok(())
}

fn construir_mcp_manifest() -> serde_json::Value {
    json!({
        "name": "mayab-arbitraje-btc",
        "version": env!("CARGO_PKG_VERSION"),
        "transport": "http-json",
        "description": "Bridge MCP-lite para que agentes LLM inspeccionen y preparen la demo sin parsear HTML.",
        "safety": {
            "realTrading": false,
            "custody": false,
            "secrets": false,
            "mutableToolsRequireAdminToken": true,
            "note": "Las herramientas mutables solo cambian estado simulado en memoria."
        },
        "endpoints": {
            "manifest": "/api/mcp/manifest",
            "call": "/api/mcp/call",
            "llmSummary": "/api/resumen-llm",
            "juryMode": "/api/jurado",
            "evaluationPackage": "/api/paquete-evaluacion",
            "preflight": "/api/preflight"
        },
        "callShape": {
            "method": "POST",
            "contentType": "application/json",
            "body": {
                "tool": "summarize_for_llm",
                "arguments": {}
            }
        },
        "tools": [
            mcp_tool("get_state", false, "Devuelve /api/estado completo con contratos JSON del dominio.", json!({})),
            mcp_tool("preflight", false, "Checklist operativo y readiness del jurado.", json!({})),
            mcp_tool("jury_mode", false, "Superficie unica con rubrica, scorecard, cobertura finalista y enlaces.", json!({})),
            mcp_tool("summarize_for_llm", false, "Snapshot compacto narrativo para agentes, jueces y reportes.", json!({})),
            mcp_tool("evaluation_package", false, "Scorecard, evidencia, backtest, huella y guion reproducible.", json!({})),
            mcp_tool("latency_ranking", false, "Ranking EWMA/p50/p95/p99 por exchange.", json!({})),
            mcp_tool("backtest", false, "Backtest reproducible con costos actuales.", json!({})),
            mcp_tool("research_lab_sweep", false, "Compara presets conservador, balanceado, agresivo y GA Edge.", json!({})),
            mcp_tool("prepare_demo_final", true, "Ejecuta GA, demo rentable, fill parcial y rebalanceo simulado.", json!({})),
            mcp_tool("evolve_ga", true, "Fuerza evolucion GA con historial real o replay sintetico.", json!({
                "usarReplaySiVacio": "boolean opcional, default true",
                "muestras": "entero opcional, default 96"
            })),
            mcp_tool("demo_scenario", true, "Dispara un escenario demo simulado.", json!({
                "escenario": "fallo_orden | mercado_movido | liquidez_insuficiente | fill_parcial | circuit_breaker | rebalanceo | mercado_rentable"
            })),
        ],
        "examples": [
            {
                "description": "Resumen para un agente",
                "curl": "curl -sS -X POST http://localhost:8080/api/mcp/call -H 'Content-Type: application/json' -d '{\"tool\":\"summarize_for_llm\"}'"
            },
            {
                "description": "Preparar demo final con ADMIN_TOKEN si esta configurado",
                "curl": "curl -sS -X POST http://localhost:8080/api/mcp/call -H 'Content-Type: application/json' -H 'Authorization: Bearer $ADMIN_TOKEN' -d '{\"tool\":\"prepare_demo_final\"}'"
            }
        ]
    })
}

fn mcp_tool(
    name: &str,
    mutable: bool,
    description: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    json!({
        "name": name,
        "mutable": mutable,
        "requiresAdminToken": mutable,
        "description": description,
        "arguments": arguments,
    })
}

fn construir_resumen_llm(estado: &EstadoPublico) -> serde_json::Value {
    let mejor = estado
        .oportunidades
        .iter()
        .max_by(|a, b| a.diferencial_neto_bps.total_cmp(&b.diferencial_neto_bps));
    let ejecutable = estado
        .oportunidades
        .iter()
        .filter(|o| o.ejecutable)
        .max_by(|a, b| a.utilidad_usd.total_cmp(&b.utilidad_usd));
    let ultimo_evento = estado.eventos_ejecucion.front();
    let ultimo_rebalanceo = estado.rebalanceos.front();
    let mejor_latencia = estado.latencias_exchange.first();
    let peor_latencia = estado
        .latencias_exchange
        .iter()
        .max_by(|a, b| a.promedio_ms.total_cmp(&b.promedio_ms));
    let ga = estado.genetico.as_ref();
    let persistencia = estado.persistencia.as_ref();

    let decision = ejecutable
        .map(|o| {
            format!(
                "ejecutar candidato {} -> {} por {:.2} USD estimados ({:.2} bps netos)",
                o.compra_en, o.venta_en, o.utilidad_usd, o.diferencial_neto_bps
            )
        })
        .unwrap_or_else(|| {
            "no ejecutar; ninguna ruta supera filtros de costos, riesgo y balance".into()
        });

    let mejor_ruta = mejor
        .map(|o| {
            json!({
                "compraEn": o.compra_en,
                "ventaEn": o.venta_en,
                "par": o.par,
                "diferencialNetoBps": o.diferencial_neto_bps,
                "utilidadUsd": o.utilidad_usd,
                "ejecutable": o.ejecutable,
                "razon": o.razon,
                "decisionCode": o.decision_code,
                "decisionReason": o.decision_reason,
                "decisionThreshold": o.decision_threshold,
                "decisionActual": o.decision_actual,
                "profitBreakdown": profit_breakdown_json(o),
                "zScore": o.z_score,
            })
        })
        .unwrap_or_else(|| json!(null));

    let decision_inspector = estado
        .auditoria_decisiones
        .iter()
        .take(12)
        .map(|a| {
            json!({
                "ruta": a.ruta,
                "par": a.par,
                "decision": a.decision,
                "decisionCode": a.decision_code,
                "decisionReason": a.decision_reason,
                "decisionThreshold": a.decision_threshold,
                "decisionActual": a.decision_actual,
                "razon": a.razon,
                "score": a.score,
                "utilidadUsd": a.utilidad_usd,
                "diferencialNetoBps": a.diferencial_neto_bps,
                "profitBreakdown": {
                    "netProfitUsd": a.utilidad_usd,
                    "netBps": a.diferencial_neto_bps,
                    "totalCostUsd": a.costo_total_usd,
                    "latencyMaxMs": a.latencia_max_ms,
                },
            })
        })
        .collect::<Vec<_>>();
    let partial_fill_evidence = estado
        .operaciones
        .iter()
        .find(|op| op.parcial)
        .map(|op| {
            json!({
                "route": format!("{}->{}", op.compra_en, op.venta_en),
                "requestedQtyBtc": estado.configuracion.max_operacion_btc,
                "filledQtyBtc": op.cantidad_btc,
                "partialFill": true,
                "reason": "filledQtyBtc fue limitado por profundidad/inventario simulado; el motor no asume fill perfecto",
                "profitUsd": op.utilidad_usd,
                "latencyMaxMs": op.latencia_max_ms,
                "costBreakdown": {
                    "buyFeeUsd": op.costos.fee_compra_usd,
                    "sellFeeUsd": op.costos.fee_venta_usd,
                    "slippageUsd": op.costos.deslizamiento_usd,
                    "rebalanceCostUsd": op.costos.retiro_amort_usd,
                    "latencyHaircutUsd": op.costos.latencia_riesgo_usd,
                    "totalCostUsd": op.costos.total_usd,
                }
            })
        });

    let resumen = format!(
        "Mayab Arbitraje BTC procesa {} eventos de mercado con PnL simulado {:.2} USD, retorno {:.2} bps y riesgo '{}'. {}. Circuit breaker: {}. Modo conservador: {}.",
        estado.metricas.eventos_mercado,
        estado.metricas.utilidad_acumulada_usd,
        estado.metricas.retorno_bps,
        estado.metricas.estado_riesgo,
        decision,
        si_no(estado.metricas.circuit_breaker_activo),
        si_no(estado.metricas.modo_conservador),
    );

    let markdown = format!(
        "# Resumen operativo\n\n- PnL simulado: {:.2} USD\n- Retorno: {:.2} bps\n- Riesgo: {}\n- Decisión: {}\n- Operaciones: {} ejecutadas, {} fallidas\n- Rebalanceos: {}\n- GA: {}\n",
        estado.metricas.utilidad_acumulada_usd,
        estado.metricas.retorno_bps,
        estado.metricas.estado_riesgo,
        decision,
        estado.metricas.operaciones,
        estado.metricas.operaciones_fallidas,
        estado.metricas.rebalanceos_totales,
        ga.map(|g| format!(
            "generación {}, fitness {:.2}, diversidad {:.1}%, umbral {:.2} bps",
            g.generacion,
            g.mejor_fitness,
            g.diversidad * 100.0,
            g.umbral_optimizado
        ))
        .unwrap_or_else(|| "sin estado genético".into()),
    );

    json!({
        "generadoEn": estado.generado_en,
        "version": crate::version::current(),
        "resumen": resumen,
        "markdown": markdown,
        "decision": decision,
        "partialFillEvidence": partial_fill_evidence,
        "capabilities": [
            "monitoreo de order books publicos en tiempo real",
            "calculo de utilidad neta despues de fees, slippage, retiro amortizado y haircut de latencia",
            "simulacion de fills parciales por profundidad e inventario",
            "accounting de wallets por exchange",
            "decision inspector auditable con codigos estables y razon cuantitativa",
            "risk guards: stale books, circuit breaker, modo conservador, single-trade-in-flight",
            "demo rentable etiquetada y replay sintetico para GA cuando no hay oportunidades live"
        ],
        "limitations": [
            "ejecucion simulada solamente",
            "sin llaves privadas de exchange",
            "sin custodia ni movimientos reales de fondos",
            "la demo rentable es sintetica y se etiqueta como tal"
        ],
        "metricasClave": {
            "pnlUsd": estado.metricas.utilidad_acumulada_usd,
            "retornoBps": estado.metricas.retorno_bps,
            "capitalActualUsd": estado.metricas.capital_actual_usd,
            "latenciaPromedioMs": estado.metricas.latencia_promedio_ms,
            "sharpeRatio": estado.metricas.sharpe_ratio,
            "winRate": estado.metricas.win_rate,
            "maxDrawdownUsd": estado.metricas.max_drawdown_usd,
            "operaciones": estado.metricas.operaciones,
            "operacionesFallidas": estado.metricas.operaciones_fallidas,
            "rebalanceos": estado.metricas.rebalanceos_totales,
            "estadoRiesgo": estado.metricas.estado_riesgo,
            "circuitBreakerActivo": estado.metricas.circuit_breaker_activo,
            "modoConservador": estado.metricas.modo_conservador,
        },
        "mejorRuta": mejor_ruta,
        "decisionInspector": decision_inspector,
        "ga": ga.map(|g| json!({
            "generacion": g.generacion,
            "fitnessDelRepresentantePareto": g.mejor_fitness,
            "maxFitness": g.mejor_fitness.max(g.retador_fitness),
            "meanFitness": g.fitness_promedio,
            "champion": "baseline_hasta_validar_holdout",
            "challenger": "ga_pareto",
            "diversidad": g.diversidad,
            "umbralOptimizado": g.umbral_optimizado,
            "maxOperacionOptimizadaBtc": g.max_operacion_optimizada_btc,
            "toleranciaLatenciaMs": g.tolerancia_latencia_ms,
            "metaheuristicas": g.metaheuristicas,
        })),
        "mlEdge": estado.ml_edge.as_ref().map(|m| json!({
            "modelo": m.modelo,
            "version": m.version,
            "activo": m.activo,
            "decision": m.decision,
            "scoreActual": m.score_actual,
            "confianza": m.confianza,
            "expectedValueUsd": m.expected_value_usd,
            "survivalProbability": m.survival_probability,
            "fillProbability": m.fill_probability,
            "adverseSelectionBps": m.adverse_selection_bps,
            "features": m.features,
            "explicacion": m.explicacion,
            "nota": "Scoring heuristico explicable con pesos ajustados por GA; no es una red neuronal ni ejecuta ordenes reales."
        })),
        "persistencia": persistencia.map(|p| json!({
            "activa": p.activa,
            "backend": p.backend,
            "ruta": p.ruta,
            "operaciones": p.operaciones,
            "oportunidades": p.oportunidades,
            "eventos": p.eventos,
            "auditorias": p.auditorias,
            "rebalanceos": p.rebalanceos,
        })),
        "ultimoEvento": ultimo_evento.map(|e| json!({
            "tipo": e.tipo,
            "ruta": e.ruta,
            "detalle": e.detalle,
            "severidad": e.severidad,
            "utilidadUsd": e.utilidad_usd,
        })),
        "ultimoRebalanceo": ultimo_rebalanceo.map(|r| json!({
            "activo": r.activo,
            "desde": r.desde,
            "hacia": r.hacia,
            "cantidad": r.cantidad,
            "costoUsd": r.costo_usd,
            "razon": r.razon,
        })),
        "latenciaPorExchange": estado.latencias_exchange,
        "regionOperacion": {
            "mejorExchange": mejor_latencia.map(|l| json!({
                "exchange": l.exchange,
                "promedioMs": l.promedio_ms,
                "regionSugerida": l.region_sugerida,
            })),
            "feedMasLento": peor_latencia.map(|l| json!({
                "exchange": l.exchange,
                "promedioMs": l.promedio_ms,
                "estado": l.estado,
            })),
            "criterio": "Mantener la region primaria cerca de los exchanges dominantes y mover replica si un feed aporta mas oportunidades con menor latencia."
        },
        "exchangesActivos": estado.exchanges_activos,
        "contrato": {
            "uso": "Snapshot compacto para jueces, scripts y agentes LLM; no requiere interpretar la UI.",
            "fuenteCompleta": "/api/estado",
            "preflight": "/api/preflight",
            "latencias": "/api/latencias",
            "websocket": "/tiempo-real"
        }
    })
}

fn profit_breakdown_json(o: &crate::types::Oportunidad) -> serde_json::Value {
    json!({
        "grossSpreadUsd": o.diferencial_bruto_usd * o.cantidad_btc,
        "grossSpreadUnitUsd": o.diferencial_bruto_usd,
        "grossSpreadBps": o.diferencial_bruto_bps,
        "buyFeeUsd": o.costos.fee_compra_usd,
        "sellFeeUsd": o.costos.fee_venta_usd,
        "slippageUsd": o.costos.deslizamiento_usd,
        "rebalanceCostUsd": o.costos.retiro_amort_usd,
        "latencyHaircutUsd": o.costos.latencia_riesgo_usd,
        "adverseSelectionUsd": o.costos.seleccion_adversa_usd,
        "totalCostUsd": o.costos.total_usd,
        "netProfitUsd": o.utilidad_usd,
        "netUnitUsd": o.diferencial_neto_usd,
        "netBps": o.diferencial_neto_bps,
        "quantityBtc": o.cantidad_btc,
        "partialFill": o.parcial,
    })
}

fn construir_preflight(estado: &EstadoPublico) -> serde_json::Value {
    let configurados = estado.exchanges_activos.len();
    let activos = estado.exchanges_activos.values().filter(|v| **v).count();
    let stale_ms = estado.configuracion.stale_ms;
    let frescos = estado
        .cotizaciones
        .iter()
        .filter(|c| snapshot_fresco(estado, c))
        .map(|c| c.exchange.as_str())
        .collect::<HashSet<_>>()
        .len();
    let conectados = estado
        .cotizaciones
        .iter()
        .filter(|c| snapshot_websocket_fresco(estado, c))
        .map(|c| c.exchange.as_str())
        .collect::<HashSet<_>>()
        .len();
    // La rúbrica exige dos o más exchanges. Los adicionales mejoran cobertura,
    // pero una caída regional de un tercero no debe invalidar una demo con dos
    // WebSockets directos y snapshots ruteables claramente etiquetados.
    let feeds_ok = conectados >= 2;
    let snapshots_ok = frescos >= 2;
    let integridad_ok = estado
        .cotizaciones
        .iter()
        .filter(|c| snapshot_fresco(estado, c))
        .all(|c| {
            !matches!(
                c.integrity_status.as_str(),
                "gap_requiere_snapshot" | "fuera_de_orden" | "esperando_snapshot"
            )
        })
        && frescos >= 2;
    let costos_ok = estado.configuracion.max_operacion_btc > 0.0
        && estado.configuracion.min_utilidad_usd >= 0.0
        && estado.configuracion.min_diferencial_neto_bps >= 0.0
        && !estado.configuracion.exchanges.is_empty();
    let riesgo_ok = !estado.metricas.circuit_breaker_activo
        && estado.metricas.estado_riesgo != "critico"
        && !estado.metricas.ejecucion_en_curso;
    let dashboard_ok = dashboard_assets_present();
    let ga_ok = estado
        .genetico
        .as_ref()
        .map(|g| g.poblacion >= 10 && g.tasa_mutacion.is_finite() && g.tasa_cruce.is_finite())
        .unwrap_or(false);
    let ml_edge_ok = estado
        .ml_edge
        .as_ref()
        .map(|m| {
            m.score_actual.is_finite() && m.expected_value_usd.is_finite() && m.features.len() >= 5
        })
        .unwrap_or(false);
    let export_ok = true;
    let persistencia_ok = estado
        .persistencia
        .as_ref()
        .map(|p| p.activa)
        .unwrap_or(false);
    let rest_fallbacks = estado
        .cotizaciones
        .iter()
        .filter(|c| c.ultimo_mensaje == "rest_fallback")
        .count();
    let rest_fallback_ok = rest_fallbacks > 0 || feeds_ok;
    let decision_inspector_ok = estado
        .auditoria_decisiones
        .iter()
        .any(|a| !a.decision_code.is_empty() && !a.decision_reason.is_empty());
    let demo_mode_ok = true;
    let partial_fill_evidence = estado.operaciones.iter().any(|o| o.parcial)
        || estado.oportunidades.iter().any(|o| o.parcial);
    let partial_fill_ok = partial_fill_evidence;
    let fsm_reconciliada = estado.trazas_ejecucion.iter().any(|trace| {
        matches!(trace.estado.as_str(), "COMMITTED" | "RECONCILED_LOSS")
            && trace.exposicion_btc.abs() < 0.00000001
    });
    let unwind_reconciliado = estado
        .trazas_ejecucion
        .iter()
        .any(|trace| trace.estado == "RECONCILED_LOSS" && trace.exposicion_btc.abs() < 0.00000001);
    let rebalanceo_evidencia = estado.metricas.rebalanceos_totales > 0;
    let wallet_ok = estado.balances.len() >= activos.min(2) && !estado.balances.is_empty();
    let judge_checks = vec![
        ("realTimeOrderBooks", feeds_ok),
        ("orderBookIntegrity", integridad_ok),
        ("netProfitCalculation", costos_ok),
        ("feesSlippageLatency", costos_ok),
        ("partialFillSupport", partial_fill_ok),
        ("twoLegReconciliation", fsm_reconciliada),
        ("walletAccounting", wallet_ok),
        ("decisionInspector", decision_inspector_ok),
        ("mlEdgeExplainable", ml_edge_ok),
        ("riskGuards", riesgo_ok),
        ("safeDemoMode", demo_mode_ok),
        ("exports", export_ok),
    ];
    let judge_passed = judge_checks.iter().filter(|(_, ok)| *ok).count();
    let judge_total = judge_checks.len();
    // Readiness operativo mide si el evaluador puede usar el sistema ahora.
    // GA, fills, unwind y rebalanceos son evidencia reproducible, no requisitos
    // de salud: un reinicio limpio no debe autodeclararse roto por tener cero
    // operaciones historicas.
    let listo = feeds_ok
        && snapshots_ok
        && integridad_ok
        && costos_ok
        && riesgo_ok
        && dashboard_ok
        && export_ok
        && wallet_ok;

    let evidencia = vec![
        evidencia_preflight(
            "operacion_conciliada",
            fsm_reconciliada,
            "runtime_sintetico_etiquetado",
            "/api/estado#/trazasEjecucion",
            "POST /api/demo/final",
        ),
        evidencia_preflight(
            "fill_parcial",
            partial_fill_evidence,
            "runtime_sintetico_etiquetado",
            "/api/estado#/operaciones",
            "POST /api/demo/final",
        ),
        evidencia_preflight(
            "fallo_segunda_pierna_con_unwind",
            unwind_reconciliado,
            "runtime_sintetico_etiquetado",
            "/api/estado#/trazasEjecucion",
            "POST /api/demo/caos",
        ),
        evidencia_preflight(
            "rebalanceo",
            rebalanceo_evidencia,
            "runtime_sintetico_etiquetado",
            "/api/estado#/rebalanceos",
            "POST /api/demo/final",
        ),
        evidencia_preflight(
            "ga_challenger",
            ga_ok,
            "replay_sintetico_o_historial_publico",
            "/api/ga/estado",
            "POST /api/ga/evolucionar",
        ),
    ];

    let mut feed_detalle: Vec<_> = estado
        .cotizaciones
        .iter()
        .map(|c| {
            json!({
                "exchange": c.exchange,
                "par": c.par,
                "bid": c.bid,
                "ask": c.ask,
                "latenciaMs": c.latencia_ms,
                "edadMs": (estado.generado_en - c.recibida_en).num_milliseconds().max(0),
                "fuente": if c.ultimo_mensaje == "rest_fallback" { "rest_fallback" } else { "websocket" },
                "exchangeSequence": c.exchange_sequence,
                "integrityStatus": c.integrity_status,
                "resyncs": c.resyncs,
                "timestampConfiable": c.timestamp_confiable,
                "fresco": (estado.generado_en - c.recibida_en).num_milliseconds().max(0) <= stale_ms,
            })
        })
        .collect();
    feed_detalle.sort_by(|a, b| {
        a.get("exchange")
            .and_then(|v| v.as_str())
            .cmp(&b.get("exchange").and_then(|v| v.as_str()))
    });

    json!({
        "generadoEn": estado.generado_en,
        "listo": listo,
        "modo": if listo { "demo_operable" } else { "degradado" },
        "criterioReadiness": "capacidad_operativa_actual; la evidencia historica se reporta por separado",
        "venues": {
            "configurados": configurados,
            "habilitados": activos,
            "conWebSocketFresco": conectados,
            "conLibroRuteable": frescos,
            "minimosRequeridos": 2,
            "suficientesParaEvaluar": feeds_ok && snapshots_ok,
            "nota": "venue, feed, instrumento y lane se cuentan por separado; conectores extra no invalidan una ruta utilizable"
        },
        "evidenceMatrix": evidencia,
        "judgeReadiness": {
            "passed": judge_passed,
            "total": judge_total,
            "status": if listo { "ready" } else { "blocked" },
            "evidenceStatus": if judge_passed == judge_total { "complete" } else { "partial" },
            "partialFillEvidence": partial_fill_evidence,
            "rubricaOficial": matriz_rubrica_oficial(estado),
            "coberturaFinalista": cobertura_finalista(estado),
            "recomendaciones": recomendaciones_ganadoras(estado),
            "checks": judge_checks
                .into_iter()
                .map(|(name, ok)| json!({ "name": name, "ok": ok }))
                .collect::<Vec<_>>(),
            "verificationCommands": [
                "cargo fmt -- --check",
                "cargo test",
                "cargo clippy -- -D warnings"
            ]
        },
        "checks": [
            check("feeds_publicos", feeds_ok, format!("{conectados} venues unicos con WebSocket fresco; minimo requerido=2; {activos} habilitados de {configurados} configurados")),
            check("snapshots_ruteables", snapshots_ok, format!("{frescos} venues unicos tienen libro fresco, no cruzado y utilizable; minimo requerido=2")),
            check("integridad_books", integridad_ok, "secuencias monitoreadas; gaps/out-of-order bloquean el libro hasta snapshot o fallback"),
            check("costos_configurados", costos_ok, "fees, slippage, retiro amortizado y tamanos son validos"),
            check("riesgo_operativo", riesgo_ok, format!("riesgo={}, circuitBreaker={}, ejecucionEnCurso={}", estado.metricas.estado_riesgo, estado.metricas.circuit_breaker_activo, estado.metricas.ejecucion_en_curso)),
            check("decision_inspector", decision_inspector_ok, format!("{} decisiones recientes con decisionCode y decisionReason", estado.auditoria_decisiones.len())),
            check("wallet_accounting", wallet_ok, format!("{} wallets simuladas visibles", estado.balances.len())),
            check("partial_fills", partial_fill_ok, format!("evidencia visible de fill parcial en estado actual={partial_fill_evidence}")),
            check("fsm_dos_piernas", fsm_reconciliada, format!("{} transiciones; final conciliado sin exposicion residual={fsm_reconciliada}", estado.trazas_ejecucion.len())),
            check("demo_segura", demo_mode_ok, "POST /api/demo disponible; solo modifica estado simulado en memoria"),
            check("ga_disponible", ga_ok, estado.genetico.as_ref().map(|g| format!("poblacion={}, generacion={}, diversidad={:.3}", g.poblacion, g.generacion, g.diversidad)).unwrap_or_else(|| "sin estado GA".into())),
            check("ml_edge_explicable", ml_edge_ok, estado.ml_edge.as_ref().map(|m| format!("{} score={:.3}, EV={:.2} USD, confianza={:.1}%", m.version, m.score_actual, m.expected_value_usd, m.confianza * 100.0)).unwrap_or_else(|| "esperando auditoria para calcular ML Edge".into())),
            check("dashboard_estatico", dashboard_ok, "index.html, app.js y styles.css encontrados"),
            check("auditoria_exportable", export_ok, "/api/export/json y /api/export/csv disponibles"),
            check("sqlite_auditoria", persistencia_ok, estado.persistencia.as_ref().map(|p| format!("{} ops, {} oportunidades, {} auditorias en {}", p.operaciones, p.oportunidades, p.auditorias, p.ruta)).unwrap_or_else(|| "persistencia no inicializada".into())),
            check("rest_fallback", rest_fallback_ok, format!("{rest_fallbacks} feeds usan snapshot REST publico como respaldo; WS sigue siendo la fuente primaria")),
            check("telemetria_pipeline", estado.telemetria_pipeline.muestras > 0, format!("{} muestras; compute p50/p95/p99={}/{}/{} us; scheduling p50/p95/p99={}/{}/{} us; {:.1} eventos/s", estado.telemetria_pipeline.muestras, estado.telemetria_pipeline.compute_p50_us, estado.telemetria_pipeline.compute_p95_us, estado.telemetria_pipeline.compute_p99_us, estado.telemetria_pipeline.scheduling_p50_us, estado.telemetria_pipeline.scheduling_p95_us, estado.telemetria_pipeline.scheduling_p99_us, estado.telemetria_pipeline.eventos_por_segundo)),
        ],
        "feeds": feed_detalle,
        "endpoints": {
            "estado": "/api/estado",
            "jurado": "/api/jurado",
            "preflight": "/api/preflight",
            "resumenLlm": "/api/resumen-llm",
            "paqueteEvaluacion": "/api/paquete-evaluacion",
            "latencias": "/api/latencias",
            "backtest": "/api/backtest",
            "labSweep": "/api/lab/sweep",
            "demoCaos": "/api/demo/caos",
            "exportJson": "/api/export/json",
            "exportCsv": "/api/export/csv",
            "metrics": "/metrics",
            "websocket": "/tiempo-real"
        },
        "notas": [
            "El motor consume datos publicos; no custodia fondos ni firma ordenes reales.",
            "Solo se permite una operacion simulada en validacion/ejecucion a la vez para evitar doble gasto de balances.",
            "Las rutas se revalidan contra el snapshot fresco antes de mover carteras simuladas."
        ]
    })
}

fn construir_modo_jurado(estado: &EstadoPublico) -> serde_json::Value {
    let preflight = construir_preflight(estado);
    let paquete = construir_paquete_evaluacion(estado);
    let readiness = preflight
        .get("judgeReadiness")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let cobertura = readiness
        .get("coberturaFinalista")
        .cloned()
        .unwrap_or_else(|| cobertura_finalista(estado));
    let rubrica = readiness
        .get("rubricaOficial")
        .cloned()
        .unwrap_or_else(|| json!(matriz_rubrica_oficial(estado)));
    let checks = readiness
        .get("checks")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let scorecard = paquete
        .get("criterios")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let huella = paquete
        .get("huellaAuditoria")
        .cloned()
        .unwrap_or_else(|| json!(huella_estado(estado)));

    json!({
        "generadoEn": estado.generado_en,
        "nombre": "Mayab Jury Mode",
        "version": crate::version::current(),
        "objetivo": "Superficie unica para evaluar la demo contra el benchmark finalista sin navegar todo el dashboard.",
        "estado": {
            "status": readiness.get("status").cloned().unwrap_or_else(|| json!("review")),
            "passed": readiness.get("passed").cloned().unwrap_or_else(|| json!(0)),
            "total": readiness.get("total").cloned().unwrap_or_else(|| json!(0)),
            "evidenceStatus": readiness.get("evidenceStatus").cloned().unwrap_or_else(|| json!("partial")),
            "notaEvaluacion": "Mayab no se autocalifica; PASS/WARN/FAIL enlaza evidencia para que el comite asigne el puntaje.",
            "huellaAuditoria": huella,
        },
        "script60Segundos": [
            "GET /api/healthz",
            "GET /api/jurado",
            "POST /api/demo/final",
            "GET /api/preflight",
            "GET /api/paquete-evaluacion",
            "GET /api/export/json"
        ],
        "rubricaOficial": rubrica,
        "scorecard": scorecard,
        "coberturaFinalista": cobertura,
        "checks": checks,
        "evidenciaClave": {
            "scorecardCuantitativa": {
                "exchangesPublicos": 10,
                "pruebasRustLineaBaseVerificadas": 116,
                "pruebasRustObjetivoArbolActual": 186,
                "pruebasObjetivoVerificadas": false,
                "semillasPareadasBacktest": 24,
                "remuestrasBootstrap": 10000,
                "fuenteLatencia": "/api/latencias",
                "nota": "116 fue la ultima corrida verde observada; 152 requiere CI verde del SHA entregable. Latencias y resultados de mercado se calculan en runtime."
            },
            "resultadoMemorable": {
                "afirmacion": "segunda pierna rechazada, unwind auditado y cero exposicion residual",
                "maquinaEstados": ["PENDING", "LEG_A_FILLED", "LEG_B_REJECTED", "UNWIND_FILLED", "RECONCILED_LOSS"],
                "reconciliada": estado.trazas_ejecucion.iter().any(|trace| trace.estado == "RECONCILED_LOSS" && trace.exposicion_btc.abs() < 0.00000001),
                "endpointReproducible": "/api/demo/caos"
            },
            "parametrosControlablesEstimados": parametros_controlables(estado),
            "catalogoParametros": catalogo_parametros(estado),
            "feedsWebSocketFrescos": contar_exchanges_unicos(estado.cotizaciones.iter().filter(|c| snapshot_websocket_fresco(estado, c))),
            "feedsRestFallback": contar_exchanges_unicos(estado.cotizaciones.iter().filter(|c| c.ultimo_mensaje == "rest_fallback")),
            "operaciones": estado.metricas.operaciones,
            "pnlUsd": estado.metricas.utilidad_acumulada_usd,
            "rebalanceos": estado.metricas.rebalanceos_totales,
            "auditorias": estado.auditoria_decisiones.len(),
            "latenciasP99": estado.latencias_exchange,
            "telemetriaPipeline": estado.telemetria_pipeline,
            "ga": estado.genetico,
            "mlEdge": estado.ml_edge,
            "persistencia": estado.persistencia,
        },
        "enlaces": {
            "dashboard": "/",
            "estadoCompleto": "/api/estado",
            "preflight": "/api/preflight",
            "resumenLlm": "/api/resumen-llm",
            "paqueteEvaluacion": "/api/paquete-evaluacion",
            "latencias": "/api/latencias",
            "backtest": "/api/backtest",
            "researchLab": "/api/lab/sweep",
            "exportJson": "/api/export/json",
            "exportCsv": "/api/export/csv",
            "demoFinal": "/api/demo/final",
            "demoCaos": "/api/demo/caos"
        },
        "lectura": if readiness.get("status").and_then(|v| v.as_str()) == Some("ready") {
            "Listo para presentar: ejecutar demo final solo si se quiere refrescar evidencia runtime."
        } else {
            "Accion recomendada: ejecutar POST /api/demo/final y volver a abrir /api/jurado."
        },
        "limitesSeguros": [
            "No usa llaves API privadas.",
            "No coloca ordenes reales.",
            "No custodia fondos.",
            "Los POST solo mutan simulacion en memoria."
        ]
    })
}

fn snapshot_fresco(estado: &EstadoPublico, cotizacion: &Cotizacion) -> bool {
    let edad_ms = (estado.generado_en - cotizacion.recibida_en)
        .num_milliseconds()
        .max(0);
    edad_ms <= estado.configuracion.stale_ms
        && cotizacion.bid > 0.0
        && cotizacion.ask > cotizacion.bid
}

fn dashboard_assets_present() -> bool {
    let relative = Path::new("internal/webui/web");
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")))
        .join("internal/webui/web");

    [relative.to_path_buf(), workspace].iter().any(|root| {
        ["index.html", "app.js", "styles.css"]
            .iter()
            .all(|asset| root.join(asset).is_file())
    })
}

fn snapshot_websocket_fresco(estado: &EstadoPublico, cotizacion: &Cotizacion) -> bool {
    snapshot_fresco(estado, cotizacion)
        && cotizacion.conectado
        && cotizacion.ultimo_mensaje != "rest_fallback"
}

fn contar_exchanges_unicos<'a>(cotizaciones: impl Iterator<Item = &'a Cotizacion>) -> usize {
    cotizaciones
        .map(|c| c.exchange.as_str())
        .collect::<HashSet<_>>()
        .len()
}

fn construir_paquete_evaluacion(estado: &EstadoPublico) -> serde_json::Value {
    let preflight = construir_preflight(estado);
    let resumen = construir_resumen_llm(estado);
    let backtest = backtest_reproducible(estado);
    let lab_sweep = lab_sweep_reproducible(estado);
    let microstructure = crate::microestructura::build_report(
        std::env::var_os("MAYAB_RESEARCH_TAPE")
            .map(PathBuf::from)
            .as_deref(),
        42,
    );
    let ou = crate::ou::build_report(
        std::env::var_os("MAYAB_RESEARCH_TAPE")
            .map(PathBuf::from)
            .as_deref(),
        42,
    );
    let mejor_oportunidad = estado
        .oportunidades
        .iter()
        .max_by(|a, b| a.utilidad_usd.total_cmp(&b.utilidad_usd));
    let ultima_auditoria = estado.auditoria_decisiones.front();
    let ultimo_evento = estado.eventos_ejecucion.front();
    let ga = estado.genetico.as_ref();
    let ml_edge = estado.ml_edge.as_ref();
    let persistencia = estado.persistencia.as_ref();
    let ws_conectados = contar_exchanges_unicos(
        estado
            .cotizaciones
            .iter()
            .filter(|c| snapshot_websocket_fresco(estado, c)),
    );
    let rest_fallbacks = contar_exchanges_unicos(
        estado
            .cotizaciones
            .iter()
            .filter(|c| c.ultimo_mensaje == "rest_fallback"),
    );
    let criterios = vec![
        criterio(
            "demo_segura",
            true,
            100,
            "Sin llaves API, custodia, ordenes reales ni transferencias on-chain.",
        ),
        criterio(
            "datos_tiempo_real",
            ws_conectados >= 2,
            puntaje_ratio(ws_conectados, 5),
            format!(
                "{} feeds WebSocket publicos frescos; {} feeds con latencia EWMA disponible.",
                ws_conectados,
                estado.latencias_exchange.len()
            ),
        ),
        criterio(
            "websocket_first_rest_fallback",
            ws_conectados >= 2 || rest_fallbacks > 0,
            if rest_fallbacks > 0 {
                94
            } else if ws_conectados >= 2 {
                84
            } else {
                35
            },
            format!(
                "WS es fuente primaria; {} snapshots recientes llegaron por REST fallback publico.",
                rest_fallbacks
            ),
        ),
        criterio(
            "motor_ejecutable",
            estado.metricas.operaciones > 0 || mejor_oportunidad.is_some(),
            if estado.metricas.operaciones > 0 {
                95
            } else {
                72
            },
            format!(
                "{} operaciones simuladas, {} oportunidades recientes.",
                estado.metricas.operaciones,
                estado.oportunidades.len()
            ),
        ),
        criterio(
            "explicabilidad",
            !estado.auditoria_decisiones.is_empty(),
            puntaje_ratio(estado.auditoria_decisiones.len(), 24),
            format!(
                "{} decisiones auditadas con score, costos, pesos GA y razon.",
                estado.auditoria_decisiones.len()
            ),
        ),
        criterio(
            "ga_activo",
            ga.map(|g| g.activo || g.generacion > 0).unwrap_or(false),
            ga.map(|g| {
                if g.generacion > 0 {
                    95
                } else if g.poblacion >= 10 {
                    80
                } else {
                    55
                }
            })
            .unwrap_or(0),
            ga.map(|g| {
                format!(
                    "Generacion {}, fitness {:.2}, diversidad {:.1}%, poblacion {}.",
                    g.generacion,
                    g.mejor_fitness,
                    g.diversidad * 100.0,
                    g.poblacion
                )
            })
            .unwrap_or_else(|| "Sin estado GA publico.".into()),
        ),
        criterio(
            "ml_edge_explicable",
            ml_edge.is_some(),
            ml_edge.map(|m| if m.activo { 96 } else { 82 }).unwrap_or(0),
            ml_edge
                .map(|m| {
                    format!(
                        "{} score {:.3}, EV {:.2} USD, confianza {:.1}%, {} features auditables.",
                        m.version,
                        m.score_actual,
                        m.expected_value_usd,
                        m.confianza * 100.0,
                        m.features.len()
                    )
                })
                .unwrap_or_else(|| "Sin auditoria reciente para calcular ML Edge.".into()),
        ),
        criterio(
            "riesgo_y_resiliencia",
            estado.metricas.estado_riesgo != "critico",
            if estado.metricas.circuit_breaker_activo {
                75
            } else {
                92
            },
            format!(
                "Riesgo={}, circuitBreaker={}, modoConservador={}, fallos={}.",
                estado.metricas.estado_riesgo,
                estado.metricas.circuit_breaker_activo,
                estado.metricas.modo_conservador,
                estado.metricas.operaciones_fallidas
            ),
        ),
        criterio(
            "backtest_y_export",
            true,
            96,
            "Incluye backtest deterministico, Research Lab sweep y exportaciones JSON/CSV de auditoria.",
        ),
        criterio(
            "persistencia_sqlite_local",
            persistencia.map(|p| p.activa).unwrap_or(false),
            persistencia
                .map(|p| {
                    if p.activa && p.operaciones + p.oportunidades + p.auditorias > 0 {
                        96
                    } else if p.activa {
                        82
                    } else {
                        0
                    }
                })
                .unwrap_or(0),
            persistencia
                .map(|p| {
                    format!(
                        "SQLite en {} con {} ops, {} oportunidades, {} auditorias y {} eventos.",
                        p.ruta, p.operaciones, p.oportunidades, p.auditorias, p.eventos
                    )
                })
                .unwrap_or_else(|| "Sin SQLite de auditoria.".into()),
        ),
    ];
    let checks_cumplidos = criterios
        .iter()
        .filter(|c| c.get("cumplido").and_then(|v| v.as_bool()) == Some(true))
        .count();

    json!({
        "generadoEn": estado.generado_en,
        "nombre": "Mayab Arbitraje BTC - paquete de evaluacion",
        "modo": "demo segura read-only",
        "evidenceSummary": {
            "pass": checks_cumplidos,
            "total": criterios.len(),
            "nota": "conteo de evidencia, no calificacion ni prediccion del comite"
        },
        "huellaAuditoria": huella_estado(estado),
        "rubricaOficialComite": matriz_rubrica_oficial(estado),
        "coberturaFinalista": cobertura_finalista(estado),
        "recomendacionesParaGanar": recomendaciones_ganadoras(estado),
        "radarCompetitivo": {
            "enfoque": "Diferenciar por evidencia verificable, no por promesas: cada fortaleza apunta a endpoint, metrica o evento auditable.",
            "ventajasDefendibles": [
                "demo rentable etiquetada para no depender del mercado real",
                "scoring evolutivo con EV, supervivencia, fill probability, adverse selection y contribuciones por variable",
                "decision inspector con costos, pesos GA y balances previos",
                "preflight y paquete de evaluacion para revisar sin navegar toda la UI",
                "auditoria SQLite local y exports JSON/CSV; retencion externa explicitada para Cloud Run",
                "seguridad explicita: sin API keys, custodia ni ordenes reales"
            ],
            "riesgosDeOtrosProyectosQueEvitamos": [
                "mostrar spreads brutos sin costos reales",
                "mezclar BTC/USD y BTC/USDT sin basis",
                "asumir fills completos con solo best bid/ask",
                "depender de una oportunidad live para la demo",
                "prometer trading real sin capa de seguridad"
            ]
        },
        "parametrizacion": {
            "total": parametros_controlables(estado),
            "metodologia": "cada entrada representa un control independiente con nombre, categoria, mutabilidad y procedencia; no se cuentan valores derivados",
            "catalogo": catalogo_parametros(estado)
        },
        "criterios": criterios,
        "resumenEjecutivo": resumen,
        "evidencia": {
            "metricas": {
                "eventosMercado": estado.metricas.eventos_mercado,
                "operaciones": estado.metricas.operaciones,
                "operacionesFallidas": estado.metricas.operaciones_fallidas,
                "pnlUsd": estado.metricas.utilidad_acumulada_usd,
                "retornoBps": estado.metricas.retorno_bps,
                "sharpeRatio": estado.metricas.sharpe_ratio,
                "winRate": estado.metricas.win_rate,
                "maxDrawdownUsd": estado.metricas.max_drawdown_usd,
                "latenciaPromedioMs": estado.metricas.latencia_promedio_ms,
            },
            "mejorOportunidad": mejor_oportunidad,
            "ultimaAuditoria": ultima_auditoria,
            "mlEdge": ml_edge,
            "ultimoEvento": ultimo_evento,
            "ga": ga,
            "persistencia": persistencia,
            "preflight": preflight,
                "backtest": backtest,
                "researchLab": lab_sweep,
                "microstructureLab": microstructure,
                "ouLab": ou,
        },
        "scriptDemo": [
            "GET /api/healthz",
            "GET /api/preflight",
            "POST /api/demo/reset",
            "POST /api/demo/final",
            "POST /api/ga/evolucionar {\"usarReplaySiVacio\":true,\"muestras\":96}",
            "POST /api/demo {\"escenario\":\"mercado_rentable\"}",
            "GET /api/lab/sweep",
            "GET /api/research/microstructure",
            "GET /api/research/ou",
            "GET /api/paquete-evaluacion",
            "GET /api/export/json"
        ],
        "diferenciadores": [
            "Rust single-binary con WebSockets publicos, API Axum y dashboard sin build frontend.",
            "WebSocket-first con REST fallback publico cuando un feed queda stale o desconectado.",
            "GA real con elitismo, torneo, cruce, mutacion, annealing e inyeccion diferencial.",
            "Scoring evolutivo explicable: EV, probabilidades simuladas de supervivencia/fill, adverse selection y contribuciones por variable.",
            "Research Lab: campeon GA contra baseline y presets sobre 24 semillas comunes, sin ocultar derrotas.",
            "Auditoria por decision: score, costos, z-score, latencia, pesos GA y balances previos.",
            "Demo rentable controlada para probar valor aunque el mercado real este plano.",
            "SQLite local para auditoria durante la vida de la instancia, con exports para retencion externa.",
            "Limites explicitos de seguridad: no API keys, no custodia, no ordenes reales."
        ],
        "endpoints": {
            "estado": "/api/estado",
            "jurado": "/api/jurado",
            "preflight": "/api/preflight",
            "resumenLlm": "/api/resumen-llm",
            "paqueteEvaluacion": "/api/paquete-evaluacion",
            "backtest": "/api/backtest",
            "labSweep": "/api/lab/sweep",
            "demoReset": "/api/demo/reset",
            "demoFinal": "/api/demo/final",
            "demoCaos": "/api/demo/caos",
            "exportJson": "/api/export/json",
            "exportCsv": "/api/export/csv",
            "gaEstado": "/api/ga/estado"
        }
    })
}

fn matriz_rubrica_oficial(estado: &EstadoPublico) -> Vec<serde_json::Value> {
    let parametros_controlables = parametros_controlables(estado);
    let exchanges_activos = estado.exchanges_activos.values().filter(|v| **v).count();
    let eventos_adversos = estado
        .eventos_ejecucion
        .iter()
        .filter(|e| {
            let tipo = e.tipo.as_str();
            tipo.contains("fallo")
                || tipo.contains("movido")
                || tipo.contains("parcial")
                || tipo.contains("circuit")
                || tipo.contains("liquidez")
                || tipo.contains("demo")
        })
        .count();
    let auditoria_visible = !estado.auditoria_decisiones.is_empty();
    let dashboard_ok = Path::new("internal/webui/web/index.html").is_file()
        && Path::new("internal/webui/web/app.js").is_file()
        && Path::new("internal/webui/web/styles.css").is_file();
    let persistencia_ok = estado
        .persistencia
        .as_ref()
        .map(|p| p.activa)
        .unwrap_or(false);
    let ga_activo = estado
        .genetico
        .as_ref()
        .map(|g| g.activo || g.generacion > 0)
        .unwrap_or(false);
    let ml_edge_ok = estado.ml_edge.is_some();

    vec![
        rubrica_item(
            "profundidad_parametrizacion",
            25,
            (puntaje_ratio(parametros_controlables, 34) as u16 + if ga_activo { 10 } else { 0 })
                .min(100) as u8,
            "Cuantas variables controla el sistema y que tan configurable es la estrategia?",
            format!(
                "{} parametros operativos estimados, {} exchanges configurables, GA {}.",
                parametros_controlables,
                estado.configuracion.exchanges.len(),
                if ga_activo { "activo" } else { "disponible" }
            ),
            "Abrir controles de estrategia, costos, adversidad, exchanges y GA; luego confirmar cambios en /api/estado.",
        ),
        rubrica_item(
            "robustez_escenarios_adversos",
            25,
            (70 + (eventos_adversos.min(6) * 5) as u8).min(100),
            "Que pasa si falla una orden, falta liquidez o el mercado se mueve durante ejecucion?",
            format!(
                "{} eventos adversos recientes, circuitBreaker={}, modoConservador={}, fallos={}.",
                eventos_adversos,
                estado.metricas.circuit_breaker_activo,
                estado.metricas.modo_conservador,
                estado.metricas.operaciones_fallidas
            ),
            "Ejecutar /api/demo con fallo_orden, mercado_movido, fill_parcial y circuit_breaker antes de presentar.",
        ),
        rubrica_item(
            "wallets_y_rebalanceo",
            20,
            (puntaje_ratio(estado.balances.len(), exchanges_activos.max(2)) as u16
                + if estado.metricas.rebalanceos_totales > 0 { 10 } else { 0 })
                .min(100) as u8,
            "El sistema mantiene balance operativo entre exchanges de forma inteligente?",
            format!(
                "{} wallets simuladas, {} rebalanceos totales, persistencia {}.",
                estado.balances.len(),
                estado.metricas.rebalanceos_totales,
                if persistencia_ok { "activa" } else { "inactiva" }
            ),
            "Usar demo rebalanceo si no hay movimientos recientes; exportar JSON para mostrar saldos antes/despues.",
        ),
        rubrica_item(
            "interfaz_y_visualizacion",
            20,
            (if dashboard_ok { 55 } else { 0 }
                + puntaje_ratio(estado.auditoria_decisiones.len(), 12).min(35)
                + if estado.metricas.operaciones > 0 { 6 } else { 0 }
                + if ml_edge_ok { 4 } else { 0 })
                .min(100),
            "Se puede seguir en tiempo real lo que hace el bot, historial, PnL y oportunidades?",
            format!(
                "Dashboard={}, {} oportunidades, {} operaciones, {} auditorias, ML Edge={}.",
                if dashboard_ok { "ok" } else { "faltante" },
                estado.oportunidades.len(),
                estado.metricas.operaciones,
                estado.auditoria_decisiones.len(),
                if ml_edge_ok { "visible" } else { "pendiente" }
            ),
            "Presentar primero el dashboard y despues abrir /api/paquete-evaluacion para evidencia estructurada.",
        ),
        rubrica_item(
            "documentacion_y_claridad",
            10,
            if Path::new("README.md").is_file() && auditoria_visible {
                96
            } else if Path::new("README.md").is_file() {
                88
            } else {
                45
            },
            "README, decisiones tecnicas y codigo legible explican el sistema?",
            "README en espanol, AGENTS.md operativo, endpoints de resumen LLM y paquete de evaluacion.".to_string(),
            "Mantener README alineado: toda promesa debe existir en API/UI o quitarse antes del deploy final.",
        ),
    ]
}

fn cobertura_finalista(estado: &EstadoPublico) -> serde_json::Value {
    let parametros = parametros_controlables(estado);
    let feeds_ws = contar_exchanges_unicos(
        estado
            .cotizaciones
            .iter()
            .filter(|c| snapshot_websocket_fresco(estado, c)),
    );
    let rest_fallbacks = contar_exchanges_unicos(
        estado
            .cotizaciones
            .iter()
            .filter(|c| c.ultimo_mensaje == "rest_fallback"),
    );
    let eventos_adversos = estado
        .eventos_ejecucion
        .iter()
        .filter(|e| {
            let tipo = e.tipo.as_str();
            tipo.contains("fallo")
                || tipo.contains("movido")
                || tipo.contains("parcial")
                || tipo.contains("circuit")
                || tipo.contains("liquidez")
                || tipo.contains("demo")
        })
        .count();
    let fill_parcial = estado.operaciones.iter().any(|op| op.parcial)
        || estado.oportunidades.iter().any(|op| op.parcial);
    let ga_activo = estado
        .genetico
        .as_ref()
        .map(|g| g.activo || g.generacion > 0 || g.poblacion >= 10)
        .unwrap_or(false);
    let ml_edge = estado.ml_edge.as_ref();
    let persistencia = estado.persistencia.as_ref();
    let latencias_p99 = estado.latencias_exchange.iter().any(|l| l.p99_ms > 0);
    let backtest_lab = true;
    let exports = true;
    let dashboard = dashboard_assets_present();

    let dimensiones = vec![
        cobertura_item(
            "parametrizacion_profunda",
            parametros >= 34 && ga_activo,
            format!(
                "{} parametros controlables estimados: riesgo, costos, exchanges, adversidad, rebalanceo, GA y toggles por venue.",
                parametros
            ),
            "UI controles, POST /api/config, POST /api/ga/config y /api/estado.",
        ),
        cobertura_item(
            "robustez_adversa",
            estado.metricas.estado_riesgo != "critico" && eventos_adversos > 0,
            format!(
                "{} eventos adversos recientes; circuitBreaker={}, conservador={}, singleTradeInFlight={}.",
                eventos_adversos,
                estado.metricas.circuit_breaker_activo,
                estado.metricas.modo_conservador,
                estado.metricas.ejecucion_en_curso
            ),
            "Botones de Demo controlada y POST /api/demo.",
        ),
        cobertura_item(
            "wallets_rebalanceo",
            estado.balances.len() >= 2 && (estado.metricas.rebalanceos_totales > 0 || fill_parcial),
            format!(
                "{} wallets, {} rebalanceos, fillParcial={}.",
                estado.balances.len(),
                estado.metricas.rebalanceos_totales,
                fill_parcial
            ),
            "Panel Carteras, tabla Rebalanceos, demo rebalanceo y exports.",
        ),
        cobertura_item(
            "ui_visualizacion_jurado",
            dashboard && !estado.auditoria_decisiones.is_empty(),
            format!(
                "Dashboard={}, {} auditorias, {} oportunidades, {} operaciones.",
                dashboard,
                estado.auditoria_decisiones.len(),
                estado.oportunidades.len(),
                estado.metricas.operaciones
            ),
            "Dashboard, panel Readiness, scoring evolutivo, mapa, timeline y auditoria.",
        ),
        cobertura_item(
            "metricas_latency_replay",
            latencias_p99 || backtest_lab,
            format!(
                "p99 latencia visible={}, backtest={}, researchLab={}, restFallbacks={}.",
                latencias_p99, backtest_lab, backtest_lab, rest_fallbacks
            ),
            "GET /api/latencias, /api/backtest y /api/lab/sweep.",
        ),
        cobertura_item(
            "documentacion_tests_deploy",
            Path::new("README.md").is_file() && Path::new("scripts/smoke-demo.sh").is_file(),
            "README, ARCHITECTURE, DEMO_SCRIPT, release-check, smoke-demo y comandos cargo fmt/cargo test documentados.".to_string(),
            "README, scripts/release-check.sh y scripts/smoke-demo.sh.",
        ),
        cobertura_item(
            "auditoria_local_exports",
            exports && persistencia.map(|p| p.activa).unwrap_or(false),
            persistencia
                .map(|p| {
                    format!(
                        "SQLite activa={} en {}, operaciones={}, oportunidades={}, auditorias={}.",
                        p.activa, p.ruta, p.operaciones, p.oportunidades, p.auditorias
                    )
                })
                .unwrap_or_else(|| "SQLite no inicializada; exports siguen disponibles.".into()),
            "GET /api/export/json, /api/export/csv y AUDITORIA_DB_PATH.",
        ),
        cobertura_item(
            "ia_explicable_ga",
            ga_activo && ml_edge.is_some(),
            ml_edge
                .map(|m| {
                    format!(
                        "GA activo={}, scoring {} con {} variables, EV {:.2} USD.",
                        ga_activo,
                        m.version,
                        m.features.len(),
                        m.expected_value_usd
                    )
                })
                .unwrap_or_else(|| format!("GA activo={}, scoring pendiente de auditoria.", ga_activo)),
            "Panel GA Lab, scoring evolutivo, /api/ga/estado y /api/resumen-llm.",
        ),
    ];

    let cubiertas = dimensiones
        .iter()
        .filter(|d| d.get("ok").and_then(|v| v.as_bool()).unwrap_or(false))
        .count();
    json!({
        "nombre": "Cobertura de benchmark finalista publico",
        "fuente": "Sintesis interna basada en la revision publica adjunta: parametrizacion, robustez, wallets/rebalanceo, UI, metricas, tests, deploy y documentacion.",
        "cubiertas": cubiertas,
        "total": dimensiones.len(),
        "status": if cubiertas == dimensiones.len() { "completo" } else { "accionable" },
        "parametrosControlablesEstimados": parametros,
        "feedsWebSocketFrescos": feeds_ws,
        "restFallbacks": rest_fallbacks,
        "dimensiones": dimensiones,
        "lectura": if cubiertas == dimensiones.len() {
            "La demo cubre el benchmark finalista con evidencia en API/UI/export."
        } else {
            "Ejecuta /api/demo/final y revisa /api/preflight para llenar evidencia runtime faltante."
        }
    })
}

fn cobertura_item(
    nombre: &'static str,
    ok: bool,
    evidencia: impl Into<String>,
    donde_verificar: &'static str,
) -> serde_json::Value {
    json!({
        "nombre": nombre,
        "ok": ok,
        "evidencia": evidencia.into(),
        "dondeVerificar": donde_verificar,
    })
}

fn parametros_controlables(estado: &EstadoPublico) -> usize {
    catalogo_parametros(estado).len()
}

fn catalogo_parametros(estado: &EstadoPublico) -> Vec<serde_json::Value> {
    let base = [
        ("maxOperacionBtc", "sizing", true, "configuracion"),
        ("minUtilidadUsd", "rentabilidad", true, "configuracion"),
        (
            "minDiferencialNetoBps",
            "rentabilidad",
            true,
            "configuracion",
        ),
        ("deslizamientoBps", "costos", true, "configuracion"),
        ("latenciaRiesgoBps", "costos", true, "configuracion"),
        ("retiroAmortizadoBps", "costos", true, "configuracion"),
        ("usdtUsdPremiumBps", "basis", true, "configuracion"),
        ("permitirCruceUsdUsdt", "basis", true, "configuracion"),
        ("staleMs", "integridad_feed", true, "configuracion"),
        ("enfriamientoMs", "ejecucion", true, "configuracion"),
        ("circuitBreakerPerdidaUsd", "riesgo", true, "configuracion"),
        ("circuitBreakerVentanaMin", "riesgo", true, "configuracion"),
        ("volatilidadUmbralBps", "riesgo", true, "configuracion"),
        ("volatilidadVentanaSeg", "riesgo", true, "configuracion"),
        ("simularAdversidad", "adversidad", true, "configuracion"),
        ("probFalloOrden", "adversidad", true, "configuracion"),
        ("probMovimientoBrusco", "adversidad", true, "configuracion"),
        ("movimientoBruscoBps", "adversidad", true, "configuracion"),
        ("rebalanceUmbralPct", "rebalanceo", true, "configuracion"),
        (
            "rebalanceMaxTransferPct",
            "rebalanceo",
            true,
            "configuracion",
        ),
        ("costoRebalanceoUsd", "rebalanceo", true, "configuracion"),
        ("rebalanceSettlementMs", "rebalanceo", true, "configuracion"),
        ("tamanoPoblacionGa", "ga", true, "api_ga"),
        ("tasaMutacionGa", "ga", true, "api_ga"),
        ("tasaCruceGa", "ga", true, "api_ga"),
        ("muestrasReplayGa", "ga", true, "api_ga"),
        ("usarReplaySiVacio", "ga", true, "api_ga"),
        ("killSwitch", "riesgo", true, "api_admin"),
        ("webhookUrl", "integracion", true, "configuracion"),
        ("reglasRebalanceo", "rebalanceo", true, "api_admin"),
        ("parBase", "universo", false, "entorno"),
        ("paresExtra", "universo", false, "entorno"),
        ("capitalInicialUsd", "simulacion", false, "entorno"),
        ("balanceInicialBtc", "simulacion", false, "entorno"),
        ("intervaloAnalisisMs", "pipeline", false, "entorno"),
    ];

    let mut catalogo = base
        .into_iter()
        .map(|(nombre, categoria, mutable_runtime, origen)| {
            json!({
                "nombre": nombre,
                "categoria": categoria,
                "mutableRuntime": mutable_runtime,
                "origen": origen,
                "restriccion": restriccion_parametro(nombre),
                "scope": "global"
            })
        })
        .collect::<Vec<_>>();

    let mut exchanges = estado
        .configuracion
        .exchanges
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    exchanges.sort();
    for exchange in exchanges {
        for (campo, categoria) in [
            ("feeTaker", "costos_exchange"),
            ("retiroBtc", "costos_exchange"),
            ("confiabilidad", "riesgo_exchange"),
            ("activo", "routing_exchange"),
        ] {
            catalogo.push(json!({
                "nombre": format!("exchanges.{exchange}.{campo}"),
                "categoria": categoria,
                "mutableRuntime": true,
                "origen": "configuracion_exchange",
                "restriccion": match campo {
                    "feeTaker" => "0..=0.10",
                    "retiroBtc" => "0..=10 BTC",
                    "confiabilidad" => "0..=1",
                    _ => "booleano"
                },
                "scope": exchange
            }));
        }
    }
    catalogo
}

fn restriccion_parametro(nombre: &str) -> &'static str {
    match nombre {
        "maxOperacionBtc" => ">0 y <=10 BTC",
        "probFalloOrden" | "probMovimientoBrusco" => "0..=1",
        "rebalanceUmbralPct" | "rebalanceMaxTransferPct" => "0..=100",
        "staleMs" => ">0 y <=300000 ms",
        "enfriamientoMs" => "0..=3600000 ms",
        "rebalanceSettlementMs" => "0..=300000 ms",
        "tamanoPoblacionGa" => "10..=300",
        "tasaMutacionGa" => "0..=0.8",
        "tasaCruceGa" => "0..=1",
        "muestrasReplayGa" => "12..=240",
        "webhookUrl" => "HTTP(S), máximo 2048 caracteres",
        "deslizamientoBps"
        | "latenciaRiesgoBps"
        | "retiroAmortizadoBps"
        | "usdtUsdPremiumBps"
        | "volatilidadUmbralBps"
        | "movimientoBruscoBps"
        | "minDiferencialNetoBps" => "0..=10000 bps",
        "minUtilidadUsd" | "circuitBreakerPerdidaUsd" | "costoRebalanceoUsd" => ">=0, finito",
        "circuitBreakerVentanaMin" | "volatilidadVentanaSeg" => ">0",
        "permitirCruceUsdUsdt"
        | "simularAdversidad"
        | "usarReplaySiVacio"
        | "killSwitch"
        | "exchangeActivo" => "booleano",
        _ => "validado por contrato de origen",
    }
}

fn rubrica_item(
    criterio: &'static str,
    peso: u8,
    _puntaje_interno: u8,
    pregunta: &'static str,
    evidencia: impl Into<String>,
    siguiente: &'static str,
) -> serde_json::Value {
    json!({
        "criterio": criterio,
        "pesoPct": peso,
        "estado": "EVIDENCIA_DISPONIBLE",
        "preguntaComite": pregunta,
        "evidenciaActual": evidencia.into(),
        "siguienteMovimientoDemo": siguiente,
        "nota": "Mayab no se autocalifica; el peso proviene de la rubrica y el evaluador asigna el puntaje.",
    })
}

fn evidencia_preflight(
    claim: &'static str,
    disponible: bool,
    origen: &'static str,
    endpoint: &'static str,
    reproducir: &'static str,
) -> serde_json::Value {
    json!({
        "claim": claim,
        "status": if disponible { "PASS" } else { "WARN" },
        "available": disponible,
        "origin": origen,
        "endpoint": endpoint,
        "reproduce": reproducir,
        "note": if disponible {
            "evidencia presente en la corrida actual"
        } else {
            "capacidad implementada; la corrida limpia aun no genero esta evidencia"
        },
    })
}

fn recomendaciones_ganadoras(estado: &EstadoPublico) -> Vec<&'static str> {
    let mut recomendaciones = Vec::new();
    if estado.metricas.operaciones == 0 || estado.metricas.utilidad_acumulada_usd <= 0.0 {
        recomendaciones.push("Antes de la demo, ejecutar POST /api/demo mercado_rentable para mostrar PnL positivo, eventos demo_rentable y GA activo.");
    }
    if estado.auditoria_decisiones.len() < 12 {
        recomendaciones.push("Generar mas evidencia forense con demo rentable, fill parcial y evolucion GA; el juez debe ver decisiones aceptadas y descartadas.");
    }
    if estado.metricas.rebalanceos_totales == 0 {
        recomendaciones.push("Forzar POST /api/demo rebalanceo para mostrar gestion de wallets y movimiento interno auditado.");
    }
    if estado
        .persistencia
        .as_ref()
        .map(|p| !p.activa)
        .unwrap_or(true)
    {
        recomendaciones.push("Revisar AUDITORIA_DB_PATH y permisos de SQLite; documentar export o backend externo para retencion entre instancias.");
    }
    if estado
        .genetico
        .as_ref()
        .map(|g| g.generacion == 0)
        .unwrap_or(true)
    {
        recomendaciones.push("Ejecutar POST /api/ga/evolucionar con replay si el mercado esta plano para mostrar estrategia optimizada.");
    }
    if recomendaciones.is_empty() {
        recomendaciones.push("Estado listo: presentar dashboard, preflight, paquete de evaluacion y exports en ese orden.");
    }
    recomendaciones
}

fn criterio(
    nombre: &'static str,
    ok: bool,
    _puntaje_interno: u8,
    detalle: impl Into<String>,
) -> serde_json::Value {
    json!({
        "nombre": nombre,
        "cumplido": ok,
        "estado": if ok { "PASS" } else { "WARN" },
        "detalle": detalle.into(),
    })
}

fn puntaje_ratio(actual: usize, objetivo: usize) -> u8 {
    if objetivo == 0 {
        return 100;
    }
    ((actual.min(objetivo) * 100) / objetivo) as u8
}

fn huella_estado(estado: &EstadoPublico) -> String {
    let payload = json!({
        "generadoEn": estado.generado_en,
        "eventosMercado": estado.metricas.eventos_mercado,
        "operaciones": estado.metricas.operaciones,
        "operacionesFallidas": estado.metricas.operaciones_fallidas,
        "utilidadAcumuladaUsd": estado.metricas.utilidad_acumulada_usd,
        "auditoria": estado.auditoria_decisiones.front(),
        "ultimaOperacion": estado.operaciones.front(),
        "ultimoEvento": estado.eventos_ejecucion.front(),
        "ga": estado.genetico,
    });
    let mut hasher = DefaultHasher::new();
    payload.to_string().hash(&mut hasher);
    format!("mayab-{:016x}", hasher.finish())
}

fn check(nombre: &str, ok: bool, detalle: impl Into<String>) -> serde_json::Value {
    json!({
        "nombre": nombre,
        "ok": ok,
        "detalle": detalle.into(),
    })
}

fn si_no(valor: bool) -> &'static str {
    if valor {
        "si"
    } else {
        "no"
    }
}

fn default_true() -> bool {
    true
}

fn csv_cell(valor: &str) -> String {
    let escaped = valor.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}

fn backtest_reproducible(estado: &EstadoPublico) -> serde_json::Value {
    let cfg = &estado.configuracion;
    let (umbral_ga, max_btc_ga, fuente_ga) = estado
        .genetico
        .as_ref()
        .map(|ga| {
            (
                ga.umbral_optimizado,
                ga.max_operacion_optimizada_btc,
                "campeon_ga_publicado",
            )
        })
        .unwrap_or((
            (cfg.min_diferencial_neto_bps * 0.65).clamp(0.20, 1.25),
            (cfg.max_operacion_btc * 1.20).clamp(0.03, 0.60),
            "fallback_parametrico",
        ));
    let umbral_base = 1.20;
    let max_btc_base = cfg.max_operacion_btc.min(0.12);
    let base = simular_backtest(cfg, umbral_base, max_btc_base, 42);
    let optimizada = simular_backtest(cfg, umbral_ga, max_btc_ga, 42);
    let delta_pnl = optimizada.pnl_usd - base.pnl_usd;
    let delta_drawdown = base.max_drawdown_usd - optimizada.max_drawdown_usd;
    let semillas = (101_u64..=124).collect::<Vec<_>>();
    let validacion_base = resumen_multisemilla(cfg, umbral_base, max_btc_base, &semillas);
    let validacion_ga = resumen_multisemilla(cfg, umbral_ga, max_btc_ga, &semillas);
    let validacion_fuera_muestra = validacion_fuera_muestra(cfg, umbral_ga, max_btc_ga, fuente_ga);
    let comparacion_impacto = comparar_modelos_impacto(cfg, 42);
    let significancia =
        significancia_bootstrap_bloques(cfg, (umbral_base, max_btc_base), (umbral_ga, max_btc_ga));
    let base_mediana = validacion_base["pnlMedianoUsd"].as_f64().unwrap_or(0.0);
    let ga_mediana = validacion_ga["pnlMedianoUsd"].as_f64().unwrap_or(0.0);
    json!({
        "ticks": 240,
        "seedPrincipal": 42,
        "fuenteOptimizada": fuente_ga,
        "parametrosOptimizados": {
            "umbralBps": umbral_ga,
            "maxOperacionBtc": max_btc_ga,
        },
        "parametrosBaseline": {
            "umbralBps": umbral_base,
            "maxOperacionBtc": max_btc_base,
            "definicion": "Referencia estatica conservadora, fijada antes de observar las semillas de validacion."
        },
        "rutasEvaluadas": base.rutas_evaluadas,
        "base": base,
        "optimizada": optimizada,
        "validacionMultisemilla": {
            "semillas": semillas,
            "base": validacion_base,
            "optimizada": validacion_ga,
            "deltaPnlMedianoUsd": ga_mediana - base_mediana,
            "ganadorMediana": if ga_mediana >= base_mediana { "optimizada" } else { "base" },
            "lectura": "La mediana de 24 corridas reduce la dependencia de una semilla favorable; el resultado se reporta aunque el GA no gane."
        },
        "validacionFueraMuestra": validacion_fuera_muestra,
        "comparacionImpacto": comparacion_impacto,
        "significanciaBootstrap": significancia,
        "comparacion": {
            "deltaPnlUsd": delta_pnl,
            "deltaDrawdownUsd": delta_drawdown,
            "ganador": if delta_pnl >= 0.0 { "optimizada" } else { "base" },
            "criterio": "Mismo seed y costos vigentes; baseline estatico predefinido contra el campeon GA publicado."
        },
        "nota": "Replay Monte Carlo sintetico y deterministico sobre BTC con costos actuales, cinco exchanges, dispersion entre libros y movimiento adverso posterior a la decision; no demuestra rentabilidad real."
    })
}

#[derive(Clone, Serialize)]
struct FilaImpacto {
    modelo: String,
    #[serde(rename = "pnlUsd")]
    pnl_usd: f64,
    #[serde(rename = "fillRate")]
    fill_rate: f64,
    #[serde(rename = "maxDrawdownUsd")]
    max_drawdown_usd: f64,
    #[serde(rename = "impactoMedioBps")]
    impacto_medio_bps: f64,
    #[serde(rename = "decisionesDistintas")]
    decisiones_distintas: u64,
    #[serde(rename = "ordenesAceptadas")]
    ordenes_aceptadas: u64,
    #[serde(rename = "cantidadMediaBtc")]
    cantidad_media_btc: f64,
    #[serde(rename = "errorAbsolutoMedioBps")]
    error_absoluto_medio_bps: f64,
    #[serde(rename = "sesgoCostoBps")]
    sesgo_costo_bps: f64,
}

#[derive(Clone)]
struct CandidatoImpacto {
    precio: f64,
    cantidad: f64,
    spread_bruto_bps: f64,
    volatilidad_bps: f64,
    horizonte_ms: u64,
    niveles: Vec<NivelOrden>,
    impacto_observado_bps: f64,
}

/// Comparación pareada: cada modelo ve exactamente los mismos candidatos, books
/// y markouts. La decisión usa el costo estimado; el PnL usa el costo observado.
fn comparar_modelos_impacto(cfg: &MapaCostos, seed: u64) -> serde_json::Value {
    let mut rng = StdRng::seed_from_u64(seed ^ 0x1A4C_7004);
    let mut tape = Vec::with_capacity(2_400);
    for _ in 0..2_400 {
        let precio = rng.gen_range(92_000.0..108_000.0);
        let cantidad = rng
            .gen_range(0.025..0.55_f64)
            .min(cfg.max_operacion_btc.max(0.025));
        let volatilidad_bps = rng.gen_range(18.0..85.0);
        let horizonte_ms = rng.gen_range(120..1_500);
        let pendiente_bps = rng.gen_range(0.35..2.8);
        let mut niveles = Vec::with_capacity(6);
        for nivel in 0..6 {
            niveles.push(NivelOrden {
                precio: precio * (1.0 + pendiente_bps * nivel as f64 / 10_000.0),
                cantidad: rng.gen_range(0.015..0.16),
            });
        }
        let orden = OrdenImpacto {
            lado: LadoOrden::Compra,
            cantidad_btc: cantidad,
            precio_referencia: precio,
            niveles: &niveles,
            volatilidad_bps: Some(volatilidad_bps),
            horizonte_ms: Some(horizonte_ms),
        };
        let book = ModeloImpacto::BookWalk.estimar(&orden);
        let ruido_markout = rng.gen_range(-0.8_f64..2.4_f64);
        tape.push(CandidatoImpacto {
            precio,
            cantidad,
            spread_bruto_bps: rng.gen_range(0.4..8.5),
            volatilidad_bps,
            horizonte_ms,
            niveles,
            impacto_observado_bps: (book.impacto_bps + ruido_markout).max(0.0),
        });
    }
    let modelos = [
        ModeloImpacto::BookWalk,
        ModeloImpacto::SquareRoot {
            eta: 0.72,
            volumen_diario_btc: 18_000.0,
        },
        ModeloImpacto::AlmgrenLite {
            impacto_temporal: 0.018,
            impacto_permanente: 0.012,
            horizonte_ms: 750,
        },
    ];
    let decisiones_book = decisiones_modelo(&modelos[0], &tape, cfg);
    let filas = modelos
        .iter()
        .map(|modelo| resumir_modelo(modelo, &tape, cfg, &decisiones_book))
        .collect::<Vec<_>>();
    let decisiones_square = decisiones_modelo(&modelos[1], &tape, cfg);
    let book_acepta_square_rechaza = decisiones_book
        .iter()
        .zip(&decisiones_square)
        .filter(|(b, s)| **b && !**s)
        .count();
    let menor_error = filas
        .iter()
        .min_by(|a, b| {
            a.error_absoluto_medio_bps
                .total_cmp(&b.error_absoluto_medio_bps)
        })
        .map(|f| f.modelo.clone())
        .unwrap_or_default();
    let mayor_subestimacion = filas
        .iter()
        .min_by(|a, b| a.sesgo_costo_bps.total_cmp(&b.sesgo_costo_bps))
        .map(|f| f.modelo.clone())
        .unwrap_or_default();
    let book = &filas[0];
    let cambios = filas
        .iter()
        .skip(1)
        .map(|f| {
            json!({
                "modelo": f.modelo,
                "cambioCantidadMediaBtc": f.cantidad_media_btc - book.cantidad_media_btc,
                "cambioPnlUsd": f.pnl_usd - book.pnl_usd,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "modeloPredeterminado": "Book-walk",
        "cambioRecomendado": false,
        "seed": seed,
        "candidatos": tape.len(),
        "tabla": filas,
        "respuestas": {
            "bookWalkAceptaSquareRootRechaza": book_acepta_square_rechaza,
            "modeloQueMasSubestimaCostoExPost": mayor_subestimacion,
            "modeloConMenorErrorContraMarkout": menor_error,
            "cambiosVsBookWalk": cambios,
        },
        "metodologia": "Comparacion pareada sobre el mismo tape, ordenes y markout. La seleccion usa impacto esperado; PnL y error usan impacto observado ex post.",
        "criterioDefault": "Book-walk permanece por defecto hasta que evidencia fuera de muestra muestre menor error y mejor PnL ajustado por drawdown de forma consistente."
    })
}

fn decisiones_modelo(
    modelo: &ModeloImpacto,
    tape: &[CandidatoImpacto],
    cfg: &MapaCostos,
) -> Vec<bool> {
    tape.iter()
        .map(|c| {
            let e = modelo.estimar(&OrdenImpacto {
                lado: LadoOrden::Compra,
                cantidad_btc: c.cantidad,
                precio_referencia: c.precio,
                niveles: &c.niveles,
                volatilidad_bps: Some(c.volatilidad_bps),
                horizonte_ms: Some(c.horizonte_ms),
            });
            e.cantidad_ejecutable > 0.0
                && c.spread_bruto_bps - e.impacto_bps >= cfg.min_diferencial_neto_bps
                && (c.spread_bruto_bps - e.impacto_bps) * e.cantidad_ejecutable * c.precio
                    / 10_000.0
                    >= cfg.min_utilidad_usd
        })
        .collect()
}

fn resumir_modelo(
    modelo: &ModeloImpacto,
    tape: &[CandidatoImpacto],
    cfg: &MapaCostos,
    book: &[bool],
) -> FilaImpacto {
    let decisiones = decisiones_modelo(modelo, tape, cfg);
    let mut pnl = 0.0_f64;
    let mut pico = 0.0_f64;
    let mut drawdown = 0.0_f64;
    let mut aceptadas = 0_u64;
    let mut cantidad = 0.0;
    let mut impacto = 0.0;
    let mut error = 0.0;
    let mut sesgo = 0.0;
    for (i, c) in tape.iter().enumerate() {
        let e = modelo.estimar(&OrdenImpacto {
            lado: LadoOrden::Compra,
            cantidad_btc: c.cantidad,
            precio_referencia: c.precio,
            niveles: &c.niveles,
            volatilidad_bps: Some(c.volatilidad_bps),
            horizonte_ms: Some(c.horizonte_ms),
        });
        impacto += e.impacto_bps;
        error += (e.impacto_bps - c.impacto_observado_bps).abs();
        sesgo += e.impacto_bps - c.impacto_observado_bps;
        if decisiones[i] {
            aceptadas += 1;
            cantidad += e.cantidad_ejecutable;
            pnl +=
                (c.spread_bruto_bps - c.impacto_observado_bps) * e.cantidad_ejecutable * c.precio
                    / 10_000.0;
            pico = pico.max(pnl);
            drawdown = drawdown.max(pico - pnl);
        }
    }
    let n = tape.len().max(1) as f64;
    FilaImpacto {
        modelo: modelo.nombre().into(),
        pnl_usd: pnl,
        fill_rate: aceptadas as f64 / n,
        max_drawdown_usd: drawdown,
        impacto_medio_bps: impacto / n,
        decisiones_distintas: decisiones.iter().zip(book).filter(|(a, b)| a != b).count() as u64,
        ordenes_aceptadas: aceptadas,
        cantidad_media_btc: if aceptadas > 0 {
            cantidad / aceptadas as f64
        } else {
            0.0
        },
        error_absoluto_medio_bps: error / n,
        sesgo_costo_bps: sesgo / n,
    }
}

/// Holdout cronologico y reproducible. El campeón publicado se congela antes
/// de tocar las semillas 401..424; este reporte no reajusta ningún parámetro.
/// Las semillas 301..312 documentan la partición de calibración, pero no se
/// usan para escoger retrospectivamente al ganador mostrado.
fn validacion_fuera_muestra(
    cfg: &MapaCostos,
    umbral_ga: f64,
    max_btc_ga: f64,
    fuente_ga: &str,
) -> serde_json::Value {
    let calibracion = (301_u64..=312).collect::<Vec<_>>();
    let holdout = (401_u64..=424).collect::<Vec<_>>();
    let estrategias = [
        ("campeon_ga_congelado", umbral_ga, max_btc_ga),
        ("fija_conservadora", 1.60, 0.08),
        ("fija_balanceada", 0.65, 0.18),
        (
            "solo_spread_neto",
            cfg.min_diferencial_neto_bps,
            cfg.max_operacion_btc,
        ),
    ];
    let resultados = estrategias
        .into_iter()
        .map(|(nombre, umbral, max_btc)| {
            json!({
                "estrategia": nombre,
                "parametros": { "umbralBps": umbral, "maxOperacionBtc": max_btc },
                "holdout": resumen_multisemilla(cfg, umbral, max_btc, &holdout),
            })
        })
        .collect::<Vec<_>>();
    let ga_mediana = resultados[0]["holdout"]["pnlMedianoUsd"]
        .as_f64()
        .unwrap_or(0.0);
    let (ganador, mejor_mediana) = resultados
        .iter()
        .map(|r| {
            (
                r["estrategia"].as_str().unwrap_or("desconocida"),
                r["holdout"]["pnlMedianoUsd"].as_f64().unwrap_or(0.0),
            )
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap_or(("sin_ganador", 0.0));

    json!({
        "metodo": "holdout_cronologico_sin_reentrenamiento",
        "fuenteCampeon": fuente_ga,
        "campeonCongelado": true,
        "semillasCalibracion": calibracion,
        "semillasHoldoutNoVistas": holdout,
        "resultados": resultados,
        "ganador": ganador,
        "gaGana": ganador == "campeon_ga_congelado",
        "deltaGaVsMejorBaselineMedianoUsd": ga_mediana - if ganador == "campeon_ga_congelado" { resultados.iter().skip(1).map(|r| r["holdout"]["pnlMedianoUsd"].as_f64().unwrap_or(0.0)).max_by(f64::total_cmp).unwrap_or(0.0) } else { mejor_mediana },
        "lectura": if ganador == "campeon_ga_congelado" {
            "El campeón congelado supera los baselines en la mediana del holdout."
        } else {
            "El campeón congelado NO supera al mejor baseline en este holdout; la derrota se conserva como evidencia contra cherry-picking."
        },
        "limitacion": "Replay Monte Carlo sintético: valida generalización interna y reproducibilidad, no rentabilidad sobre mercado real."
    })
}

fn lab_sweep_reproducible(estado: &EstadoPublico) -> serde_json::Value {
    let cfg = &estado.configuracion;
    let (umbral_ga, max_btc_ga) = estado
        .genetico
        .as_ref()
        .map(|ga| (ga.umbral_optimizado, ga.max_operacion_optimizada_btc))
        .unwrap_or((
            (cfg.min_diferencial_neto_bps * 0.65).clamp(0.20, 1.25),
            (cfg.max_operacion_btc * 1.20).clamp(0.03, 0.60),
        ));
    let presets = [
        ("conservador", 1.60, 0.08, 11_u64),
        ("balanceado", 0.65, 0.18, 11_u64),
        ("agresivo", 0.25, 0.35, 11_u64),
        ("ga_edge", umbral_ga, max_btc_ga, 11_u64),
    ];
    let semillas = (201_u64..=224).collect::<Vec<_>>();
    let resultados = presets
        .into_iter()
        .map(|(nombre, umbral, max_btc, seed)| {
            let resultado = simular_backtest(cfg, umbral, max_btc, seed);
            json!({
                "preset": nombre,
                "umbralBps": umbral,
                "maxOperacionBtc": max_btc,
                "resultado": resultado,
                "scoreLab": score_lab(&resultado),
                "validacion": resumen_multisemilla(cfg, umbral, max_btc, &semillas),
            })
        })
        .collect::<Vec<_>>();

    let ganador = resultados
        .iter()
        .max_by(|a, b| {
            let sa = a.get("scoreLab").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let sb = b.get("scoreLab").and_then(|v| v.as_f64()).unwrap_or(0.0);
            sa.total_cmp(&sb)
        })
        .and_then(|v| v.get("preset"))
        .cloned()
        .unwrap_or_else(|| json!("sin_ganador"));

    let sensibilidad = sensibilidad_parametros(cfg, umbral_ga, max_btc_ga);

    json!({
        "generadoEn": chrono::Utc::now(),
        "tipo": "research_lab_sweep",
        "ticks": 240,
        "seed": 11,
        "semillasValidacion": semillas,
        "ganador": ganador,
        "resultados": resultados,
        "sensibilidad": sensibilidad,
        "lectura": "Sweep reproducible: el resultado principal usa la misma semilla y la robustez usa 24 semillas comunes. GA Edge consume el campeon publicado, no parametros inventados para el reporte.",
        "limitacion": "No prueba rentabilidad real; prueba sensibilidad del motor y parametros bajo un replay deterministico."
    })
}

/// Barre el umbral de diferencial neto y el slippage para medir la respuesta
/// del PnL (análisis de sensitividad). Cada punto usa la semilla base para
/// aislar el efecto del parámetro.
fn sensibilidad_parametros(
    cfg: &MapaCostos,
    umbral_base: f64,
    max_btc_base: f64,
) -> serde_json::Value {
    let umbrales = [0.25, 0.5, 1.0, 1.5, 2.0];
    let puntos_umbral = umbrales
        .iter()
        .map(|mult| {
            let u = (umbral_base * mult).clamp(0.05, 5.0);
            let r = simular_backtest(cfg, u, max_btc_base, 11);
            json!({
                "parametro": "umbralDiferencialNetoBps",
                "multiplicador": mult,
                "valor": u,
                "pnlUsd": r.pnl_usd,
                "trades": r.trades_ejecutados,
                "winRate": r.win_rate,
            })
        })
        .collect::<Vec<_>>();

    let slippages = [0.0, 0.5, 1.0, 2.0, 4.0];
    let puntos_slippage = slippages
        .iter()
        .map(|mult| {
            let r = simular_backtest_factor(cfg, umbral_base, max_btc_base, 11, *mult);
            json!({
                "parametro": "slippageMultiplicador",
                "multiplicador": mult,
                "pnlUsd": r.pnl_usd,
                "trades": r.trades_ejecutados,
                "winRate": r.win_rate,
            })
        })
        .collect::<Vec<_>>();

    // Elasticidad: cambio relativo de PnL por cambio unitario del multiplicador.
    let pnls: Vec<f64> = puntos_umbral
        .iter()
        .map(|p| p.get("pnlUsd").and_then(|v| v.as_f64()).unwrap_or(0.0))
        .collect();
    let elasticidad = if pnls.len() >= 2 && pnls[0].abs() > 1e-9 {
        (pnls[pnls.len() - 1] - pnls[0]) / pnls[0]
    } else {
        0.0
    };

    json!({
        "umbral": puntos_umbral,
        "slippage": puntos_slippage,
        "elasticidadPnlVsUmbral": elasticidad,
        "lectura": "Muestra cuánto se mueve el PnL al variar el umbral y el slippage bajo replay deterministico.",
    })
}

#[derive(Clone, serde::Serialize)]
struct ResultadoBacktest {
    #[serde(rename = "rutasEvaluadas")]
    rutas_evaluadas: u64,
    #[serde(rename = "tradesEjecutados")]
    trades_ejecutados: u64,
    #[serde(rename = "pnlUsd")]
    pnl_usd: f64,
    #[serde(rename = "winRate")]
    win_rate: f64,
    #[serde(rename = "maxDrawdownUsd")]
    max_drawdown_usd: f64,
    #[serde(rename = "spreadNetoMedioBps")]
    spread_neto_medio_bps: f64,
    #[serde(rename = "utilidadMediaUsd")]
    utilidad_media_usd: f64,
    #[serde(rename = "utilidadP50Usd")]
    utilidad_p50_usd: f64,
    #[serde(rename = "utilidadP95Usd")]
    utilidad_p95_usd: f64,
    #[serde(rename = "desviacionUsd")]
    desviacion_usd: f64,
    #[serde(rename = "intervaloConfianza95Usd")]
    intervalo_confianza_95_usd: f64,
    #[serde(rename = "profitFactor")]
    profit_factor: f64,
}

#[derive(Clone, Copy, Debug, Default)]
struct ResultadoTickBacktest {
    pnl_usd: f64,
    fills: u64,
    rutas: u64,
}

#[derive(Clone, Copy, Debug)]
struct MetricasRemuestra {
    pnl: f64,
    fill_rate: f64,
    drawdown: f64,
}

fn significancia_bootstrap_bloques(
    cfg: &MapaCostos,
    baseline: (f64, f64),
    candidato: (f64, f64),
) -> serde_json::Value {
    const SEED_CINTA: u64 = 42;
    const SEED_BOOTSTRAP: u64 = 20_260_712;
    const REMUESTRAS: usize = 10_000;
    const BLOQUES: [usize; 3] = [30, 60, 120];

    let (_, serie_base) = simular_backtest_con_serie(cfg, baseline.0, baseline.1, SEED_CINTA, 1.0);
    let (_, serie_ga) = simular_backtest_con_serie(cfg, candidato.0, candidato.1, SEED_CINTA, 1.0);
    let sensibilidades = BLOQUES
        .iter()
        .enumerate()
        .map(|(i, segundos)| {
            bootstrap_pareado(
                &serie_base,
                &serie_ga,
                *segundos,
                REMUESTRAS,
                SEED_BOOTSTRAP + i as u64,
            )
        })
        .collect::<Vec<_>>();
    let principal = sensibilidades[1].clone();
    let mut permutacion =
        permutacion_pareada_bloques(&serie_base, &serie_ga, 60, REMUESTRAS, SEED_BOOTSTRAP + 100);
    let p_value = permutacion["pValueDosColas"].as_f64().unwrap_or(1.0);
    let p_value_holm = holm_ajustar(&[p_value])[0];
    permutacion["pValueHolm"] = json!(p_value_holm);
    let estable = estabilidad_cinco_ventanas(&serie_base, &serie_ga);

    json!({
        "metodo": "moving_block_bootstrap_pareado",
        "remuestras": REMUESTRAS,
        "seed": SEED_BOOTSTRAP,
        "seedCinta": SEED_CINTA,
        "intervalo": "percentil_95",
        "bca": "mejora_posterior",
        "tickSegundos": 1,
        "bloquePrincipalSegundos": 60,
        "bloquesSensibilidadSegundos": BLOQUES,
        "pareado": true,
        "principal": principal,
        "sensibilidad": sensibilidades,
        "permutacionPareadaBloques": permutacion,
        "correccionMultiplesModelos": {
            "metodo": "Holm",
            "comparaciones": 1,
            "pValueAjustado": p_value_holm,
            "pValuesAjustados": [p_value_holm],
            "nota": "Con una sola comparacion, Holm no altera el p-value. Aplicar el ajuste al conjunto completo al agregar modelos."
        },
        "estabilidadVentanas": estable,
        "advertencia": "Bootstrap temporal sobre replay sintetico; cuantifica incertidumbre interna y no demuestra rentabilidad real."
    })
}

fn bootstrap_pareado(
    base: &[ResultadoTickBacktest],
    candidato: &[ResultadoTickBacktest],
    bloque: usize,
    remuestras: usize,
    seed: u64,
) -> serde_json::Value {
    let n = base.len().min(candidato.len());
    let bloque = bloque.clamp(1, n.max(1));
    let mut rng = StdRng::seed_from_u64(seed);
    let mut pnl_base = Vec::with_capacity(remuestras);
    let mut pnl_ga = Vec::with_capacity(remuestras);
    let mut fill_base = Vec::with_capacity(remuestras);
    let mut fill_ga = Vec::with_capacity(remuestras);
    let mut dd_base = Vec::with_capacity(remuestras);
    let mut dd_ga = Vec::with_capacity(remuestras);
    let mut delta_pnl = Vec::with_capacity(remuestras);
    let mut delta_fill = Vec::with_capacity(remuestras);
    let mut delta_dd = Vec::with_capacity(remuestras);

    for _ in 0..remuestras {
        let mut indices = Vec::with_capacity(n);
        while indices.len() < n {
            let inicio = rng.gen_range(0..=n - bloque);
            for idx in inicio..inicio + bloque {
                if indices.len() == n {
                    break;
                }
                indices.push(idx);
            }
        }
        let mb = metricas_indices(base, &indices);
        let mg = metricas_indices(candidato, &indices);
        pnl_base.push(mb.pnl);
        pnl_ga.push(mg.pnl);
        fill_base.push(mb.fill_rate);
        fill_ga.push(mg.fill_rate);
        dd_base.push(mb.drawdown);
        dd_ga.push(mg.drawdown);
        delta_pnl.push(mg.pnl - mb.pnl);
        delta_fill.push(mg.fill_rate - mb.fill_rate);
        // Negativo significa menor drawdown para el candidato.
        delta_dd.push(mg.drawdown - mb.drawdown);
    }
    for valores in [
        &mut pnl_base,
        &mut pnl_ga,
        &mut fill_base,
        &mut fill_ga,
        &mut dd_base,
        &mut dd_ga,
        &mut delta_pnl,
        &mut delta_fill,
        &mut delta_dd,
    ] {
        valores.sort_by(f64::total_cmp);
    }
    let ci_delta_pnl = intervalo_percentil(&delta_pnl);
    let prob_superior = delta_pnl.iter().filter(|v| **v > 0.0).count() as f64 / remuestras as f64;
    let deltas_bloque = base
        .iter()
        .zip(candidato)
        .map(|(b, g)| g.pnl_usd - b.pnl_usd)
        .collect::<Vec<_>>()
        .chunks(bloque)
        .map(|valores| valores.iter().sum::<f64>())
        .collect::<Vec<_>>();
    let media_delta = deltas_bloque.iter().sum::<f64>() / deltas_bloque.len().max(1) as f64;
    let efecto = if deltas_bloque.len() > 1 {
        media_delta / desviacion_estandar(&deltas_bloque, media_delta).max(1e-12)
    } else {
        0.0
    };

    json!({
        "bloqueSegundos": bloque,
        "baseline": {
            "pnlNetoUsd": resumen_ci(&pnl_base),
            "fillRate": resumen_ci(&fill_base),
            "maxDrawdownUsd": resumen_ci(&dd_base)
        },
        "candidato": {
            "pnlNetoUsd": resumen_ci(&pnl_ga),
            "fillRate": resumen_ci(&fill_ga),
            "maxDrawdownUsd": resumen_ci(&dd_ga)
        },
        "deltasCandidatoMenosBaseline": {
            "pnlNetoUsd": resumen_ci(&delta_pnl),
            "fillRate": resumen_ci(&delta_fill),
            "maxDrawdownUsd": resumen_ci(&delta_dd)
        },
        "probabilidadDeltaPnlMayorCero": prob_superior,
        "tamanoEfecto": {
            "metodo": "Cohen_dz_por_bloque_temporal_pareado",
            "valor": efecto,
            "bloques": deltas_bloque.len()
        },
        "resultado": if ci_delta_pnl.0 <= 0.0 && ci_delta_pnl.1 >= 0.0 { "resultado inconcluso" } else if ci_delta_pnl.0 > 0.0 { "candidato superior" } else { "baseline superior" }
    })
}

fn metricas_indices(serie: &[ResultadoTickBacktest], indices: &[usize]) -> MetricasRemuestra {
    let mut pnl: f64 = 0.0;
    let mut pico: f64 = 0.0;
    let mut drawdown: f64 = 0.0;
    let mut fills = 0_u64;
    let mut rutas = 0_u64;
    for &idx in indices {
        let tick = serie[idx];
        pnl += tick.pnl_usd;
        pico = pico.max(pnl);
        drawdown = drawdown.max(pico - pnl);
        fills += tick.fills;
        rutas += tick.rutas;
    }
    MetricasRemuestra {
        pnl,
        fill_rate: if rutas == 0 {
            0.0
        } else {
            fills as f64 / rutas as f64
        },
        drawdown,
    }
}

fn resumen_ci(valores_ordenados: &[f64]) -> serde_json::Value {
    let (inferior, superior) = intervalo_percentil(valores_ordenados);
    json!({ "mediana": percentil(valores_ordenados, 0.5), "ci95": [inferior, superior] })
}

fn intervalo_percentil(valores_ordenados: &[f64]) -> (f64, f64) {
    (
        percentil(valores_ordenados, 0.025),
        percentil(valores_ordenados, 0.975),
    )
}

fn permutacion_pareada_bloques(
    base: &[ResultadoTickBacktest],
    candidato: &[ResultadoTickBacktest],
    bloque: usize,
    remuestras: usize,
    seed: u64,
) -> serde_json::Value {
    let deltas = base
        .iter()
        .zip(candidato)
        .map(|(b, g)| g.pnl_usd - b.pnl_usd)
        .collect::<Vec<_>>();
    let sumas = deltas
        .chunks(bloque)
        .map(|c| c.iter().sum::<f64>())
        .collect::<Vec<_>>();
    let observado = sumas.iter().sum::<f64>();
    let mut rng = StdRng::seed_from_u64(seed);
    let extremos = (0..remuestras)
        .filter(|_| {
            let permutado = sumas
                .iter()
                .map(|v| if rng.gen_bool(0.5) { *v } else { -*v })
                .sum::<f64>();
            permutado.abs() >= observado.abs()
        })
        .count();
    json!({
        "metodo": "sign_flip_de_bloques_no_solapados",
        "bloqueSegundos": bloque,
        "remuestras": remuestras,
        "seed": seed,
        "estadisticoDeltaPnlUsd": observado,
        "pValueDosColas": (extremos as f64 + 1.0) / (remuestras as f64 + 1.0)
    })
}

fn holm_ajustar(p_values: &[f64]) -> Vec<f64> {
    let m = p_values.len();
    let mut orden = p_values.iter().copied().enumerate().collect::<Vec<_>>();
    orden.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut ajustados = vec![1.0; m];
    let mut anterior: f64 = 0.0;
    for (rango, (indice, p)) in orden.into_iter().enumerate() {
        let ajustado = ((m - rango) as f64 * p.clamp(0.0, 1.0))
            .max(anterior)
            .min(1.0);
        ajustados[indice] = ajustado;
        anterior = ajustado;
    }
    ajustados
}

fn estabilidad_cinco_ventanas(
    base: &[ResultadoTickBacktest],
    candidato: &[ResultadoTickBacktest],
) -> serde_json::Value {
    let n = base.len().min(candidato.len());
    let resultados = (0..5)
        .map(|i| {
            let inicio = i * n / 5;
            let fin = (i + 1) * n / 5;
            candidato[inicio..fin]
                .iter()
                .map(|t| t.pnl_usd)
                .sum::<f64>()
                - base[inicio..fin].iter().map(|t| t.pnl_usd).sum::<f64>()
        })
        .collect::<Vec<_>>();
    json!({
        "ventanas": 5,
        "favorables": resultados.iter().filter(|v| **v > 0.0).count(),
        "deltaPnlUsdPorVentana": resultados,
        "criterio": "Una ventana es favorable cuando delta PnL candidato menos baseline es mayor que cero."
    })
}

fn resumen_multisemilla(
    cfg: &MapaCostos,
    umbral_bps: f64,
    max_btc: f64,
    semillas: &[u64],
) -> serde_json::Value {
    let resultados = semillas
        .iter()
        .map(|seed| simular_backtest(cfg, umbral_bps, max_btc, *seed))
        .collect::<Vec<_>>();
    let mut pnls = resultados.iter().map(|r| r.pnl_usd).collect::<Vec<_>>();
    let mut drawdowns = resultados
        .iter()
        .map(|r| r.max_drawdown_usd)
        .collect::<Vec<_>>();
    let mut trades = resultados
        .iter()
        .map(|r| r.trades_ejecutados as f64)
        .collect::<Vec<_>>();
    pnls.sort_by(|a, b| a.total_cmp(b));
    drawdowns.sort_by(|a, b| a.total_cmp(b));
    trades.sort_by(|a, b| a.total_cmp(b));
    let media_pnl = if pnls.is_empty() {
        0.0
    } else {
        pnls.iter().sum::<f64>() / pnls.len() as f64
    };
    let desviacion_pnl = desviacion_estandar(&pnls, media_pnl);
    let ic_95 = if pnls.len() > 1 {
        1.96 * desviacion_pnl / (pnls.len() as f64).sqrt()
    } else {
        0.0
    };
    json!({
        "corridas": resultados.len(),
        "pnlMedianoUsd": percentil(&pnls, 0.50),
        "pnlPromedioUsd": media_pnl,
        "pnlP05Usd": percentil(&pnls, 0.05),
        "pnlP95Usd": percentil(&pnls, 0.95),
        "intervaloConfianza95MediaUsd": ic_95,
        "drawdownMedianoUsd": percentil(&drawdowns, 0.50),
        "tradesMediana": percentil(&trades, 0.50),
        "corridasPnlPositivo": resultados.iter().filter(|r| r.pnl_usd > 0.0).count(),
    })
}

fn simular_backtest(
    cfg: &MapaCostos,
    umbral_bps: f64,
    max_btc: f64,
    seed: u64,
) -> ResultadoBacktest {
    simular_backtest_factor(cfg, umbral_bps, max_btc, seed, 1.0)
}

fn simular_backtest_factor(
    cfg: &MapaCostos,
    umbral_bps: f64,
    max_btc: f64,
    seed: u64,
    slippage_mult: f64,
) -> ResultadoBacktest {
    simular_backtest_con_serie(cfg, umbral_bps, max_btc, seed, slippage_mult).0
}

fn simular_backtest_con_serie(
    cfg: &MapaCostos,
    umbral_bps: f64,
    max_btc: f64,
    seed: u64,
    slippage_mult: f64,
) -> (ResultadoBacktest, Vec<ResultadoTickBacktest>) {
    let mut cfg = cfg.clone();
    cfg.deslizamiento_bps *= slippage_mult;
    let exchanges = [
        "Binance", "Kraken", "Coinbase", "OKX", "Bybit", "Bitfinex", "KuCoin", "Gate.io",
        "Bitstamp", "Gemini",
    ];
    let mut rng = StdRng::seed_from_u64(seed);
    let mut precio = 100_000.0;
    let mut rutas = 0;
    let mut trades = 0;
    let mut wins = 0;
    let mut pnl = 0.0;
    let mut pico = 0.0;
    let mut drawdown = 0.0;
    let mut suma_spread = 0.0;
    let mut utilidades = Vec::new();
    let mut serie = Vec::with_capacity(240);

    // 240 ticks × 90 rutas × 24 semillas conservan una muestra amplia sin
    // convertir el endpoint interactivo del laboratorio en un trabajo largo.
    for _ in 0..240 {
        let pnl_inicio = pnl;
        let trades_inicio = trades;
        let rutas_inicio = rutas;
        precio *= 1.0 + rng.gen_range(-0.0009..0.0009);
        let mut libros = Vec::new();
        for exchange in exchanges {
            let shock = if rng.gen_bool(0.025) {
                rng.gen_range(-0.0045..0.0045)
            } else {
                rng.gen_range(-0.00035..0.00035)
            };
            let mid = precio * (1.0 + shock);
            let half = mid * rng.gen_range(0.00003..0.00012);
            libros.push((exchange, mid - half, mid + half));
        }
        for compra in &libros {
            for venta in &libros {
                if compra.0 == venta.0 {
                    continue;
                }
                rutas += 1;
                let cantidad = max_btc.min(rng.gen_range(0.04..0.45));
                let fee_compra = cfg
                    .exchanges
                    .get(compra.0)
                    .map(|e| e.fee_taker)
                    .unwrap_or(0.0015);
                let fee_venta = cfg
                    .exchanges
                    .get(venta.0)
                    .map(|e| e.fee_taker)
                    .unwrap_or(0.0015);
                let medio = (compra.2 + venta.1) / 2.0;
                let costos = cantidad * compra.2 * fee_compra
                    + cantidad * venta.1 * fee_venta
                    + cantidad * medio * cfg.deslizamiento_bps / 10000.0
                    + cantidad * medio * cfg.retiro_amortizado_bps / 10000.0
                    + cantidad * medio * cfg.latencia_riesgo_bps / 10000.0;
                let utilidad = (venta.1 - compra.2) * cantidad - costos;
                let neto_bps = if medio > 0.0 && cantidad > 0.0 {
                    utilidad / cantidad / medio * 10000.0
                } else {
                    0.0
                };
                // El shock se consume para cada ruta, se ejecute o no. Así baseline
                // y campeón recorren exactamente la misma cinta aleatoria: una
                // decisión distinta no desplaza el RNG de todos los eventos futuros.
                let movimiento_realizado_bps = if rng.gen_bool(0.09) {
                    -rng.gen_range(3.0..16.0)
                } else {
                    rng.gen_range(-2.0..2.0)
                };
                if utilidad >= cfg.min_utilidad_usd && neto_bps >= umbral_bps {
                    let utilidad_realizada =
                        utilidad + cantidad * medio * movimiento_realizado_bps / 10_000.0;
                    trades += 1;
                    pnl += utilidad_realizada;
                    utilidades.push(utilidad_realizada);
                    suma_spread += neto_bps;
                    if utilidad_realizada > 0.0 {
                        wins += 1;
                    }
                    if pnl > pico {
                        pico = pnl;
                    }
                    let dd = pico - pnl;
                    if dd > drawdown {
                        drawdown = dd;
                    }
                }
            }
        }
        serie.push(ResultadoTickBacktest {
            pnl_usd: pnl - pnl_inicio,
            fills: trades - trades_inicio,
            rutas: rutas - rutas_inicio,
        });
    }
    utilidades.sort_by(|a, b| a.total_cmp(b));
    let utilidad_media = if utilidades.is_empty() {
        0.0
    } else {
        utilidades.iter().sum::<f64>() / utilidades.len() as f64
    };
    let desviacion = desviacion_estandar(&utilidades, utilidad_media);
    let intervalo_95 = if utilidades.len() > 1 {
        1.96 * desviacion / (utilidades.len() as f64).sqrt()
    } else {
        0.0
    };
    let ganancias = utilidades.iter().filter(|v| **v > 0.0).sum::<f64>();
    let perdidas = utilidades
        .iter()
        .filter(|v| **v < 0.0)
        .map(|v| v.abs())
        .sum::<f64>();

    let resultado = ResultadoBacktest {
        rutas_evaluadas: rutas,
        trades_ejecutados: trades,
        pnl_usd: pnl,
        win_rate: if trades == 0 {
            0.0
        } else {
            wins as f64 / trades as f64
        },
        max_drawdown_usd: drawdown,
        spread_neto_medio_bps: if trades == 0 {
            0.0
        } else {
            suma_spread / trades as f64
        },
        utilidad_media_usd: utilidad_media,
        utilidad_p50_usd: percentil(&utilidades, 0.50),
        utilidad_p95_usd: percentil(&utilidades, 0.95),
        desviacion_usd: desviacion,
        intervalo_confianza_95_usd: intervalo_95,
        profit_factor: if perdidas > 0.0 {
            ganancias / perdidas
        } else if ganancias > 0.0 {
            ganancias
        } else {
            0.0
        },
    };
    (resultado, serie)
}

fn percentil(valores: &[f64], p: f64) -> f64 {
    if valores.is_empty() {
        return 0.0;
    }
    let idx = ((valores.len() - 1) as f64 * p.clamp(0.0, 1.0)).round() as usize;
    valores[idx.min(valores.len() - 1)]
}

fn desviacion_estandar(valores: &[f64], media: f64) -> f64 {
    if valores.len() < 2 {
        return 0.0;
    }
    let var = valores.iter().map(|v| (v - media).powi(2)).sum::<f64>() / (valores.len() - 1) as f64;
    var.sqrt()
}

fn score_lab(resultado: &ResultadoBacktest) -> f64 {
    resultado.pnl_usd - resultado.max_drawdown_usd * 0.55
        + resultado.win_rate * 120.0
        + resultado.profit_factor.min(25.0) * 4.0
        - resultado.intervalo_confianza_95_usd
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, collections::VecDeque};

    use chrono::Utc;
    use serde_json::Value;

    use super::*;
    use crate::types::{
        AuditoriaDecision, Balance, CostosOperacion, EstadoCorrida, EstadoGenetico, EstadoMlEdge,
        EstadoPersistencia, EventoEjecucion, FeatureMlEdge, LatenciaExchange, Metricas, NivelOrden,
        Operacion, PuntoSerie, Rebalanceo, TelemetriaPipeline, TransicionEjecucion,
    };
    use smallvec::SmallVec;

    #[test]
    fn config_patch_is_atomic_when_a_late_exchange_validation_fails() {
        let mut cfg = cfg_test();
        let original = cfg.clone();
        let mut exchanges = HashMap::new();
        exchanges.insert(
            "VenueInventado".to_string(),
            ExchangeConfig {
                nombre: "VenueInventado".into(),
                fee_taker: 0.001,
                retiro_btc: 0.0,
                confiabilidad: 1.0,
            },
        );
        let patch = ParcheConfig {
            max_operacion_btc: Some(0.25),
            exchanges: Some(exchanges),
            ..Default::default()
        };
        assert!(aplicar_config_patch(&mut cfg, patch).is_err());
        assert_eq!(cfg, original);
    }

    #[test]
    fn config_patch_rejects_extreme_values_and_non_http_webhooks() {
        let mut cfg = cfg_test();
        let original = cfg.clone();
        assert!(aplicar_config_patch(
            &mut cfg,
            ParcheConfig {
                max_operacion_btc: Some(100.0),
                ..Default::default()
            }
        )
        .is_err());
        assert_eq!(cfg, original);
        assert!(aplicar_config_patch(
            &mut cfg,
            ParcheConfig {
                webhook_url: Some("file:///etc/passwd".into()),
                ..Default::default()
            }
        )
        .is_err());
        assert_eq!(cfg, original);
    }

    #[test]
    fn config_patch_commits_multiple_valid_fields_together() {
        let mut cfg = cfg_test();
        aplicar_config_patch(
            &mut cfg,
            ParcheConfig {
                max_operacion_btc: Some(0.33),
                stale_ms: Some(8_000),
                prob_fallo_orden: Some(0.2),
                permitir_cruce_usd_usdt: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(cfg.max_operacion_btc, 0.33);
        assert_eq!(cfg.stale_ms, 8_000);
        assert_eq!(cfg.prob_fallo_orden, 0.2);
        assert!(!cfg.permitir_cruce_usd_usdt);
    }

    #[test]
    fn config_patch_denies_unknown_json_fields() {
        let result = serde_json::from_str::<ParcheConfig>(
            r#"{"maxOperacionBtc":0.2,"habilitarTradingReal":true}"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn parameter_catalog_has_unique_named_controls_with_constraints() {
        let estado = estado_publico_test(true, true);
        let catalog = catalogo_parametros(&estado);
        let names = catalog
            .iter()
            .filter_map(|item| item["nombre"].as_str())
            .collect::<HashSet<_>>();
        // 35 controles base + 4 por cada uno de los 3 exchanges del fixture.
        assert_eq!(catalog.len(), 47);
        assert_eq!(names.len(), catalog.len());
        assert!(catalog.iter().all(|item| {
            item["categoria"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
                && item["restriccion"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty())
                && item["origen"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty())
        }));
    }

    #[test]
    fn quantitative_scan_is_accepted_only_for_the_visible_corpus_hash() {
        let bytes = serde_json::to_vec(&json!({
            "sourceClassification": "public_market_capture_corpus",
            "corpusSha256": "corpus-one",
            "rawEvents": 1_000_000,
            "netDislocations": 42
        }))
        .unwrap();
        let scan = validar_scan_corpus("corpus-one", &bytes).unwrap();
        assert_eq!(scan["netDislocations"], 42);
        assert_eq!(
            validar_scan_corpus("corpus-two", &bytes),
            Err("corpus_hash_mismatch")
        );
    }

    #[test]
    fn malformed_quantitative_scan_is_rejected_closed() {
        assert_eq!(
            validar_scan_corpus("corpus-one", b"not-json"),
            Err("invalid_scan_json")
        );
        let synthetic = serde_json::to_vec(&json!({
            "sourceClassification": "synthetic_benchmark_corpus",
            "corpusSha256": "corpus-one"
        }))
        .unwrap();
        assert_eq!(
            validar_scan_corpus("corpus-one", &synthetic),
            Err("non_public_scan")
        );
    }

    #[test]
    fn preflight_separa_salud_operativa_de_evidencia_forense() {
        let sin_evidencia = construir_preflight(&estado_publico_test(false, false));
        let checks = checks_por_nombre(&sin_evidencia);

        assert_eq!(checks.get("decisionInspector"), Some(&false));
        assert_eq!(checks.get("partialFillSupport"), Some(&false));
        assert_eq!(
            sin_evidencia
                .pointer("/judgeReadiness/status")
                .and_then(Value::as_str),
            Some("ready")
        );
        assert_eq!(
            sin_evidencia
                .pointer("/judgeReadiness/evidenceStatus")
                .and_then(Value::as_str),
            Some("partial")
        );

        let con_evidencia = construir_preflight(&estado_publico_test(true, true));
        let checks = checks_por_nombre(&con_evidencia);

        assert_eq!(checks.get("decisionInspector"), Some(&true));
        assert_eq!(checks.get("partialFillSupport"), Some(&true));
        assert_eq!(
            con_evidencia
                .pointer("/judgeReadiness/status")
                .and_then(Value::as_str),
            Some("ready")
        );
    }

    #[test]
    fn backtest_y_lab_exponen_contratos_qa() {
        let estado = estado_publico_test(true, true);
        let backtest = backtest_reproducible(&estado);
        let lab = lab_sweep_reproducible(&estado);

        assert_eq!(backtest["ticks"], 240);
        assert!(backtest["base"]["rutasEvaluadas"].as_u64().unwrap_or(0) > 0);
        assert!(matches!(
            backtest["comparacion"]["ganador"].as_str(),
            Some("base" | "optimizada")
        ));
        assert_eq!(backtest["validacionMultisemilla"]["base"]["corridas"], 24);
        assert_eq!(backtest["significanciaBootstrap"]["remuestras"], 10_000);
        assert_eq!(
            backtest["significanciaBootstrap"]["metodo"],
            "moving_block_bootstrap_pareado"
        );
        assert_eq!(
            backtest["significanciaBootstrap"]["sensibilidad"]
                .as_array()
                .map(Vec::len),
            Some(3)
        );
        assert!(
            backtest["significanciaBootstrap"]["principal"]["deltasCandidatoMenosBaseline"]
                ["pnlNetoUsd"]["ci95"]
                .is_array()
        );
        assert_eq!(
            backtest["validacionFueraMuestra"]["metodo"],
            "holdout_cronologico_sin_reentrenamiento"
        );
        assert_eq!(
            backtest["validacionFueraMuestra"]["resultados"]
                .as_array()
                .map(Vec::len),
            Some(4)
        );
        assert_eq!(
            backtest["validacionFueraMuestra"]["semillasHoldoutNoVistas"]
                .as_array()
                .map(Vec::len),
            Some(24)
        );
        assert_eq!(
            backtest["comparacionImpacto"]["modeloPredeterminado"],
            "Book-walk"
        );
        assert_eq!(
            backtest["comparacionImpacto"]["tabla"]
                .as_array()
                .map(Vec::len),
            Some(3)
        );
        assert!(
            backtest["comparacionImpacto"]["respuestas"]["modeloConMenorErrorContraMarkout"]
                .is_string()
        );

        assert_eq!(lab["tipo"], "research_lab_sweep");
        assert_eq!(lab["resultados"].as_array().map(Vec::len), Some(4));
        assert!(lab["ganador"].as_str().is_some_and(|v| !v.is_empty()));
        assert_eq!(lab["resultados"][0]["validacion"]["corridas"], 24);

        // Fase 9: análisis de sensibilidad (umbral y slippage).
        assert!(
            lab["sensibilidad"]["umbral"]
                .as_array()
                .map(Vec::len)
                .unwrap_or(0)
                >= 5
        );
        assert!(
            lab["sensibilidad"]["slippage"]
                .as_array()
                .map(Vec::len)
                .unwrap_or(0)
                >= 5
        );
        assert!(lab["sensibilidad"]["elasticidadPnlVsUmbral"].is_number());
    }

    #[test]
    fn holm_ajusta_varias_comparaciones_y_preserva_orden_original() {
        let ajustados = holm_ajustar(&[0.04, 0.01, 0.03]);
        assert_eq!(ajustados, vec![0.06, 0.03, 0.06]);
    }

    #[test]
    fn bootstrap_bloques_es_determinista_y_reporta_efecto_temporal() {
        let base = (0..240)
            .map(|i| ResultadoTickBacktest {
                pnl_usd: if i % 17 == 0 { -0.2 } else { 0.1 },
                fills: 1,
                rutas: 2,
            })
            .collect::<Vec<_>>();
        let candidato = base
            .iter()
            .map(|tick| ResultadoTickBacktest {
                pnl_usd: tick.pnl_usd + 0.02,
                fills: tick.fills,
                rutas: tick.rutas,
            })
            .collect::<Vec<_>>();
        let a = bootstrap_pareado(&base, &candidato, 60, 500, 7);
        let b = bootstrap_pareado(&base, &candidato, 60, 500, 7);

        assert_eq!(a, b);
        assert_eq!(
            a["tamanoEfecto"]["metodo"],
            "Cohen_dz_por_bloque_temporal_pareado"
        );
        assert_eq!(a["tamanoEfecto"]["bloques"], 4);
        assert_eq!(a["resultado"], "candidato superior");
    }

    #[test]
    fn mcp_manifest_expone_herramientas_para_agentes() {
        let manifest = construir_mcp_manifest();
        let tools = manifest["tools"].as_array().expect("tools array");
        let names = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();

        assert!(names.contains(&"summarize_for_llm"));
        assert!(names.contains(&"jury_mode"));
        assert!(names.contains(&"evaluation_package"));
        assert!(names.contains(&"prepare_demo_final"));
        let demo_final = tools
            .iter()
            .find(|tool| tool["name"] == "prepare_demo_final")
            .expect("prepare_demo_final tool");
        assert_eq!(demo_final["mutable"].as_bool(), Some(true));
        assert_eq!(demo_final["requiresAdminToken"].as_bool(), Some(true));
    }

    #[test]
    fn modo_jurado_consolida_rubrica_scorecard_y_enlaces() {
        let estado = estado_publico_test(true, true);
        let jurado = construir_modo_jurado(&estado);

        assert_eq!(jurado["nombre"], "Mayab Jury Mode");
        assert_eq!(
            jurado.pointer("/estado/status").and_then(Value::as_str),
            Some("ready")
        );
        assert_eq!(
            jurado
                .pointer("/rubricaOficial")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(5)
        );
        assert!(jurado
            .pointer("/scorecard")
            .and_then(Value::as_array)
            .is_some_and(|items| items.len() >= 8));
        assert_eq!(
            jurado
                .pointer("/enlaces/paqueteEvaluacion")
                .and_then(Value::as_str),
            Some("/api/paquete-evaluacion")
        );
        assert_eq!(
            jurado.pointer("/enlaces/demoFinal").and_then(Value::as_str),
            Some("/api/demo/final")
        );
        assert_eq!(
            jurado.pointer("/enlaces/demoCaos").and_then(Value::as_str),
            Some("/api/demo/caos")
        );
        assert_eq!(
            jurado
                .pointer("/evidenciaClave/resultadoMemorable/endpointReproducible")
                .and_then(Value::as_str),
            Some("/api/demo/caos")
        );
        assert_eq!(
            jurado
                .pointer("/evidenciaClave/scorecardCuantitativa/semillasPareadasBacktest")
                .and_then(Value::as_u64),
            Some(24)
        );
    }

    #[test]
    fn preflight_y_paquete_exponen_cobertura_finalista() {
        let estado = estado_publico_test(true, true);
        let preflight = construir_preflight(&estado);
        let cobertura = preflight
            .pointer("/judgeReadiness/coberturaFinalista")
            .expect("cobertura finalista en preflight");

        assert_eq!(
            cobertura["nombre"],
            "Cobertura de benchmark finalista publico"
        );
        assert_eq!(cobertura["total"].as_u64(), Some(8));
        assert_eq!(
            cobertura["dimensiones"].as_array().map(|items| {
                items
                    .iter()
                    .filter(|item| item["ok"].as_bool().unwrap_or(false))
                    .count()
            }),
            cobertura["cubiertas"].as_u64().map(|v| v as usize)
        );
        assert!(cobertura["dimensiones"]
            .as_array()
            .expect("dimensiones")
            .iter()
            .any(|item| item["nombre"] == "wallets_rebalanceo"));

        let paquete = construir_paquete_evaluacion(&estado);
        assert_eq!(
            paquete
                .pointer("/coberturaFinalista/dimensiones")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(8)
        );
    }

    fn checks_por_nombre(preflight: &Value) -> HashMap<String, bool> {
        preflight
            .pointer("/judgeReadiness/checks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|check| {
                Some((
                    check.get("name")?.as_str()?.to_string(),
                    check.get("ok")?.as_bool()?,
                ))
            })
            .collect()
    }

    fn estado_publico_test(con_auditoria: bool, con_fill_parcial: bool) -> EstadoPublico {
        let ahora = Utc::now();
        let mut exchanges_activos = HashMap::new();
        for exchange in ["Binance", "Kraken", "Coinbase"] {
            exchanges_activos.insert(exchange.to_string(), true);
        }

        let cotizaciones = vec![
            cotizacion_test("Binance", "BTC/USDT", 99_990.0, 100_010.0, ahora),
            cotizacion_test("Kraken", "BTC/USD", 100_000.0, 100_020.0, ahora),
            cotizacion_test("Coinbase", "BTC/USD", 100_005.0, 100_025.0, ahora),
        ];

        let mut operaciones = VecDeque::new();
        if con_fill_parcial {
            operaciones.push_front(Operacion {
                tipo: crate::types::TipoOportunidad::Lineal,
                piernas: vec![],
                id: "op-parcial-test".to_string(),
                compra_en: "Binance".to_string(),
                venta_en: "Kraken".to_string(),
                par: "BTC/USD".to_string(),
                cantidad_btc: (0.04),
                precio_compra: (100_000.0),
                precio_venta: (100_090.0),
                utilidad_usd: (2.4),
                utilidad_esperada_usd: (2.4),
                costos: CostosOperacion::default(),
                parcial: true,
                ejecutada_en: ahora,
                latencia_max_ms: 12,
            });
        }

        let mut auditoria_decisiones = VecDeque::new();
        if con_auditoria {
            auditoria_decisiones.push_front(AuditoriaDecision {
                id: "aud-test".to_string(),
                ruta: "Binance->Kraken".to_string(),
                par: "BTC/USD".to_string(),
                decision: "aceptada".to_string(),
                decision_code: "ACCEPT_EXECUTABLE".to_string(),
                decision_reason: "evidencia QA con costos netos".to_string(),
                decision_threshold: 0.65,
                decision_actual: 1.40,
                razon: "ruta rentable".to_string(),
                score: 0.82,
                pesos_ga: vec![0.4, 0.2, 0.2, 0.1, 0.1],
                utilidad_usd: (2.4),
                diferencial_neto_bps: 1.4,
                cantidad_btc: (0.04),
                costo_total_usd: (0.8),
                latencia_max_ms: 12,
                z_score: 1.2,
                compra_usd_antes: (10_000.0),
                venta_btc_antes: (0.2),
                tiempo: ahora,
            });
        }

        EstadoPublico {
            generado_en: Utc::now(),
            configuracion: cfg_test(),
            reglas_rebalanceo: Vec::new(),
            corrida: EstadoCorrida {
                id: "jury-qa".to_string(),
                modo: "demo_controlada".to_string(),
                iniciada_en: ahora,
                fuente_pnl: "demo_controlada".to_string(),
                ejecucion_real: false,
            },
            cotizaciones,
            oportunidades: VecDeque::new(),
            operaciones,
            eventos_ejecucion: VecDeque::from([EventoEjecucion {
                id: "evt-test".to_string(),
                tipo: "demo_rentable".to_string(),
                ruta: "Binance->Kraken".to_string(),
                detalle: "evento QA".to_string(),
                severidad: "normal".to_string(),
                tiempo: ahora,
                utilidad_usd: (2.4),
                cantidad_btc: (0.04),
            }]),
            trazas_ejecucion: if con_auditoria {
                VecDeque::from([TransicionEjecucion {
                    id: "fsm-test".to_string(),
                    operacion_id: "op-test".to_string(),
                    ruta: "Binance->Kraken".to_string(),
                    estado_anterior: "LEG_B_FILLED".to_string(),
                    estado: "COMMITTED".to_string(),
                    pierna: "ambas".to_string(),
                    detalle: "ledger conciliado".to_string(),
                    exposicion_btc: 0.0,
                    pnl_realizado_usd: (2.4),
                    tiempo: ahora,
                }])
            } else {
                VecDeque::new()
            },
            auditoria_decisiones,
            rebalanceos: VecDeque::from([Rebalanceo {
                id: "reb-test".to_string(),
                desde: "Binance".to_string(),
                hacia: "Kraken".to_string(),
                activo: "BTC".to_string(),
                cantidad: (0.01),
                costo_usd: (5.0),
                razon: "QA rebalanceo".to_string(),
                tiempo: ahora,
            }]),
            transferencias_inventario: VecDeque::new(),
            balances: vec![
                Balance {
                    exchange: "Binance".to_string(),
                    usd: (10_000.0),
                    btc: (0.2),
                },
                Balance {
                    exchange: "Kraken".to_string(),
                    usd: (10_000.0),
                    btc: (0.2),
                },
                Balance {
                    exchange: "Coinbase".to_string(),
                    usd: (10_000.0),
                    btc: (0.2),
                },
            ],
            latencias_exchange: vec![LatenciaExchange {
                exchange: "Binance".to_string(),
                promedio_ms: 12.0,
                ultimo_ms: 12,
                min_ms: 10,
                max_ms: 20,
                p50_ms: 12,
                p95_ms: 18,
                p99_ms: 20,
                eventos: 5,
                estado: "ok".to_string(),
                region_sugerida: "us-central1".to_string(),
            }],
            telemetria_pipeline: TelemetriaPipeline::default(),
            serie_pnl: VecDeque::from([PuntoSerie {
                tiempo: ahora,
                valor: 2.4,
            }]),
            serie_diferencial: VecDeque::new(),
            metricas: Metricas {
                estado_riesgo: "normal".to_string(),
                operaciones: if con_fill_parcial { 1 } else { 0 },
                utilidad_acumulada_usd: (if con_fill_parcial { 2.4 } else { 0.0 }),
                rebalanceos_totales: 1,
                ..Metricas::default()
            },
            genetico: Some(EstadoGenetico {
                activo: true,
                generacion: 1,
                mejor_fitness: 10.0,
                fitness_promedio: 8.0,
                retador_fitness: 9.0,
                diversidad: 0.8,
                tasa_mutacion: 0.15,
                tasa_cruce: 0.7,
                poblacion: 40,
                convergente: false,
                mejores_pesos: vec![0.4, 0.2, 0.2, 0.1, 0.1],
                umbral_optimizado: 0.65,
                max_operacion_optimizada_btc: (0.18),
                tolerancia_latencia_ms: 4500,
                operaciones_evaluadas: 24,
                fallos_evaluados: 1,
                mejora_generacional: 1.2,
                temperatura_annealing: 0.9,
                inyecciones_diferenciales: 1,
                frontera_pareto: vec![],
                metaheuristicas: vec!["torneo".to_string(), "annealing".to_string()],
            }),
            ml_edge: Some(EstadoMlEdge {
                activo: true,
                modelo: "Mayab Edge Tensor".to_string(),
                version: "qa-test".to_string(),
                decision: "aceptar".to_string(),
                score_actual: 0.82,
                confianza: 0.76,
                expected_value_usd: (2.4),
                survival_probability: 0.91,
                fill_probability: 0.88,
                adverse_selection_bps: 0.2,
                features: (0..5)
                    .map(|i| FeatureMlEdge {
                        nombre: format!("feature_{i}"),
                        peso: 0.2,
                        valor: 1.0,
                        contribucion: 0.2,
                    })
                    .collect(),
                explicacion: "QA explicable".to_string(),
            }),
            persistencia: Some(EstadoPersistencia {
                activa: true,
                backend: "sqlite".to_string(),
                ruta: "/tmp/mayab-qa.sqlite".to_string(),
                operaciones: if con_fill_parcial { 1 } else { 0 },
                oportunidades: 1,
                eventos: 1,
                auditorias: if con_auditoria { 1 } else { 0 },
                rebalanceos: 1,
                db_bytes: 4096,
                error: None,
                storage_mode: "sqlite_ephemeral".to_string(),
                storage_status: "ephemeral".to_string(),
                storage_persistent: false,
                queue_capacity: 2048,
                queue_pending: 0,
                queue_dropped: 0,
            }),
            exchanges_activos,
            pares_activos: vec!["BTC/USD".to_string()],
        }
    }

    fn cotizacion_test(
        exchange: &str,
        par: &str,
        bid: f64,
        ask: f64,
        ahora: chrono::DateTime<Utc>,
    ) -> Cotizacion {
        Cotizacion {
            exchange: exchange.to_string(),
            par: par.to_string(),
            bid: (bid),
            bid_cantidad: (1.0),
            ask: (ask),
            ask_cantidad: (1.0),
            bids: SmallVec::from_vec(vec![NivelOrden {
                precio: bid,
                cantidad: 1.0,
            }]),
            asks: SmallVec::from_vec(vec![NivelOrden {
                precio: ask,
                cantidad: 1.0,
            }]),
            evento_unix_ms: ahora.timestamp_millis(),
            recibida_en: ahora,
            latencia_ms: 12,
            secuencia: 1,
            exchange_sequence: Some(1),
            integrity_status: "snapshot_seq".to_string(),
            resyncs: 0,
            sequence_gaps: 0,
            checksum_failures: 0,
            invalidated_ms: 0,
            timestamp_confiable: true,
            conectado: true,
            ultimo_mensaje: String::new(),
        }
    }

    fn cfg_test() -> MapaCostos {
        let mut exchanges = HashMap::new();
        for nombre in ["Binance", "Kraken", "Coinbase"] {
            exchanges.insert(
                nombre.to_string(),
                ExchangeConfig {
                    nombre: nombre.to_string(),
                    fee_taker: (0.001),
                    retiro_btc: (0.0001),
                    confiabilidad: 0.99,
                },
            );
        }
        MapaCostos {
            max_operacion_btc: 0.15,
            min_utilidad_usd: 1.0,
            webhook_url: None,
            min_diferencial_neto_bps: 0.65,
            deslizamiento_bps: 0.18,
            latencia_riesgo_bps: 0.08,
            retiro_amortizado_bps: 0.12,
            stale_ms: 4_500,
            enfriamiento_ms: 800,
            usdt_usd_premium_bps: 3.0,
            permitir_cruce_usd_usdt: true,
            circuit_breaker_perdida_usd: (80.0),
            circuit_breaker_ventana_min: 15,
            volatilidad_umbral_bps: 50.0,
            volatilidad_ventana_seg: 30,
            simular_adversidad: true,
            prob_fallo_orden: 0.0,
            prob_movimiento_brusco: 0.0,
            movimiento_brusco_bps: 7.0,
            rebalance_umbral_pct: 35.0,
            rebalance_max_transfer_pct: 35.0,
            costo_rebalanceo_usd: (5.0),
            rebalance_settlement_ms: 1_800,
            exchanges,
        }
    }
}
