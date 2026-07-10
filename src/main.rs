#![forbid(unsafe_code)]
//! Binario de Mayab Arbitraje BTC.
//!
//! Este proceso reúne feeds públicos de mercado, un motor de arbitraje simulado,
//! optimización genética, API Axum y dashboard estático. No firma órdenes reales,
//! no custodia fondos y no maneja secretos de exchanges.

mod config;
mod ga;
mod mercado;
mod motor;
mod persistencia;
mod server;
mod types;

use std::{net::SocketAddr, sync::Arc};

use anyhow::Context;
use tokio::signal;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let default_filter = if cfg!(debug_assertions) {
        "info"
    } else {
        "error"
    };
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter)))
        .with(tracing_subscriber::fmt::layer().json())
        .init();

    let cfg = config::Config::from_env();
    let persistencia = match persistencia::Persistencia::abrir(&cfg.auditoria_db_path) {
        Ok(persistencia) => {
            tracing::info!(ruta = %cfg.auditoria_db_path, "auditoria SQLite activa");
            Some(Arc::new(persistencia))
        }
        Err(err) => {
            tracing::warn!(ruta = %cfg.auditoria_db_path, error = %err, "auditoria SQLite desactivada");
            None
        }
    };
    let motor = Arc::new(motor::Motor::new(
        cfg.costos.clone(),
        cfg.capital_inicial_usd,
        cfg.balance_inicial_btc,
        cfg.par_base.clone(),
        persistencia,
    ));
    mercado::start_feeds(motor.clone(), cfg.par_base.clone()).await;
    motor.clone().start(cfg.intervalo_analisis).await;
    if cfg.demo_rentable_inicial {
        let ga = motor.evolucionar_ga(true, 96).await;
        let rentable = motor
            .activar_escenario_demo(motor::EscenarioDemo::MercadoRentable)
            .await;
        let fill_parcial = motor
            .activar_escenario_demo(motor::EscenarioDemo::FillParcial)
            .await;
        let rebalanceo = motor
            .activar_escenario_demo(motor::EscenarioDemo::Rebalanceo)
            .await;
        tracing::info!(
            ga = %ga,
            mercado_rentable = %rentable,
            fill_parcial = %fill_parcial,
            rebalanceo = %rebalanceo,
            "demo rentable inicial aplicada"
        );
    }

    let app = server::router(motor, cfg.token_admin.clone());
    let addr: SocketAddr = format!("0.0.0.0:{}", cfg.port)
        .parse()
        .context("puerto invalido")?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(url = %format!("http://localhost:{}", cfg.port), "servidor iniciado");

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
