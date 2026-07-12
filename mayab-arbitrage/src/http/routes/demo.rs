use crate::server::{self, EstadoApp};
use axum::{
    routing::{get, post},
    Router,
};

pub(crate) fn routes() -> Router<EstadoApp> {
    Router::new()
        .route("/api/config", post(server::actualizar_config_http))
        .route("/api/demo", post(server::demo_escenario))
        .route("/api/demo/caos", post(server::demo_caos_http))
        .route("/api/demo/final", post(server::demo_final_http))
        .route("/api/demo/reset", post(server::reset_demo_http))
        .route(
            "/api/demo/capturar/iniciar",
            post(server::captura_iniciar_http),
        )
        .route(
            "/api/demo/capturar/detener",
            post(server::captura_detener_http),
        )
        .route(
            "/api/demo/capturar/estado",
            get(server::captura_estado_http),
        )
        .route(
            "/api/demo/capturar/replay",
            post(server::captura_replay_http),
        )
        .route(
            "/api/replay/captura/iniciar",
            post(server::captura_iniciar_http),
        )
        .route(
            "/api/replay/captura/detener",
            post(server::captura_detener_http),
        )
        .route(
            "/api/replay/captura/estado",
            get(server::captura_estado_http),
        )
        .route("/api/replay/ejecutar", post(server::captura_replay_http))
        .route("/api/adverso", post(server::trigger_adverso_http))
}
