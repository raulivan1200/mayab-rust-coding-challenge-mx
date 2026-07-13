//! Backend de auditoría TimescaleDB/Postgres (opt-in, feature `timescaledb`).
//!
//! Implementa el mismo contrato [`crate::auditoria::Auditoria`] que la
//! persistencia SQLite local, pero sobre hypertables de TimescaleDB. Se activa
//! con `cargo build --features timescaledb` y requiere `DATABASE_URL` apuntando
//! a una instancia TimescaleDB con el esquema de `scripts/timescaledb/schema.sql`.
//!
//! El motor no cambia: basta intercambiar la implementación de `Auditoria`.

use anyhow::Context;
use chrono::Utc;
use serde_json::json;
use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::Mutex;
use tokio_postgres::{config::SslMode, Client, Config as PgConfig, NoTls, Row};
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::auditoria::Auditoria;
use crate::execution::ExecutionReport;
use crate::types::{
    AuditoriaDecision, EstadoPersistencia, EventoEjecucion, Operacion, Oportunidad, Rebalanceo,
};

pub struct TimescaleDbAuditoria {
    cliente: Mutex<Client>,
    runtime: tokio::runtime::Handle,
    operaciones: AtomicUsize,
    oportunidades: AtomicUsize,
    auditorias: AtomicUsize,
    eventos: AtomicUsize,
    rebalanceos: AtomicUsize,
    ejecuciones: AtomicUsize,
    health_ok: Arc<AtomicBool>,
}

impl TimescaleDbAuditoria {
    /// Conecta a TimescaleDB y verifica el esquema.
    pub async fn abrir(url: &str) -> anyhow::Result<Self> {
        let mut config: PgConfig = url
            .parse()
            .context("DATABASE_URL de TimescaleDB inválida")?;
        let allow_insecure = std::env::var("ALLOW_INSECURE_DATABASE").ok();
        aplicar_politica_tls(
            &mut config,
            valor_habilita_base_insegura(allow_insecure.as_deref()),
        )?;
        if config
            .get_connect_timeout()
            .map(|timeout| *timeout > Duration::from_secs(5))
            .unwrap_or(true)
        {
            config.connect_timeout(Duration::from_secs(5));
        }
        let bounded_options = match config.get_options().map(str::trim) {
            Some(existing) if !existing.is_empty() => {
                format!("{existing} -c statement_timeout=5000 -c lock_timeout=5000")
            }
            _ => "-c statement_timeout=5000 -c lock_timeout=5000".to_string(),
        };
        config.options(bounded_options);
        let cliente = conectar_timescale(&config, "auditoria").await?;
        cliente
            .batch_execute(
                "SELECT 1 FROM operaciones LIMIT 1;
                 SELECT 1 FROM auditorias LIMIT 1;
                 SELECT 1 FROM ejecuciones LIMIT 1;
                 SELECT 1 FROM audit_idempotency_keys LIMIT 1;",
            )
            .await
            .context("el esquema TimescaleDB no está inicializado")?;
        let operaciones = contar_tabla(&cliente, "operaciones").await?;
        let oportunidades = contar_tabla(&cliente, "oportunidades").await?;
        let auditorias = contar_tabla(&cliente, "auditorias").await?;
        let eventos = contar_tabla(&cliente, "eventos").await?;
        let rebalanceos = contar_tabla(&cliente, "rebalanceos").await?;
        let ejecuciones = contar_tabla(&cliente, "ejecuciones").await?;
        let cliente_salud = conectar_timescale(&config, "health-probe").await?;
        let health_ok = Arc::new(AtomicBool::new(true));
        iniciar_sondeo_salud(cliente_salud, &health_ok);
        Ok(Self {
            cliente: Mutex::new(cliente),
            runtime: tokio::runtime::Handle::current(),
            operaciones,
            oportunidades,
            auditorias,
            eventos,
            rebalanceos,
            ejecuciones,
            health_ok,
        })
    }

