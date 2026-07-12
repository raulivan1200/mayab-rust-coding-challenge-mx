use axum::Router;

use crate::{http::routes, server::EstadoApp};

/// Builds the API route tree in auditable, domain-oriented groups.
pub(crate) fn api_routes() -> Router<EstadoApp> {
    Router::new()
        .merge(routes::health::routes())
        .merge(routes::state::routes())
        .merge(routes::exports::routes())
        .merge(routes::admin::routes())
        .merge(routes::demo::routes())
        .merge(routes::ga::routes())
        .merge(routes::metrics::routes())
        .merge(routes::websocket::routes())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    use crate::{motor::Motor, types::MapaCostos};

    #[tokio::test]
    async fn route_groups_are_reachable_from_the_public_router() {
        let motor = Arc::new(Motor::new(
            MapaCostos::default(),
            250_000.0,
            2.5,
            "BTC/USD".into(),
            vec![],
            None,
        ));
        let app = crate::server::router(motor, None);

        for path in [
            "/healthz",
            "/api/version",
            "/api/estado",
            "/api/export/json",
            "/api/ga/estado",
            "/metrics",
        ] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_ne!(response.status(), StatusCode::NOT_FOUND, "missing {path}");
        }

        for path in ["/api/demo", "/api/exchanges", "/api/rebalance/rules"] {
            let response = app
                .clone()
                .oneshot(Request::post(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_ne!(response.status(), StatusCode::NOT_FOUND, "missing {path}");
        }
    }

    #[tokio::test]
    async fn admin_mutations_require_valid_bearer_while_reads_stay_public() {
        let motor = Arc::new(Motor::new(
            MapaCostos::default(),
            250_000.0,
            2.5,
            "BTC/USD".into(),
            vec![],
            None,
        ));
        let app = crate::server::router(motor, Some("test-admin-token".into()));
        let body = r#"{"escenario":"mercado_rentable"}"#;

        let missing = app
            .clone()
            .oneshot(
                Request::post("/api/demo")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let wrong = app
            .clone()
            .oneshot(
                Request::post("/api/demo")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer wrong")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::FORBIDDEN);

        let valid = app
            .clone()
            .oneshot(
                Request::post("/api/demo")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer test-admin-token")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(valid.status(), StatusCode::OK);

        let public = app
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(public.status(), StatusCode::OK);
    }
}
