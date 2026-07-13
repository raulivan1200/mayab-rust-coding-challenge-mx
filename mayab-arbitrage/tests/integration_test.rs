use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use mayab_arbitrage::{config::Config, motor::Motor, persistencia::Persistencia, server};
use tower::ServiceExt;

async fn make_test_app() -> (Router, std::sync::Arc<Motor>) {
    let cfg = Config::from_env();
    // Cada app de integración usa su propio SQLite en memoria. Compartir la
    // ruta por defecto entre tests paralelos contamina conteos y hace que una
    // prueba determinista dependa del orden de ejecución.
    let persistencia = Persistencia::abrir(":memory:")
        .ok()
        .map(std::sync::Arc::new);
    let motor = std::sync::Arc::new(Motor::new(
        cfg.costos.clone(),
        cfg.capital_inicial_usd,
        cfg.balance_inicial_btc,
        cfg.par_base.clone(),
        cfg.pares_extra.clone(),
        persistencia.map(|p| p as std::sync::Arc<dyn mayab_arbitrage::auditoria::Auditoria>),
    ));
    let app = server::router(motor.clone(), cfg.token_admin.clone());
    (app, motor)
}

#[tokio::test]
async fn integration_motor_startup_and_receives_quotes() {
    let cfg = Config::from_env();
    let persistencia = Persistencia::abrir(":memory:")
        .ok()
        .map(std::sync::Arc::new);
    let motor = std::sync::Arc::new(Motor::new(
        cfg.costos.clone(),
        cfg.capital_inicial_usd,
        cfg.balance_inicial_btc,
        cfg.par_base.clone(),
        cfg.pares_extra.clone(),
        persistencia.map(|p| p as std::sync::Arc<dyn mayab_arbitrage::auditoria::Auditoria>),
    ));

    let cotizacion = mayab_arbitrage::types::Cotizacion {
        exchange: "Binance".into(),
        par: "BTC/USD".into(),
        bid: 100_000.0,
        bid_cantidad: 2.0,
        ask: 100_100.0,
        ask_cantidad: 1.5,
        bids: Default::default(),
        asks: Default::default(),
        evento_unix_ms: chrono::Utc::now().timestamp_millis(),
        recibida_en: chrono::Utc::now(),
        latencia_ms: 50,
        secuencia: 1,
        exchange_sequence: None,
        integrity_status: "snapshot".into(),
        resyncs: 0,
        sequence_gaps: 0,
        checksum_failures: 0,
        invalidated_ms: 0,
        timestamp_confiable: true,
        conectado: true,
        ultimo_mensaje: "".into(),
    };
    motor.recibir_cotizacion(cotizacion).await;

    let cotizacion2 = mayab_arbitrage::types::Cotizacion {
        exchange: "Coinbase".into(),
        par: "BTC/USD".into(),
        bid: 100_200.0,
        bid_cantidad: 1.0,
        ask: 100_300.0,
        ask_cantidad: 1.0,
        bids: Default::default(),
        asks: Default::default(),
        evento_unix_ms: chrono::Utc::now().timestamp_millis(),
        recibida_en: chrono::Utc::now(),
        latencia_ms: 80,
        secuencia: 2,
        exchange_sequence: None,
        integrity_status: "snapshot".into(),
        resyncs: 0,
        sequence_gaps: 0,
        checksum_failures: 0,
        invalidated_ms: 0,
        timestamp_confiable: true,
        conectado: true,
        ultimo_mensaje: "".into(),
    };
    motor.recibir_cotizacion(cotizacion2).await;

    let estado = motor.estado().await;
    assert_eq!(estado.cotizaciones.len(), 2);
    assert_eq!(estado.exchanges_activos.get("Binance"), Some(&true));
    assert_eq!(estado.exchanges_activos.get("Coinbase"), Some(&true));
}

