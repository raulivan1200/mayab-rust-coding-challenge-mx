use crate::server::{self, EstadoApp};
use axum::{routing::get, Router};

pub(crate) fn routes() -> Router<EstadoApp> {
    Router::new()
        .route("/operator", get(server::operator))
        .route("/api/estado", get(server::estado))
        .route("/api/jurado", get(server::jurado))
        .route("/api/preflight", get(server::preflight))
        .route("/api/resumen-llm", get(server::resumen_llm))
        .route("/api/latencias", get(server::latencias))
        .route("/api/backtest", get(server::backtest))
        .route("/api/research/tapes", get(server::research_tapes))
        .route(
            "/api/research/walk-forward",
            get(server::research_walk_forward),
        )
        .route("/api/research/impact", get(server::research_impact))
        .route("/api/research/economics", get(server::research_economics))
        .route(
            "/api/research/execution-matrix",
            get(server::research_execution_matrix),
        )
        .route("/api/research/bootstrap", get(server::research_bootstrap))
        .route(
            "/api/research/microstructure",
            get(server::research_microstructure),
        )
        .route("/api/research/ou", get(server::research_ou))
        .route(
            "/api/research/ledger-audit",
            get(server::research_ledger_audit),
        )
        .route("/api/readiness/live", get(server::readiness_live))
        .route("/api/lab/sweep", get(server::lab_sweep))
}