    fn block_on<F: Future>(&self, future: F) -> F::Output {
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| self.runtime.block_on(future))
        } else {
            self.runtime.block_on(future)
        }
    }
}

fn valor_habilita_base_insegura(valor: Option<&str>) -> bool {
    valor == Some("true")
}

fn aplicar_politica_tls(config: &mut PgConfig, allow_insecure: bool) -> anyhow::Result<()> {
    match config.get_ssl_mode() {
        SslMode::Disable if !allow_insecure => anyhow::bail!(
            "sslmode=disable requiere ALLOW_INSECURE_DATABASE=true; use TLS en producción"
        ),
        SslMode::Disable | SslMode::Require => Ok(()),
        SslMode::Prefer => {
            config.ssl_mode(SslMode::Require);
            Ok(())
        }
        _ => {
            config.ssl_mode(SslMode::Require);
            Ok(())
        }
    }
}

async fn conectar_timescale(config: &PgConfig, proposito: &'static str) -> anyhow::Result<Client> {
    if config.get_ssl_mode() == SslMode::Disable {
        let (cliente, conexion) = config
            .connect(NoTls)
            .await
            .with_context(|| format!("no se pudo conectar a TimescaleDB sin TLS ({proposito})"))?;
        tokio::spawn(async move {
            if let Err(error) = conexion.await {
                tracing::warn!(%error, proposito, "conexión TimescaleDB cerrada");
            }
        });
        return Ok(cliente);
    }

    let roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let (cliente, conexion) = config
        .connect(MakeRustlsConnect::new(tls_config))
        .await
        .with_context(|| format!("no se pudo conectar a TimescaleDB con TLS ({proposito})"))?;
    tokio::spawn(async move {
        if let Err(error) = conexion.await {
            tracing::warn!(%error, proposito, "conexión TLS de TimescaleDB cerrada");
        }
    });
    Ok(cliente)
}

fn iniciar_sondeo_salud(cliente: Client, health_ok: &Arc<AtomicBool>) {
    let health_ok = Arc::downgrade(health_ok);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let Some(health_ok) = health_ok.upgrade() else {
                break;
            };
            let resultado =
                tokio::time::timeout(Duration::from_secs(2), cliente.simple_query("SELECT 1"))
                    .await;
            health_ok.store(matches!(resultado, Ok(Ok(_))), Ordering::Release);
        }
    });
}

async fn contar_tabla(c: &Client, tabla: &str) -> anyhow::Result<AtomicUsize> {
    let total = c
        .query_one(&format!("SELECT COUNT(*) FROM {tabla}"), &[])
        .await
        .with_context(|| format!("no se pudo contar la tabla requerida {tabla}"))?
        .get::<_, i64>(0)
        .max(0) as usize;
    Ok(AtomicUsize::new(total))
}

async fn contar_tabla_valor(c: &Client, tabla: &str) -> anyhow::Result<i64> {
    c.query_one(&format!("SELECT COUNT(*) FROM {tabla}"), &[])
        .await
        .with_context(|| format!("no se pudo contar la tabla {tabla}"))
        .map(|r| r.get::<_, i64>(0))
}

fn fila_operacion(fila: &Row) -> Option<Operacion> {
    let raw: String = match fila.try_get("payload_json") {
        Ok(raw) => raw,
        Err(error) => {
            tracing::warn!(%error, "fila de operación inválida en TimescaleDB");
            return None;
        }
    };
    match serde_json::from_str(&raw) {
        Ok(operacion) => Some(operacion),
        Err(error) => {
            tracing::warn!(%error, "payload de operación inválido en TimescaleDB");
            None
        }
    }
}