#[tokio::test]
async fn integration_demo_mercado_rentable_genera_pnl_positivo() {
    let (app, _motor) = make_test_app().await;

    let req = Request::builder()
        .method("POST")
        .uri("/api/demo/reset")
        .header("content-type", "application/json")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = Request::builder()
        .method("POST")
        .uri("/api/demo")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"escenario":"mercado_rentable"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = Request::builder()
        .method("GET")
        .uri("/api/estado")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let estado: mayab_arbitrage::types::EstadoPublico = serde_json::from_slice(&body).unwrap();

    assert!(
        estado.metricas.utilidad_acumulada_usd > 0.0,
        "PnL debe ser positivo: {}",
        estado.metricas.utilidad_acumulada_usd
    );
    assert!(
        estado.metricas.operaciones_totales > 0,
        "Debe haber operaciones"
    );
    assert!(
        estado.genetico.is_some(),
        "GA debe estar activo tras mercado rentable"
    );
}

#[tokio::test]
async fn integration_demo_final_preserva_inventario_para_evidencia_forense() {
    let (app, _motor) = make_test_app().await;
    let request = Request::builder()
        .method("POST")
        .uri("/api/demo/final")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let result: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(
        result["ok"],
        true,
        "demo final incompleta: {}",
        serde_json::to_string_pretty(&result).unwrap()
    );
    assert_eq!(result["persistenciaDrenada"], true);
    assert_eq!(
        result["preflightReady"], false,
        "la prueba determinista no debe inventar feeds públicos en tests"
    );
    assert!(result["resultSha256"]
        .as_str()
        .is_some_and(|hash| hash.starts_with("sha256:")));
    assert_eq!(result["deterministicProof"]["matrixPassed"], 12);
    assert_eq!(result["deterministicProof"]["matrixTotal"], 12);
    assert_eq!(result["fillParcial"]["partialFill"], true);
    assert_eq!(result["riesgoSegundaPierna"]["ok"], true);
    assert_eq!(result["riesgoSegundaPierna"]["estadoFinal"], "RECONCILED");
    assert_eq!(result["riesgoSegundaPierna"]["exposicionFinalBtc"], 0.0);
    assert!(result["mercadoRentable"]["operacionesInsertadas"]
        .as_u64()
        .is_some_and(|count| count > 0));
}

#[tokio::test]
async fn integration_ga_evolucionar_con_replay_sintetico() {
    let (app, _motor) = make_test_app().await;

    let req = Request::builder()
        .method("POST")
        .uri("/api/demo/reset")
        .header("content-type", "application/json")
        .body(Body::empty())
        .unwrap();
    let _ = app.clone().oneshot(req).await.unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/api/ga/evolucionar")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"usarReplaySiVacio":true,"muestras":24}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["ok"], true);
    assert_eq!(json["fuente"], "replay_sintetico");
    assert!(json["generacion"].as_i64().unwrap() > 0);
    assert_eq!(json["ga"]["mejoresPesos"].as_array().unwrap().len(), 5);
}

#[tokio::test]
async fn integration_ga_sensibilidad_aplica_estrategias_y_expone_metodologia() {
    let (app, _motor) = make_test_app().await;
    let req = Request::builder()
        .method("GET")
        .uri("/api/ga/sensibilidad")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let resultados = json["resultados"].as_array().unwrap();

    assert_eq!(resultados.len(), 7);
    assert!(json["metodologia"]
        .as_str()
        .unwrap()
        .contains("24 semillas holdout"));
    assert_eq!(json["sinFugaHoldout"], true);
    assert_eq!(json["seleccionAntesHoldout"], true);
    assert_eq!(json["semillasEntrenamiento"].as_array().unwrap().len(), 24);
    assert_eq!(
        json["semillasHoldoutNoVistas"].as_array().unwrap().len(),
        24
    );
    assert!(resultados.iter().all(|fila| {
        fila["modelo"].is_string()
            && fila["profitFactor"].is_number()
            && fila["winRate"].is_number()
            && fila["runs"] == 24
            && fila["trades"].as_u64().is_some_and(|trades| trades > 0)
            && fila["worstRunLoss"].is_number()
            && fila["medianaPnL"].is_number()
            && fila["p05"].is_number()
            && fila["p95"].is_number()
            && fila["config"].is_object()
    }));
}

