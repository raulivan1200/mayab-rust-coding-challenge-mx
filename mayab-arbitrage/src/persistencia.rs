//! Auditoría durable local en SQLite.
//!
//! La persistencia es deliberadamente local y sin credenciales: guarda eventos
//! simulados para auditoría y revisión posterior, sin tocar exchanges ni fondos.

use std::{
    path::Path,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Mutex, MutexGuard,
    },
    time::Duration,
};

const LIMITE_OPERACIONES: usize = 10_000;
const LIMITE_OPORTUNIDADES: usize = 25_000;
const LIMITE_EVENTOS: usize = 15_000;
const LIMITE_AUDITORIAS: usize = 25_000;
const LIMITE_REBALANCEOS: usize = 5_000;
const LIMITE_EJECUCIONES: usize = 10_000;
const MANTENIMIENTO_CADA_ESCRITURAS: usize = 256;

fn sqlite_storage_persistent(storage_mode: &str) -> bool {
    matches!(storage_mode, "sqlite_persistent" | "volume")
}

use anyhow::{anyhow, Context};
use rusqlite::{params, Connection};

use crate::auditoria::Auditoria;
use crate::execution::ExecutionReport;
use crate::types::{
    AuditoriaDecision, EstadoPersistencia, EventoEjecucion, Operacion, Oportunidad, Rebalanceo,
};

pub struct Persistencia {
    ruta: String,
    conn: Mutex<Connection>,
    operaciones: AtomicUsize,
    oportunidades: AtomicUsize,
    eventos: AtomicUsize,
    auditorias: AtomicUsize,
    rebalanceos: AtomicUsize,
    ejecuciones: AtomicUsize,
    db_bytes: AtomicU64,
    escrituras_desde_mantenimiento: AtomicUsize,
}