impl Auditoria for TimescaleDbAuditoria {
    fn registrar_operacion(&self, op: &Operacion) -> anyhow::Result<()> {
        let payload = json!(op).to_string();
        let (id, tiempo, compra, venta, par, cantidad, utilidad, costo, parcial) = (
            op.id.clone(),
            op.ejecutada_en.clone(),
            op.compra_en.clone(),
            op.venta_en.clone(),
            op.par.clone(),
            op.cantidad_btc,
            op.utilidad_usd,
            op.costos.total_usd,
            op.parcial,
        );
        let changed = self.block_on(async move {
            let c = self.cliente.lock().await;
            Ok::<_, anyhow::Error>(c.execute(
                "WITH claimed AS (
                     INSERT INTO audit_idempotency_keys (kind, id) VALUES ('operacion', $1)
                     ON CONFLICT DO NOTHING RETURNING 1
                 )
                 INSERT INTO operaciones (tiempo, id, compra_en, venta_en, par, cantidad_btc, utilidad_usd, costo_usd, score, partial_fill, payload_json)
                 SELECT $2, $1, $3, $4, $5, $6, $7, $8, $9, $10, $11::jsonb FROM claimed",
                &[&id, &tiempo, &compra, &venta, &par, &cantidad, &utilidad, &costo, &None::<f64>, &parcial, &payload],
            )
            .await?)
        })?;
        self.operaciones
            .fetch_add(changed as usize, Ordering::Relaxed);
        Ok(())
    }

    fn registrar_evento(&self, evento: &EventoEjecucion) -> anyhow::Result<()> {
        let payload = json!(evento).to_string();
        let (id, tiempo, tipo, severidad, detalle) = (
            evento.id.clone(),
            evento.tiempo.clone(),
            evento.tipo.clone(),
            evento.severidad.clone(),
            evento.detalle.clone(),
        );
        let changed = self.block_on(async move {
            let c = self.cliente.lock().await;
            Ok::<_, anyhow::Error>(
                c.execute(
                    "WITH claimed AS (
                     INSERT INTO audit_idempotency_keys (kind, id) VALUES ('evento', $1)
                     ON CONFLICT DO NOTHING RETURNING 1
                 )
                 INSERT INTO eventos (tiempo, id, tipo, severidad, mensaje, payload_json)
                 SELECT $2, $1, $3, $4, $5, $6::jsonb FROM claimed",
                    &[&id, &tiempo, &tipo, &severidad, &detalle, &payload],
                )
                .await?,
            )
        })?;
        self.eventos.fetch_add(changed as usize, Ordering::Relaxed);
        Ok(())
    }

    fn registrar_rebalanceo(&self, r: &Rebalanceo) -> anyhow::Result<()> {
        let payload = json!(r).to_string();
        let (id, tiempo, desde, hacia, cantidad, costo) = (
            r.id.clone(),
            r.tiempo.clone(),
            r.desde.clone(),
            r.hacia.clone(),
            r.cantidad,
            r.costo_usd,
        );
        let changed = self.block_on(async move {
            let c = self.cliente.lock().await;
            Ok::<_, anyhow::Error>(c.execute(
                "WITH claimed AS (
                     INSERT INTO audit_idempotency_keys (kind, id) VALUES ('rebalanceo', $1)
                     ON CONFLICT DO NOTHING RETURNING 1
                 )
                 INSERT INTO rebalanceos (tiempo, id, desde, hacia, cantidad, costo_usd, payload_json)
                 SELECT $2, $1, $3, $4, $5, $6, $7::jsonb FROM claimed",
                &[&id, &tiempo, &desde, &hacia, &cantidad, &costo, &payload],
            )
            .await?)
        })?;
        self.rebalanceos
            .fetch_add(changed as usize, Ordering::Relaxed);
        Ok(())
    }

    fn registrar_oportunidades(&self, oportunidades: &[Oportunidad]) -> anyhow::Result<()> {
        for o in oportunidades {
            let payload = json!(o).to_string();
            let (id, tiempo, compra, venta, utilidad, diff, payload) = (
                o.id.clone(),
                o.detectada_en.clone(),
                o.compra_en.clone(),
                o.venta_en.clone(),
                o.utilidad_usd,
                o.diferencial_neto_bps,
                payload,
            );
            let ruta = format!("{compra}->{venta}");
            let changed = self.block_on(async move {
                let c = self.cliente.lock().await;
                Ok::<_, anyhow::Error>(c.execute(
                    "WITH claimed AS (
                         INSERT INTO audit_idempotency_keys (kind, id) VALUES ('oportunidad', $1)
                         ON CONFLICT DO NOTHING RETURNING 1
                     )
                     INSERT INTO oportunidades (tiempo, id, ruta, utilidad_usd, diferencial, payload_json)
                     SELECT $2, $1, $3, $4, $5, $6::jsonb FROM claimed",
                    &[&id, &tiempo, &ruta, &utilidad, &diff, &payload],
                )
                .await?)
            })?;
            self.oportunidades
                .fetch_add(changed as usize, Ordering::Relaxed);
        }
        Ok(())
    }

    fn registrar_auditorias(&self, auditorias: &[AuditoriaDecision]) -> anyhow::Result<()> {
        for a in auditorias {
            let payload = json!(a).to_string();
            let (id, tiempo, ruta, decision, score, utilidad, razon, payload) = (
                a.id.clone(),
                a.tiempo.clone(),
                a.ruta.clone(),
                a.decision_code.clone(),
                a.score,
                a.utilidad_usd,
                a.decision_reason.clone(),
                payload,
            );
            let changed = self.block_on(async move {
                let c = self.cliente.lock().await;
                Ok::<_, anyhow::Error>(c.execute(
                    "WITH claimed AS (
                         INSERT INTO audit_idempotency_keys (kind, id) VALUES ('auditoria', $1)
                         ON CONFLICT DO NOTHING RETURNING 1
                     )
                     INSERT INTO auditorias (tiempo, id, ruta, decision, score, utilidad_usd, razon, payload_json)
                     SELECT $2, $1, $3, $4, $5, $6, $7, $8::jsonb FROM claimed",
                    &[&id, &tiempo, &ruta, &decision, &score, &utilidad, &razon, &payload],
                )
                .await?)
            })?;
            self.auditorias
                .fetch_add(changed as usize, Ordering::Relaxed);
        }
        Ok(())
    }

    fn registrar_ejecucion(&self, execution: &ExecutionReport) -> anyhow::Result<()> {
        let payload = json!(execution).to_string();
        let id = execution.execution_id.clone();
        let scenario = serde_json::to_string(&execution.scenario)?;
        let state = serde_json::to_string(&execution.state)?;
        let pnl = execution.pnl_usd.to_string();
        let tiempo = Utc::now();
        let changed = self.block_on(async move {
            let c = self.cliente.lock().await;
            Ok::<_, anyhow::Error>(
                c.execute(
                    "WITH claimed AS (
                     INSERT INTO audit_idempotency_keys (kind, id) VALUES ('ejecucion', $1)
                     ON CONFLICT DO NOTHING RETURNING 1
                 )
                 INSERT INTO ejecuciones (tiempo, id, escenario, estado, pnl_usd, payload_json)
                 SELECT $2, $1, $3, $4, $5, $6::jsonb FROM claimed
                 ON CONFLICT (id) DO NOTHING",
                    &[&id, &tiempo, &scenario, &state, &pnl, &payload],
                )
                .await?,
            )
        })?;
        self.ejecuciones
            .fetch_add(changed as usize, Ordering::Relaxed);
        Ok(())
    }

    fn estado(&self) -> EstadoPersistencia {
        let activa = self.health_ok.load(Ordering::Acquire);
        EstadoPersistencia {
            activa,
            backend: "timescaledb".to_string(),
            ruta: "timescaledb://[redacted]".to_string(),
            operaciones: self.operaciones.load(Ordering::Relaxed),
            oportunidades: self.oportunidades.load(Ordering::Relaxed),
            auditorias: self.auditorias.load(Ordering::Relaxed),
            eventos: self.eventos.load(Ordering::Relaxed),
            rebalanceos: self.rebalanceos.load(Ordering::Relaxed),
            ejecuciones: self.ejecuciones.load(Ordering::Relaxed),
            db_bytes: 0,
            error: (!activa).then(|| "TimescaleDB no saludable".to_string()),
            storage_mode: "timescaledb".to_string(),
            storage_status: if activa { "persistent" } else { "unavailable" }.to_string(),
            storage_persistent: activa,
            queue_capacity: 0,
            queue_pending: 0,
            queue_dropped: 0,
            queue_failed: 0,
            queue_last_error: None,
        }
    }

    fn total_pnl(&self) -> f64 {
        self.block_on(async {
            let c = self.cliente.lock().await;
            c.query_one(
                "SELECT COALESCE(SUM(utilidad_usd), 0.0) FROM operaciones",
                &[],
            )
            .await
            .ok()
            .map(|r| r.get::<_, f64>(0))
            .unwrap_or(0.0)
        })
    }

    fn win_rate(&self) -> f64 {
        self.block_on(async {
            let c = self.cliente.lock().await;
            let total = contar_tabla_valor(&c, "operaciones").await.unwrap_or(0);
            if total == 0 {
                return 0.0;
            }
            let ganadas: i64 = c
                .query_one(
                    "SELECT COUNT(*) FROM operaciones WHERE utilidad_usd > 0",
                    &[],
                )
                .await
                .ok()
                .map(|r| r.get::<_, i64>(0))
                .unwrap_or(0);
            ganadas as f64 / total as f64
        })
    }

    fn ultimas_operaciones(&self, limite: usize) -> Vec<Operacion> {
        self.block_on(async {
            let c = self.cliente.lock().await;
            c.query(
                "SELECT payload_json::text AS payload_json FROM operaciones ORDER BY tiempo DESC LIMIT $1",
                &[&(limite as i64)],
            )
            .await
            .ok()
            .map(|filas| filas.iter().filter_map(fila_operacion).collect::<Vec<_>>())
            .unwrap_or_default()
        })
    }

    fn resumen_agregado(&self) -> serde_json::Value {
        json!({
            "backend": "timescaledb",
            "totalPnl": self.total_pnl(),
            "winRate": self.win_rate(),
            "ruta": "timescaledb://[redacted]",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(sslmode: Option<&str>) -> PgConfig {
        let suffix = sslmode
            .map(|mode| format!("?sslmode={mode}"))
            .unwrap_or_default();
        format!("postgresql://mayab@localhost/mayab{suffix}")
            .parse()
            .expect("config PostgreSQL de prueba válida")
    }

    #[test]
    fn opt_in_inseguro_requiere_true_exacto() {
        assert!(valor_habilita_base_insegura(Some("true")));
        for valor in [None, Some("TRUE"), Some("1"), Some(" true"), Some("true ")] {
            assert!(!valor_habilita_base_insegura(valor));
        }
    }

    #[test]
    fn sslmode_omitido_o_prefer_se_elevan_a_require() {
        for sslmode in [None, Some("prefer")] {
            let mut config = config(sslmode);
            assert_eq!(config.get_ssl_mode(), SslMode::Prefer);
            aplicar_politica_tls(&mut config, false).expect("prefer debe elevarse");
            assert_eq!(config.get_ssl_mode(), SslMode::Require);
        }
    }

    #[test]
    fn sslmode_disable_falla_sin_opt_in_exacto() {
        let mut config = config(Some("disable"));
        let error = aplicar_politica_tls(&mut config, false)
            .expect_err("disable debe rechazarse sin opt-in");
        assert!(error.to_string().contains("ALLOW_INSECURE_DATABASE=true"));
    }

    #[test]
    fn sslmode_disable_solo_pasa_con_opt_in() {
        let mut config = config(Some("disable"));
        aplicar_politica_tls(&mut config, true)
            .expect("el opt-in explícito habilita entorno local");
        assert_eq!(config.get_ssl_mode(), SslMode::Disable);
    }

    #[test]
    fn sslmode_require_permanece_estricto() {
        let mut config = config(Some("require"));
        aplicar_politica_tls(&mut config, false).expect("require ya cumple la política");
        assert_eq!(config.get_ssl_mode(), SslMode::Require);
    }
}