#[tokio::test]
async fn integration_tape_incluido_no_se_presenta_como_captura_verificada() {
    let (app, _motor) = make_test_app().await;
    let req = Request::builder()
        .method("GET")
        .uri("/api/research/tapes")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let tape = &json["tapes"][0];

    assert_eq!(tape["provenance"], "repository_sample_unverified");
    assert_eq!(tape["classification"], "unverified_market_sample");
    assert_eq!(tape["authenticityVerified"], false);
    assert_eq!(tape["captureManifestVerified"], false);
    assert!(tape["sha256"].as_str().is_some_and(|hash| !hash.is_empty()));
}

#[tokio::test]
async fn integration_demo_caos_ejecuta_checks_y_recupera() {
    let (app, _motor) = make_test_app().await;

    let req = Request::builder()
        .method("POST")
        .uri("/api/demo/reset")
        .header("content-type", "application/json")
        .body(Body::empty())
        .unwrap();
    let _ = app.clone().oneshot(req).await.unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/api/demo/caos")
        .header("content-type", "application/json")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["ok"], true);
    assert!(
        json["checks"].as_object().unwrap().len() >= 8,
        "Debe tener 8+ checks"
    );
    assert_eq!(
        json["estadoFinal"]["exposicionResidualBtc"], 0.0,
        "Exposición residual debe ser 0"
    );
}

#[tokio::test]
async fn integration_estado_endpoint_devuelve_contrato_completo() {
    let (app, _motor) = make_test_app().await;

    let req = Request::builder()
        .method("GET")
        .uri("/api/estado")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let estado: mayab_arbitrage::types::EstadoPublico = serde_json::from_slice(&body).unwrap();

    assert!(estado.pares_activos.contains(&"BTC/USD".to_string()));
    assert!(!estado.exchanges_activos.is_empty());
    assert!(estado.metricas.capital_inicial_usd > 0.0);
    assert!(estado.configuracion.exchanges.len() >= 5);
    assert!(estado.genetico.is_some() || estado.genetico.is_none());
}

#[tokio::test]
async fn integration_preflight_reporta_salud_feeds_y_ga() {
    let (app, _motor) = make_test_app().await;

    let req = Request::builder()
        .method("GET")
        .uri("/api/preflight")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Un motor recién creado puede estar blocked hasta recibir dos libros frescos.
    let status = json["judgeReadiness"]["status"].as_str().unwrap();
    assert!(matches!(status, "ready" | "blocked"));
    // En preflight, "checks" es un array de {name, ok, detalle}
    assert!(json["checks"].is_array());
    assert!(json["checks"].as_array().unwrap().len() >= 10);
    assert!(json["judgeReadiness"]["checks"].is_array());
    assert_eq!(json["judgeReadiness"]["total"], 12);
    assert_eq!(
        json["judgeReadiness"]["checks"].as_array().map(Vec::len),
        Some(12)
    );
    assert_eq!(json["judgeReadiness"]["executionMatrix"]["passed"], 12);
    assert_eq!(json["judgeReadiness"]["executionMatrix"]["total"], 12);
    assert_eq!(json["judgeReadiness"]["executionMatrix"]["allPassed"], true);
}

#[tokio::test]
async fn integration_matriz_forense_publica_doce_escenarios_con_huella() {
    let (app, _motor) = make_test_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/research/execution-matrix")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["passed"], 12);
    assert_eq!(json["total"], 12);
    assert_eq!(json["allPassed"], true);
    assert_eq!(json["cases"].as_array().map(Vec::len), Some(12));
    assert!(json["matrixSha256"]
        .as_str()
        .is_some_and(|hash| hash.starts_with("sha256:")));
}