impl Persistencia {
    pub fn abrir(ruta: &str) -> anyhow::Result<Self> {
        let path = Path::new(ruta);
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("no se pudo crear directorio SQLite {}", parent.display())
            })?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("no se pudo abrir SQLite {ruta}"))?;
        conn.busy_timeout(Duration::from_secs(2))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "cache_size", "-65536")?;
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
        conn.execute_batch("PRAGMA mmap_size = 268435456;")?;
        inicializar_schema(&conn)?;
        aplicar_retencion(&conn)?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        let operaciones = contar_tabla(&conn, "operaciones")?;
        let oportunidades = contar_tabla(&conn, "oportunidades")?;
        let eventos = contar_tabla(&conn, "eventos")?;
        let auditorias = contar_tabla(&conn, "auditorias")?;
        let rebalanceos = contar_tabla(&conn, "rebalanceos")?;
        let ejecuciones = contar_tabla(&conn, "ejecuciones")?;
        Ok(Self {
            ruta: ruta.to_string(),
            conn: Mutex::new(conn),
            operaciones: AtomicUsize::new(operaciones),
            oportunidades: AtomicUsize::new(oportunidades),
            eventos: AtomicUsize::new(eventos),
            auditorias: AtomicUsize::new(auditorias),
            rebalanceos: AtomicUsize::new(rebalanceos),
            ejecuciones: AtomicUsize::new(ejecuciones),
            db_bytes: AtomicU64::new(tamano_sqlite(ruta)),
            escrituras_desde_mantenimiento: AtomicUsize::new(0),
        })
    }

    pub fn estado(&self) -> EstadoPersistencia {
        let storage_mode = std::env::var("STORAGE_MODE")
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "sqlite_ephemeral".to_string());
        let storage_persistent = sqlite_storage_persistent(&storage_mode);
        EstadoPersistencia {
            activa: true,
            backend: "sqlite".to_string(),
            ruta: self.ruta.clone(),
            operaciones: self.operaciones.load(Ordering::Relaxed),
            oportunidades: self.oportunidades.load(Ordering::Relaxed),
            eventos: self.eventos.load(Ordering::Relaxed),
            auditorias: self.auditorias.load(Ordering::Relaxed),
            rebalanceos: self.rebalanceos.load(Ordering::Relaxed),
            ejecuciones: self.ejecuciones.load(Ordering::Relaxed),
            db_bytes: self.db_bytes.load(Ordering::Relaxed),
            error: None,
            storage_mode,
            storage_status: if storage_persistent {
                "persistent"
            } else {
                "ephemeral"
            }
            .to_string(),
            storage_persistent,
            queue_capacity: 0,
            queue_pending: 0,
            queue_dropped: 0,
            queue_failed: 0,
            queue_last_error: None,
        }
    }

    pub fn registrar_operacion(&self, op: &Operacion) -> anyhow::Result<()> {
        let conn = self.conn()?;
        let changed = conn.execute(
            "INSERT OR IGNORE INTO operaciones
             (id, tiempo, ruta, par, cantidad_btc, utilidad_usd, parcial, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                op.id,
                op.ejecutada_en.to_rfc3339(),
                format!("{}->{}", op.compra_en, op.venta_en),
                op.par,
                decimal_string(op.cantidad_btc, 8),
                decimal_string(op.utilidad_usd, 6),
                op.parcial,
                serde_json::to_string(op)?,
            ],
        )?;
        if changed > 0 {
            self.operaciones.fetch_add(1, Ordering::Relaxed);
            self.mantenimiento_si_corresponde(&conn, changed)?;
        }
        Ok(())
    }

    pub fn registrar_evento(&self, evento: &EventoEjecucion) -> anyhow::Result<()> {
        let conn = self.conn()?;
        let changed = conn.execute(
            "INSERT OR IGNORE INTO eventos
             (id, tiempo, tipo, ruta, severidad, utilidad_usd, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                evento.id,
                evento.tiempo.to_rfc3339(),
                evento.tipo,
                evento.ruta,
                evento.severidad,
                decimal_string(evento.utilidad_usd, 6),
                serde_json::to_string(evento)?,
            ],
        )?;
        if changed > 0 {
            self.eventos.fetch_add(1, Ordering::Relaxed);
            self.mantenimiento_si_corresponde(&conn, changed)?;
        }
        Ok(())
    }

    pub fn registrar_rebalanceo(&self, rebalanceo: &Rebalanceo) -> anyhow::Result<()> {
        let conn = self.conn()?;
        let changed = conn.execute(
            "INSERT OR IGNORE INTO rebalanceos
             (id, tiempo, ruta, activo, cantidad, costo_usd, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                rebalanceo.id,
                rebalanceo.tiempo.to_rfc3339(),
                format!("{}->{}", rebalanceo.desde, rebalanceo.hacia),
                rebalanceo.activo,
                decimal_string(rebalanceo.cantidad, 8),
                decimal_string(rebalanceo.costo_usd, 6),
                serde_json::to_string(rebalanceo)?,
            ],
        )?;
        if changed > 0 {
            self.rebalanceos.fetch_add(1, Ordering::Relaxed);
            self.mantenimiento_si_corresponde(&conn, changed)?;
        }
        Ok(())
    }

    pub fn registrar_oportunidades(&self, oportunidades: &[Oportunidad]) -> anyhow::Result<()> {
        if oportunidades.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let changed = {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO oportunidades
                 (id, tiempo, ruta, par, ejecutable, utilidad_usd, diferencial_neto_bps, payload_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            let mut changed = 0usize;
            for op in oportunidades {
                changed += stmt.execute(params![
                    op.id,
                    op.detectada_en.to_rfc3339(),
                    format!("{}->{}", op.compra_en, op.venta_en),
                    op.par,
                    op.ejecutable,
                    decimal_string(op.utilidad_usd, 6),
                    decimal_string(op.diferencial_neto_bps, 6),
                    serde_json::to_string(op)?,
                ])?;
            }
            changed
        };
        tx.commit()?;
        if changed > 0 {
            self.oportunidades.fetch_add(changed, Ordering::Relaxed);
            self.mantenimiento_si_corresponde(&conn, changed)?;
        }
        Ok(())
    }

    pub fn registrar_auditorias(&self, auditorias: &[AuditoriaDecision]) -> anyhow::Result<()> {
        if auditorias.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let changed = {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO auditorias
                 (id, tiempo, ruta, decision, score, utilidad_usd, razon, payload_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            let mut changed = 0usize;
            for audit in auditorias {
                changed += stmt.execute(params![
                    audit.id,
                    audit.tiempo.to_rfc3339(),
                    audit.ruta,
                    audit.decision,
                    decimal_string(audit.score, 8),
                    decimal_string(audit.utilidad_usd, 6),
                    audit.razon,
                    serde_json::to_string(audit)?,
                ])?;
            }
            changed
        };
        tx.commit()?;
        if changed > 0 {
            self.auditorias.fetch_add(changed, Ordering::Relaxed);
            self.mantenimiento_si_corresponde(&conn, changed)?;
        }
        Ok(())
    }

    pub fn registrar_ejecucion(&self, report: &ExecutionReport) -> anyhow::Result<()> {
        let conn = self.conn()?;
        let changed = conn.execute(
            "INSERT OR IGNORE INTO ejecuciones
             (id, tiempo, escenario, estado, pnl_usd, payload_json)
             VALUES (?1, strftime('%Y-%m-%dT%H:%M:%fZ','now'), ?2, ?3, ?4, ?5)",
            params![
                report.execution_id,
                serde_json::to_string(&report.scenario)?,
                serde_json::to_string(&report.state)?,
                report.pnl_usd.to_string(),
                serde_json::to_string(report)?,
            ],
        )?;
        if changed > 0 {
            self.ejecuciones.fetch_add(1, Ordering::Relaxed);
            self.mantenimiento_si_corresponde(&conn, changed)?;
        }
        Ok(())
    }

    pub fn total_pnl(&self) -> f64 {
        self.try_total_pnl().unwrap_or_else(|error| {
            tracing::error!(%error, "no se pudo consultar el P&L persistido");
            0.0
        })
    }

    pub fn try_total_pnl(&self) -> anyhow::Result<f64> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT COALESCE(SUM(CAST(utilidad_usd AS REAL)), 0) FROM operaciones",
            [],
            |row| row.get(0),
        )
        .context("no se pudo sumar el P&L persistido")
    }

    pub fn win_rate(&self) -> f64 {
        self.try_win_rate().unwrap_or_else(|error| {
            tracing::error!(%error, "no se pudo consultar el win rate persistido");
            0.0
        })
    }

    pub fn try_win_rate(&self) -> anyhow::Result<f64> {
        let conn = self.conn()?;
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM operaciones", [], |row| row.get(0))
            .context("no se pudo contar operaciones")?;
        if total == 0 {
            return Ok(0.0);
        }
        let ganadoras: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM operaciones WHERE CAST(utilidad_usd AS REAL) > 0",
                [],
                |row| row.get(0),
            )
            .context("no se pudo contar operaciones ganadoras")?;
        Ok(ganadoras as f64 / total as f64)
    }

    pub fn ultimas_operaciones(&self, limite: usize) -> Vec<Operacion> {
        self.try_ultimas_operaciones(limite)
            .unwrap_or_else(|error| {
                tracing::error!(%error, "no se pudieron consultar operaciones persistidas");
                Vec::new()
            })
    }

    pub fn try_ultimas_operaciones(&self, limite: usize) -> anyhow::Result<Vec<Operacion>> {
        let conn = self.conn()?;
        let mut stmt =
            conn.prepare("SELECT payload_json FROM operaciones ORDER BY tiempo DESC LIMIT ?1")?;
        let rows = stmt.query_map([limite.min(i64::MAX as usize) as i64], |row| {
            let json: String = row.get(0)?;
            serde_json::from_str(&json)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("una operación persistida contiene JSON inválido")
    }

    pub fn resumen_agregado(&self) -> serde_json::Value {
        serde_json::json!({
            "totalPnl": self.total_pnl(),
            "winRate": self.win_rate(),
            "operaciones": self.operaciones.load(Ordering::Relaxed),
            "oportunidades": self.oportunidades.load(Ordering::Relaxed),
            "eventos": self.eventos.load(Ordering::Relaxed),
            "auditorias": self.auditorias.load(Ordering::Relaxed),
            "rebalanceos": self.rebalanceos.load(Ordering::Relaxed),
            "ejecuciones": self.ejecuciones.load(Ordering::Relaxed),
            "dbBytes": self.db_bytes.load(Ordering::Relaxed),
        })
    }

    fn conn(&self) -> anyhow::Result<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| anyhow!("conexión SQLite bloqueada por panic previo"))
    }

    fn mantenimiento_si_corresponde(
        &self,
        conn: &Connection,
        escrituras: usize,
    ) -> anyhow::Result<()> {
        let previas = self
            .escrituras_desde_mantenimiento
            .fetch_add(escrituras, Ordering::Relaxed);
        if previas + escrituras < MANTENIMIENTO_CADA_ESCRITURAS {
            return Ok(());
        }
        self.escrituras_desde_mantenimiento
            .store(0, Ordering::Relaxed);
        aplicar_retencion(conn)?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        self.operaciones
            .store(contar_tabla(conn, "operaciones")?, Ordering::Relaxed);
        self.oportunidades
            .store(contar_tabla(conn, "oportunidades")?, Ordering::Relaxed);
        self.eventos
            .store(contar_tabla(conn, "eventos")?, Ordering::Relaxed);
        self.auditorias
            .store(contar_tabla(conn, "auditorias")?, Ordering::Relaxed);
        self.rebalanceos
            .store(contar_tabla(conn, "rebalanceos")?, Ordering::Relaxed);
        self.ejecuciones
            .store(contar_tabla(conn, "ejecuciones")?, Ordering::Relaxed);
        self.db_bytes
            .store(tamano_sqlite(&self.ruta), Ordering::Relaxed);
        Ok(())
    }
}

