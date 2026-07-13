//! Backend de auditoría TimescaleDB/Postgres (opt-in, feature `timescaledb`).
//!
//! Implementa el mismo contrato [`crate::auditoria::Auditoria`] que la
//! persistencia SQLite local, pero sobre hypertables de TimescaleDB. Se activa
//! con `cargo build --features timescaledb` y requiere `DATABASE_URL` apuntando
//! a una instancia TimescaleDB con el esquema de `scripts/timescaledb/schema.sql`.
//!
//! El motor no cambia: basta intercambiar la implementación de `Auditoria`.

use anyhow::Context;
use serde_json::json;
use std::{
    future::Future,
    sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
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
    health_ok: AtomicBool,
    health_checked_at_ms: AtomicI64,
}

impl TimescaleDbAuditoria {
    /// Conecta a TimescaleDB y verifica el esquema.
    pub async fn abrir(url: &str) -> anyhow::Result<Self> {
        let config: PgConfig = url
            .parse()
            .context("DATABASE_URL de TimescaleDB inválida")?;
        let cliente = if config.get_ssl_mode() == SslMode::Disable {
            let (cliente, conexion) = config
                .connect(NoTls)
                .await
                .context("no se pudo conectar a TimescaleDB sin TLS")?;
            tokio::spawn(async move {
                if let Err(err) = conexion.await {
                    tracing::warn!(error = %err, "conexión TimescaleDB cerrada");
                }
            });
            cliente
        } else {
            let roots =
                rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let tls_config = rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();
            let (cliente, conexion) = config
                .connect(MakeRustlsConnect::new(tls_config))
                .await
                .context("no se pudo conectar a TimescaleDB con TLS")?;
            tokio::spawn(async move {
                if let Err(err) = conexion.await {
                    tracing::warn!(error = %err, "conexión TLS de TimescaleDB cerrada");
                }
            });
            cliente
        };
        cliente
            .batch_execute(
                "SELECT 1 FROM operaciones LIMIT 1;
                 SELECT 1 FROM auditorias LIMIT 1;
                 SELECT 1 FROM ejecuciones LIMIT 1;",
            )
            .await
            .context("el esquema TimescaleDB no está inicializado")?;
        let operaciones = contar_tabla(&cliente, "operaciones").await?;
        let oportunidades = contar_tabla(&cliente, "oportunidades").await?;
        let auditorias = contar_tabla(&cliente, "auditorias").await?;
        let eventos = contar_tabla(&cliente, "eventos").await?;
        let rebalanceos = contar_tabla(&cliente, "rebalanceos").await?;
        let ejecuciones = contar_tabla(&cliente, "ejecuciones").await?;
        Ok(Self {
            cliente: Mutex::new(cliente),
            runtime: tokio::runtime::Handle::current(),
            operaciones,
            oportunidades,
            auditorias,
            eventos,
            rebalanceos,
            ejecuciones,
            health_ok: AtomicBool::new(true),
            health_checked_at_ms: AtomicI64::new(epoch_millis()),
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

fn epoch_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
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
        let (id, compra, venta, par, cantidad, utilidad, costo, parcial) = (
            op.id.clone(),
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
                "INSERT INTO operaciones (tiempo, id, compra_en, venta_en, par, cantidad_btc, utilidad_usd, costo_usd, score, partial_fill, payload_json) VALUES (NOW(), $1, $2, $3, $4, $5, $6, $7, $8, $9, $10::jsonb)",
                &[&id, &compra, &venta, &par, &cantidad, &utilidad, &costo, &None::<f64>, &parcial, &payload],
            )
            .await?)
        })?;
        self.operaciones
            .fetch_add(changed as usize, Ordering::Relaxed);
        Ok(())
    }

    fn registrar_evento(&self, evento: &EventoEjecucion) -> anyhow::Result<()> {
        let payload = json!(evento).to_string();
        let (id, tipo, severidad, detalle) = (
            evento.id.clone(),
            evento.tipo.clone(),
            evento.severidad.clone(),
            evento.detalle.clone(),
        );
        let changed = self.block_on(async move {
            let c = self.cliente.lock().await;
            Ok::<_, anyhow::Error>(c.execute(
                "INSERT INTO eventos (tiempo, id, tipo, severidad, mensaje, payload_json) VALUES (NOW(), $1, $2, $3, $4, $5::jsonb)",
                &[&id, &tipo, &severidad, &detalle, &payload],
            )
            .await?)
        })?;
        self.eventos.fetch_add(changed as usize, Ordering::Relaxed);
        Ok(())
    }

    fn registrar_rebalanceo(&self, r: &Rebalanceo) -> anyhow::Result<()> {
        let payload = json!(r).to_string();
        let (id, desde, hacia, cantidad, costo) = (
            r.id.clone(),
            r.desde.clone(),
            r.hacia.clone(),
            r.cantidad,
            r.costo_usd,
        );
        let changed = self.block_on(async move {
            let c = self.cliente.lock().await;
            Ok::<_, anyhow::Error>(c.execute(
                "INSERT INTO rebalanceos (tiempo, id, desde, hacia, cantidad, costo_usd, payload_json) VALUES (NOW(), $1, $2, $3, $4, $5, $6::jsonb)",
                &[&id, &desde, &hacia, &cantidad, &costo, &payload],
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
            let (id, compra, venta, utilidad, diff, payload) = (
                o.id.clone(),
                o.compra_en.clone(),
                o.venta_en.clone(),
                o.utilidad_usd,
                o.diferencial_neto_bps,
                payload,
            );
            let changed = self.block_on(async move {
                let c = self.cliente.lock().await;
                Ok::<_, anyhow::Error>(c.execute(
                    "INSERT INTO oportunidades (tiempo, id, ruta, utilidad_usd, diferencial, payload_json) VALUES (NOW(), $1, $2, $3, $4, $5::jsonb)",
                    &[&id, &format!("{compra}->{venta}"), &utilidad, &diff, &payload],
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
            let (id, ruta, decision, score, utilidad, razon, payload) = (
                a.id.clone(),
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
                    "INSERT INTO auditorias (tiempo, id, ruta, decision, score, utilidad_usd, razon, payload_json) VALUES (NOW(), $1, $2, $3, $4, $5, $6, $7::jsonb)",
                    &[&id, &ruta, &decision, &score, &utilidad, &razon, &payload],
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
        let changed =
            self.block_on(async move {
                let c = self.cliente.lock().await;
                Ok::<_, anyhow::Error>(c.execute(
                "INSERT INTO ejecuciones (tiempo, id, escenario, estado, pnl_usd, payload_json)
                 VALUES (NOW(), $1, $2, $3, $4, $5::jsonb)
                 ON CONFLICT (id) DO NOTHING",
                &[&id, &scenario, &state, &pnl, &payload],
            )
            .await?)
            })?;
        self.ejecuciones
            .fetch_add(changed as usize, Ordering::Relaxed);
        Ok(())
    }

    fn estado(&self) -> EstadoPersistencia {
        let now = epoch_millis();
        let last = self.health_checked_at_ms.load(Ordering::Relaxed);
        if now.saturating_sub(last) >= 5_000
            && self
                .health_checked_at_ms
                .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        {
            let health_ok = self.block_on(async {
                let c = self.cliente.lock().await;
                c.simple_query("SELECT 1").await.is_ok()
            });
            self.health_ok.store(health_ok, Ordering::Release);
        }
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
            ganadas as f64 / total as f64 * 100.0
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