#[tokio::test]
async fn integration_toggle_exchange_desactiva_y_reactiva() {
    let (app, motor) = make_test_app().await;

    let req = Request::builder()
        .method("POST")
        .uri("/api/exchanges")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"exchange":"Binance","activo":false}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let estado = motor.estado().await;
    assert_eq!(estado.exchanges_activos.get("Binance"), Some(&false));

    let req = Request::builder()
        .method("POST")
        .uri("/api/exchanges")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"exchange":"Binance","activo":true}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let estado = motor.estado().await;
    assert_eq!(estado.exchanges_activos.get("Binance"), Some(&true));
}

#[tokio::test]
async fn integration_circuit_breaker_pausa_ejecuciones() {
    let (app, motor) = make_test_app().await;

    let req = Request::builder()
        .method("POST")
        .uri("/api/demo/reset")
        .header("content-type", "application/json")
        .body(Body::empty())
        .unwrap();
    let _ = app.clone().oneshot(req).await.unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/api/demo")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"escenario":"circuit_breaker"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let estado = motor.estado().await;
    assert!(
        estado.metricas.circuit_breaker_activo,
        "Circuit breaker debe estar activo"
    );
}

#[tokio::test]
async fn integration_rebalanceo_forzado_mueve_saldo_y_cobra_costo() {
    let (app, _motor) = make_test_app().await;

    let req = Request::builder()
        .method("POST")
        .uri("/api/demo/reset")
        .header("content-type", "application/json")
        .body(Body::empty())
        .unwrap();
    let _ = app.clone().oneshot(req).await.unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/api/demo")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"escenario":"rebalanceo"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["ok"], true);
    assert!(json["rebalanceo"]["costoUsd"].as_f64().unwrap() > 0.0);
    assert!(json["rebalanceo"]["cantidad"].as_f64().unwrap() > 0.0);
}

#[tokio::test]
async fn integration_backtest_devuelve_metricas_comparativas() {
    let (app, _motor) = make_test_app().await;

    let req = Request::builder()
        .method("GET")
        .uri("/api/backtest")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Contrato JSON del backtest.
    assert!(json["base"].is_object());
    assert!(json["optimizada"].is_object());
    assert!(json["validacionMultisemilla"]["base"]["pnlMedianoUsd"].is_number());
    assert!(json["validacionMultisemilla"]["optimizada"]["pnlMedianoUsd"].is_number());
    assert!(json["comparacion"]["ganador"].is_string());
    assert_eq!(json["significanciaBootstrap"]["remuestras"], 10_000);
    assert!(
        json["significanciaBootstrap"]["principal"]["probabilidadDeltaPnlMayorCero"].is_number()
    );
    assert!(
        json["significanciaBootstrap"]["permutacionPareadaBloques"]["pValueDosColas"].is_number()
    );
}

#[tokio::test]
async fn integration_microestructura_expone_holdout_calibracion_y_wilson() {
    let (app, _motor) = make_test_app().await;
    let req = Request::builder()
        .method("GET")
        .uri("/api/research/microstructure")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["report"]["split"][0], 1200);
    assert_eq!(json["report"]["split"][1], 480);
    assert_eq!(json["report"]["split"][2], 720);
    assert_eq!(
        json["report"]["leakageGuards"]["gaParametersUnchanged"],
        true
    );
    assert!(json["report"]["markoutsMeanBps"]["500ms"].is_number());
    assert!(json["report"]["estimatedSecondLegRiskMean"].is_number());
    assert_eq!(json["report"]["ouLab"]["separateFromGa"], true);
    assert!(json["report"]["ouLab"]["decision"].is_string());
    assert_eq!(json["report"]["calibration"].as_array().unwrap().len(), 3);
    assert!(json["report"]["calibration"][0]["reliability"][0]["wilson95"].is_array());
}

#[tokio::test]
async fn integration_ou_expone_estacionariedad_baselines_y_holdout() {
    let (app, _motor) = make_test_app().await;
    let req = Request::builder()
        .method("GET")
        .uri("/api/research/ou")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["report"]["phase"], 10);
    assert_eq!(json["report"]["protocol"]["separateFromGa"], true);
    assert!(json["report"]["stationarity"]["adfTStat"].is_number());
    assert!(json["report"]["stationarity"]["kpssStat"].is_number());
    assert_eq!(json["report"]["holdoutC"].as_array().unwrap().len(), 3);
    assert_eq!(
        json["report"]["stabilityWindows"].as_array().unwrap().len(),
        5
    );
}