fn aplicar_retencion(conn: &Connection) -> anyhow::Result<()> {
    for (tabla, limite) in [
        ("operaciones", LIMITE_OPERACIONES),
        ("oportunidades", LIMITE_OPORTUNIDADES),
        ("eventos", LIMITE_EVENTOS),
        ("auditorias", LIMITE_AUDITORIAS),
        ("rebalanceos", LIMITE_REBALANCEOS),
        ("ejecuciones", LIMITE_EJECUCIONES),
    ] {
        let sql = format!(
            "DELETE FROM {tabla} WHERE rowid NOT IN \
             (SELECT rowid FROM {tabla} ORDER BY tiempo DESC, rowid DESC LIMIT ?1)"
        );
        conn.execute(&sql, [limite as i64])?;
    }
    Ok(())
}

fn tamano_sqlite(ruta: &str) -> u64 {
    [
        ruta.to_string(),
        format!("{ruta}-wal"),
        format!("{ruta}-shm"),
    ]
    .iter()
    .filter_map(|path| std::fs::metadata(path).ok())
    .map(|meta| meta.len())
    .sum()
}

fn contar_tabla(conn: &Connection, tabla: &'static str) -> anyhow::Result<usize> {
    let sql = format!("SELECT COUNT(*) FROM {tabla}");
    let total: i64 = conn.query_row(&sql, [], |row| row.get(0))?;
    Ok(total.max(0) as usize)
}

