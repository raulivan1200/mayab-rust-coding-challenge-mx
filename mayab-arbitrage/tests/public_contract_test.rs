//! Contratos de superficie pública.
//!
//! Cada endpoint tiene un caso independiente para que un rename, una ruta
//! desconectada o un panic quede identificado por nombre en CI. Estos tests no
//! afirman que un readiness limpio deba ser 200: 503 puede ser una respuesta
//! de dominio válida; 404, 405 y 500 nunca lo son para una lectura registrada.

use std::sync::Arc;

use axum::{body::Body, http::Request, Router};
use mayab_arbitrage::{motor::Motor, server, types::MapaCostos};
use tower::ServiceExt;

fn app() -> Router {
    let motor = Arc::new(Motor::new(
        MapaCostos::default(),
        250_000.0,
        2.5,
        "BTC/USD".into(),
        vec!["BTC/USDT".into()],
        None,
    ));
    server::router(motor, None)
}

async fn assert_public_get_contract(path: &str) {
    let response = app()
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    assert_ne!(status.as_u16(), 404, "ruta pública ausente: {path}");
    assert_ne!(status.as_u16(), 405, "método GET desconectado: {path}");
    assert!(
        status.as_u16() < 500 || status.as_u16() == 503,
        "panic/fallo interno en {path}: {status}"
    );
}

macro_rules! public_get_contract {
    ($name:ident, $path:literal) => {
        #[tokio::test]
        async fn $name() {
            assert_public_get_contract($path).await;
        }
    };
}

public_get_contract!(contract_healthz, "/healthz");
public_get_contract!(contract_healthz_alias, "/api/healthz");
public_get_contract!(contract_readyz, "/readyz");
public_get_contract!(contract_readyz_alias, "/api/readyz");
public_get_contract!(contract_version, "/api/version");
public_get_contract!(contract_mcp, "/api/mcp");
public_get_contract!(contract_mcp_manifest, "/api/mcp/manifest");
public_get_contract!(contract_estado, "/api/estado");
public_get_contract!(contract_jurado, "/api/jurado");
public_get_contract!(contract_preflight, "/api/preflight");
public_get_contract!(contract_resumen_llm, "/api/resumen-llm");
public_get_contract!(contract_latencias, "/api/latencias");
public_get_contract!(contract_backtest, "/api/backtest");
public_get_contract!(contract_research_tapes, "/api/research/tapes");
public_get_contract!(
    contract_research_microstructure,
    "/api/research/microstructure"
);
public_get_contract!(contract_research_impact, "/api/research/impact");
public_get_contract!(contract_research_economics, "/api/research/economics");
public_get_contract!(
    contract_research_execution_matrix,
    "/api/research/execution-matrix"
);
public_get_contract!(contract_research_bootstrap, "/api/research/bootstrap");
public_get_contract!(contract_research_walk_forward, "/api/research/walk-forward");
public_get_contract!(contract_research_ou, "/api/research/ou");
public_get_contract!(contract_research_ledger_audit, "/api/research/ledger-audit");
public_get_contract!(contract_live_readiness, "/api/readiness/live");
public_get_contract!(contract_lab_sweep, "/api/lab/sweep");
public_get_contract!(contract_export_json, "/api/export/json");
public_get_contract!(contract_export_csv, "/api/export/csv");
public_get_contract!(contract_export_evidence, "/api/export/evidence");
public_get_contract!(contract_ga_estado, "/api/ga/estado");
public_get_contract!(contract_ga_sensibilidad, "/api/ga/sensibilidad");
public_get_contract!(contract_ga_ablacion, "/api/ga/ablacion");
public_get_contract!(contract_metrics_prometheus, "/metrics");
public_get_contract!(contract_metrics_alias, "/api/metrics");
public_get_contract!(contract_paquete_evaluacion, "/api/paquete-evaluacion");