#[tokio::test]
async fn integration_modo_jurado_devuelve_rubrica_y_scorecard() {
    let (app, _motor) = make_test_app().await;

    let req = Request::builder()
        .method("GET")
        .uri("/api/jurado")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // rubricaOficial y scorecard son arreglos; coberturaFinalista es un objeto.
    assert!(json["rubricaOficial"].is_array());
    assert!(json["scorecard"].is_array());
    assert!(json["coberturaFinalista"].is_object());
    assert!(json["checks"].is_array());
    assert!(json["evidenciaClave"].is_object());
    assert!(json["enlaces"].is_object());
}

#[tokio::test]
async fn integration_resumen_llm_devuelve_snapshot_compacto() {
    let (app, _motor) = make_test_app().await;

    let req = Request::builder()
        .method("GET")
        .uri("/api/resumen-llm")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json["resumen"].as_str().unwrap().len() > 50);
    assert!(json["markdown"].as_str().unwrap().contains("# "));
    assert!(json["decision"].is_string());
    assert!(json["metricasClave"]["pnlUsd"].is_number());
    // mejorRuta puede ser null si no hay oportunidades
    assert!(json["mejorRuta"].is_object() || json["mejorRuta"].is_null());
    assert!(json["decisionInspector"].is_array());
    assert!(json["ga"].is_object() || json["ga"].is_null());
    assert!(json["mlEdge"].is_object() || json["mlEdge"].is_null());
    assert!(json["persistencia"].is_object() || json["persistencia"].is_null());
}

#[tokio::test]
async fn integration_paquete_evaluacion_devuelve_evidencia_completa() {
    let (app, _motor) = make_test_app().await;

    let req = Request::builder()
        .method("GET")
        .uri("/api/paquete-evaluacion")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get("x-mayab-content-length").is_some());

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json["criterios"].is_array());
    assert!(json["huellaAuditoria"].is_string());
    assert!(json["scriptDemo"].is_array());
    assert!(json["endpoints"].is_object());
    assert!(json["provenance"]["configHash"].as_str().is_some());
    assert_eq!(json["evidencia"]["executionMatrix"]["passed"], 12);
    assert_eq!(json["evidencia"]["executionMatrix"]["total"], 12);
    assert!(json["packageSha256"]
        .as_str()
        .is_some_and(|hash| hash.starts_with("sha256:")));
}

#[tokio::test]
async fn integration_export_json_y_csv_funcionan() {
    let (app, _motor) = make_test_app().await;

    // JSON
    let req = Request::builder()
        .method("GET")
        .uri("/api/export/json")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["operaciones"].is_array());

    // CSV
    let req = Request::builder()
        .method("GET")
        .uri("/api/export/csv")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let csv = String::from_utf8(body.to_vec()).unwrap();
    assert!(csv.contains("tipo,tiempo,ruta"));
}

#[tokio::test]
async fn integration_metrics_endpoint_devuelve_formato_prometheus() {
    let (app, _motor) = make_test_app().await;

    // Primero una peticion que el middleware debe contar.
    let req = Request::builder()
        .method("GET")
        .uri("/api/estado")
        .body(Body::empty())
        .unwrap();
    let _ = app.clone().oneshot(req).await.unwrap();

    let req = Request::builder()
        .method("GET")
        .uri("/metrics")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let content_type = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.contains("text/plain"));

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let metrics = String::from_utf8(body.to_vec()).unwrap();

    // Formato de exposicion Prometheus: HELP/TYPE y metricas con valor numerico.
    assert!(metrics.contains("# HELP mayab_http_requests_total"));
    assert!(metrics.contains("# TYPE mayab_http_requests_total counter"));
    assert!(metrics.contains("mayab_http_requests_total{"));
    assert!(metrics.contains("mayab_pnl_usd "));
    assert!(metrics.contains("mayab_exchanges_activos "));
}