fn inicializar_schema(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS operaciones (
            id TEXT PRIMARY KEY,
            tiempo TEXT NOT NULL,
            ruta TEXT NOT NULL,
            par TEXT NOT NULL,
            cantidad_btc TEXT NOT NULL,
            utilidad_usd TEXT NOT NULL,
            parcial INTEGER NOT NULL,
            payload_json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS oportunidades (
            id TEXT PRIMARY KEY,
            tiempo TEXT NOT NULL,
            ruta TEXT NOT NULL,
            par TEXT NOT NULL,
            ejecutable INTEGER NOT NULL,
            utilidad_usd TEXT NOT NULL,
            diferencial_neto_bps TEXT NOT NULL,
            payload_json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS eventos (
            id TEXT PRIMARY KEY,
            tiempo TEXT NOT NULL,
            tipo TEXT NOT NULL,
            ruta TEXT NOT NULL,
            severidad TEXT NOT NULL,
            utilidad_usd TEXT NOT NULL,
            payload_json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS auditorias (
            id TEXT PRIMARY KEY,
            tiempo TEXT NOT NULL,
            ruta TEXT NOT NULL,
            decision TEXT NOT NULL,
            score TEXT NOT NULL,
            utilidad_usd TEXT NOT NULL,
            razon TEXT NOT NULL,
            payload_json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS rebalanceos (
            id TEXT PRIMARY KEY,
            tiempo TEXT NOT NULL,
            ruta TEXT NOT NULL,
            activo TEXT NOT NULL,
            cantidad TEXT NOT NULL,
            costo_usd TEXT NOT NULL,
            payload_json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS ejecuciones (
            id TEXT PRIMARY KEY,
            tiempo TEXT NOT NULL,
            escenario TEXT NOT NULL,
            estado TEXT NOT NULL,
            pnl_usd TEXT NOT NULL,
            payload_json TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_operaciones_tiempo ON operaciones(tiempo DESC);
        CREATE INDEX IF NOT EXISTS idx_oportunidades_tiempo ON oportunidades(tiempo DESC);
        CREATE INDEX IF NOT EXISTS idx_eventos_tiempo ON eventos(tiempo DESC);
        CREATE INDEX IF NOT EXISTS idx_auditorias_tiempo ON auditorias(tiempo DESC);
        CREATE INDEX IF NOT EXISTS idx_rebalanceos_tiempo ON rebalanceos(tiempo DESC);
        CREATE INDEX IF NOT EXISTS idx_ejecuciones_tiempo ON ejecuciones(tiempo DESC);
        CREATE INDEX IF NOT EXISTS idx_operaciones_ruta ON operaciones(ruta, tiempo DESC);
        CREATE INDEX IF NOT EXISTS idx_eventos_tipo ON eventos(tipo, tiempo DESC);
        CREATE INDEX IF NOT EXISTS idx_auditorias_decision ON auditorias(decision, tiempo DESC);
        "#,
    )?;
    conn.execute_batch("PRAGMA optimize;")?;
    Ok(())
}

fn decimal_string(valor: f64, decimales: usize) -> String {
    if valor.is_finite() {
        format!("{valor:.decimales$}")
    } else {
        "0".to_string()
    }
}

impl Auditoria for Persistencia {
    fn registrar_operacion(&self, op: &Operacion) -> anyhow::Result<()> {
        Persistencia::registrar_operacion(self, op)
    }
    fn registrar_evento(&self, evento: &EventoEjecucion) -> anyhow::Result<()> {
        Persistencia::registrar_evento(self, evento)
    }
    fn registrar_rebalanceo(&self, rebalanceo: &Rebalanceo) -> anyhow::Result<()> {
        Persistencia::registrar_rebalanceo(self, rebalanceo)
    }
    fn registrar_oportunidades(&self, oportunidades: &[Oportunidad]) -> anyhow::Result<()> {
        Persistencia::registrar_oportunidades(self, oportunidades)
    }
    fn registrar_auditorias(&self, auditorias: &[AuditoriaDecision]) -> anyhow::Result<()> {
        Persistencia::registrar_auditorias(self, auditorias)
    }
    fn registrar_ejecucion(&self, ejecucion: &ExecutionReport) -> anyhow::Result<()> {
        Persistencia::registrar_ejecucion(self, ejecucion)
    }
    fn estado(&self) -> EstadoPersistencia {
        Persistencia::estado(self)
    }
    fn total_pnl(&self) -> f64 {
        Persistencia::total_pnl(self)
    }
    fn win_rate(&self) -> f64 {
        Persistencia::win_rate(self)
    }
    fn ultimas_operaciones(&self, limite: usize) -> Vec<Operacion> {
        Persistencia::ultimas_operaciones(self, limite)
    }
    fn resumen_agregado(&self) -> serde_json::Value {
        Persistencia::resumen_agregado(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_no_puede_declararse_timescaledb_ni_durable() {
        assert!(!sqlite_storage_persistent("timescaledb"));
        assert!(!sqlite_storage_persistent("sqlite_ephemeral"));
        assert!(sqlite_storage_persistent("sqlite_persistent"));
        assert!(sqlite_storage_persistent("volume"));
    }

    #[test]
    fn retencion_conserva_solo_las_auditorias_mas_recientes() {
        let conn = Connection::open_in_memory().unwrap();
        inicializar_schema(&conn).unwrap();
        conn.execute_batch(
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x <= 25000)
             INSERT INTO auditorias (id, tiempo, ruta, decision, score, utilidad_usd, razon, payload_json)
             SELECT printf('aud-%05d', x), printf('%05d', x), 'A->B', 'skip', '0', '0', 'qa', '{}'
             FROM n;",
        )
        .unwrap();

        aplicar_retencion(&conn).unwrap();

        assert_eq!(
            contar_tabla(&conn, "auditorias").unwrap(),
            LIMITE_AUDITORIAS
        );
        let mas_antigua: String = conn
            .query_row("SELECT MIN(tiempo) FROM auditorias", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mas_antigua, "00002");
    }

    #[test]
    fn ejecucion_forense_es_idempotente_y_sobrevive_reapertura() {
        let path = std::env::temp_dir().join(format!(
            "mayab-execution-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path_str = path.to_string_lossy().to_string();
        let report = crate::execution::standard_matrix()
            .unwrap()
            .into_iter()
            .next()
            .unwrap();

        {
            let persistence = Persistencia::abrir(&path_str).unwrap();
            persistence.registrar_ejecucion(&report).unwrap();
            persistence.registrar_ejecucion(&report).unwrap();
            assert_eq!(persistence.estado().ejecuciones, 1);
        }
        {
            let reopened = Persistencia::abrir(&path_str).unwrap();
            assert_eq!(reopened.estado().ejecuciones, 1);
        }

        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{path_str}{suffix}"));
        }
    }
}
