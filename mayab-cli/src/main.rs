#![forbid(unsafe_code)]
//! Binario de Mayab Arbitraje BTC.
//!
//! Este proceso reúne feeds públicos de mercado, un motor de arbitraje simulado,
//! optimización genética, API Axum y dashboard estático. No firma órdenes reales,
//! no custodia fondos y no maneja secretos de exchanges.

use std::{net::SocketAddr, process::Command, sync::Arc};

use anyhow::Context;
use tokio::signal;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[cfg(feature = "timescaledb")]
use mayab_arbitrage::persistencia_timescale;
use mayab_arbitrage::{auditoria, config, discord, mercado, motor, persistencia, server};

fn abrir_dashboard_local(url: &str) {
    if !cfg!(debug_assertions)
        || std::env::var("MAYAB_OPEN_BROWSER").is_ok_and(|valor| valor == "0" || valor == "false")
    {
        return;
    }

    #[cfg(target_os = "macos")]
    let resultado = Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let resultado = Command::new("cmd").args(["/C", "start", "", url]).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let resultado = Command::new("xdg-open").arg(url).spawn();

    if let Err(error) = resultado {
        tracing::debug!(%error, %url, "no se pudo abrir el navegador automaticamente");
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    // Facilita desarrollo local; en Cloud Run las variables se inyectan en el entorno.
    let _ = dotenvy::dotenv();

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
    cfg.validate().context("configuracion insegura")?;

    // Fail-closed: require ADMIN_TOKEN in production
    if cfg.entorno == "production" && cfg.token_admin.is_none() {
        tracing::error!("ADMIN_TOKEN es requerido en entorno production");
        anyhow::bail!("ADMIN_TOKEN es requerido en entorno production. Configure ADMIN_TOKEN o use ENTORNO=development para desarrollo.");
    }

    let persistencia: Option<Arc<dyn auditoria::Auditoria>> =
        match persistencia::Persistencia::abrir(&cfg.auditoria_db_path) {
            Ok(persistencia) => {
                tracing::info!(ruta = %cfg.auditoria_db_path, "auditoria SQLite activa");
                Some(Arc::new(persistencia))
            }
            Err(err) => {
                tracing::warn!(ruta = %cfg.auditoria_db_path, error = %err, "auditoria SQLite desactivada");
                None
            }
        };
    #[cfg(feature = "timescaledb")]
    let persistencia: Option<Arc<dyn auditoria::Auditoria>> = {
        if let Ok(url) = std::env::var("DATABASE_URL") {
            match persistencia_timescale::TimescaleDbAuditoria::abrir(&url).await {
                Ok(ts) => {
                    tracing::info!(ruta = %url, "auditoria TimescaleDB activa");
                    Some(Arc::new(ts))
                }
                Err(err) => {
                    tracing::warn!(ruta = %url, error = %err, "auditoria TimescaleDB no disponible, usando SQLite");
                    persistencia
                }
            }
        } else {
            persistencia
        }
    };
    let persistencia = persistencia.map(|backend| {
        let capacidad = std::env::var("PERSISTENCE_QUEUE_CAPACITY")
            .ok()
            .and_then(|valor| valor.parse().ok())
            .unwrap_or(2048);
        Arc::new(auditoria::AuditoriaEnCola::nueva(backend, capacidad))
            as Arc<dyn auditoria::Auditoria>
    });
    let motor = Arc::new(motor::Motor::new(
        cfg.costos.clone(),
        cfg.capital_inicial_usd,
        cfg.balance_inicial_btc,
        cfg.par_base.clone(),
        cfg.pares_extra.clone(),
        persistencia,
    ));
    let estado = motor.estado().await;
    for par in &estado.pares_activos {
        mercado::start_feeds(motor.clone(), par.clone()).await;
    }
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

    let discord_config = discord::ConfigDiscord::from_env();
    discord_config.registrar_estado();
    if discord_config.habilitado() {
        tracing::info!("endpoint de Discord Interactions habilitado");
        tokio::spawn(discord::registrar_comandos(discord_config));
    }
    let app = server::router(motor, cfg.token_admin.clone());
    let addr: SocketAddr = format!("0.0.0.0:{}", cfg.port)
        .parse()
        .context("puerto invalido")?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let dashboard_url = format!("http://localhost:{}", cfg.port);
    tracing::info!(url = %dashboard_url, "servidor iniciado");
    abrir_dashboard_local(&dashboard_url);

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
