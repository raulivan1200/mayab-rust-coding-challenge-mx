use crate::server::{self, EstadoApp};
use axum::{
    routing::{get, post},
    Router,
};

pub(crate) fn routes() -> Router<EstadoApp> {
    Router::new()
        .route("/healthz", get(server::healthz))
        .route("/api/healthz", get(server::healthz))
        .route("/readyz", get(server::readyz))
        .route("/api/readyz", get(server::readyz))
        .route("/api/version", get(server::version))
        .route(
            "/api/discord/interactions",
            post(server::discord_interactions),
        )
        .route("/api/mcp", get(server::mcp_manifest))
        .route("/api/mcp/manifest", get(server::mcp_manifest))
        .route("/api/mcp/call", post(server::mcp_call))
}
