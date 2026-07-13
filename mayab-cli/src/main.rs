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

use mayab_arbitrage::auditoria::Auditoria as _;
#[cfg(feature = "timescaledb")]
use mayab_arbitrage::persistencia_timescale;
use mayab_arbitrage::{auditoria, config, discord, mercado, motor, persistencia, server};

async fn esperar_apagado() {
    #[cfg(unix)]
    {
        let mut terminate = signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("no se pudo instalar handler SIGTERM");
        tokio::select! {
            _ = signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = signal::ctrl_c().await;
    }
}

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
    cfg.validate().context("configuración insegura")?;

    let storage_mode = std::env::var("STORAGE_MODE")
        .unwrap_or_else(|_| "sqlite_ephemeral".to_string())
        .trim()
        .to_ascii_lowercase();
    if !matches!(
        storage_mode.as_str(),
        "sqlite_ephemeral" | "sqlite_persistent" | "volume" | "timescaledb"
    ) {
        anyhow::bail!("STORAGE_MODE no reconocido: {storage_mode}");
    }
    if cfg.entorno == config::Environment::Production && storage_mode != "timescaledb" {
        anyhow::bail!("production requiere STORAGE_MODE=timescaledb");
    }
    let persistencia: Option<Arc<dyn auditoria::Auditoria>> = if storage_mode == "timescaledb" {
        #[cfg(feature = "timescaledb")]
        {
            let url = std::env::var("DATABASE_URL")
                .context("STORAGE_MODE=timescaledb requiere DATABASE_URL")?;
            let ts = persistencia_timescale::TimescaleDbAuditoria::abrir(&url)
                .await
                .context("TimescaleDB requerido no está disponible")?;
            tracing::info!("auditoría TimescaleDB activa");
            Some(Arc::new(ts))
        }
        #[cfg(not(feature = "timescaledb"))]
        {
            anyhow::bail!(
                "STORAGE_MODE=timescaledb requiere compilar mayab-cli con --features timescaledb"
            );
        }
    } else {
        match persistencia::Persistencia::abrir(&cfg.auditoria_db_path) {
            Ok(persistencia) => {
                tracing::info!(ruta = %cfg.auditoria_db_path, "auditoría SQLite activa");
                Some(Arc::new(persistencia))
            }
            Err(err) => {
                tracing::warn!(ruta = %cfg.auditoria_db_path, error = %err, "auditoría SQLite desactivada");
                None
            }
        }
    };
    let persistencia_cola = persistencia.map(|backend| {
        let capacidad = std::env::var("PERSISTENCE_QUEUE_CAPACITY")
            .ok()
            .and_then(|valor| valor.parse().ok())
            .unwrap_or(2048);
        Arc::new(auditoria::AuditoriaEnCola::nueva(backend, capacidad))
    });
    let persistencia = persistencia_cola
        .clone()
        .map(|cola| cola as Arc<dyn auditoria::Auditoria>);
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
    if cfg.demo_rentable_inicial || cfg.judge_mode {
        let evidencia = server::preparar_evidencia_jurado(&motor).await;
        tracing::info!(
            judge_mode = cfg.judge_mode,
            evidencia = %evidencia,
            "evidencia inicial reproducible aplicada"
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
        .context("puerto inválido")?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let dashboard_url = format!("http://localhost:{}", cfg.port);
    tracing::info!(url = %dashboard_url, "servidor iniciado");
    abrir_dashboard_local(&dashboard_url);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(esperar_apagado())
    .await?;
    if let Some(cola) = persistencia_cola {
        let drenada =
            tokio::task::spawn_blocking(move || cola.flush(std::time::Duration::from_secs(10)))
                .await
                .unwrap_or(false);
        if !drenada {
            tracing::error!("la cola de persistencia no drenó sin pérdidas antes del shutdown");
        }
    }
    Ok(())
}
