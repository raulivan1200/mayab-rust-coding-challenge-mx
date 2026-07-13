//! Abstracción de auditoría durable (repository pattern).
//!
//! El motor no depende de SQLite: usa `Arc<dyn Auditoria>`. La implementación
//! por defecto es [`crate::persistencia::Persistencia`] (SQLite local). Una
//! implementación TimescaleDB/Postgres puede sustituirla tras habilitar la
//! feature `timescaledb`, sin tocar el motor ni la API.

use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    mpsc::{self, SyncSender, TrySendError},
    Arc,
};

use anyhow::{anyhow, Result};

use crate::execution::ExecutionReport;
use crate::types::{
    AuditoriaDecision, EstadoPersistencia, EventoEjecucion, Operacion, Oportunidad, Rebalanceo,
};

/// Contrato de persistencia de auditoría para el motor.
pub trait Auditoria: Send + Sync {
    /// Registra una operación simulada.
    fn registrar_operacion(&self, op: &Operacion) -> Result<()>;
    /// Registra un evento de ejecución.
    fn registrar_evento(&self, evento: &EventoEjecucion) -> Result<()>;
    /// Registra un rebalanceo de carteras.
    fn registrar_rebalanceo(&self, rebalanceo: &Rebalanceo) -> Result<()>;
    /// Registra oportunidades detectadas.
    fn registrar_oportunidades(&self, oportunidades: &[Oportunidad]) -> Result<()>;
    /// Registra decisiones auditadas.
    fn registrar_auditorias(&self, auditorias: &[AuditoriaDecision]) -> Result<()>;
    /// Persiste el reporte forense completo de una ejecución de dos piernas.
    fn registrar_ejecucion(&self, ejecucion: &ExecutionReport) -> Result<()>;
    /// Snapshot de estado de la capa de persistencia.
    fn estado(&self) -> EstadoPersistencia;
    /// PnL total acumulado en la auditoría.
    fn total_pnl(&self) -> f64;
    /// Win rate agregado.
    fn win_rate(&self) -> f64;
    /// Últimas operaciones registradas.
    fn ultimas_operaciones(&self, limite: usize) -> Vec<Operacion>;
    /// Resumen agregado para el contrato público.
    fn resumen_agregado(&self) -> serde_json::Value;
}

enum EscrituraAuditoria {
    Operacion(Operacion),
    Evento(EventoEjecucion),
    Rebalanceo(Rebalanceo),
    Oportunidades(Vec<Oportunidad>),
    Auditorias(Vec<AuditoriaDecision>),
    Ejecucion(ExecutionReport),
}

/// Worker único de persistencia con backpressure acotado y no bloqueante.
pub struct AuditoriaEnCola {
    backend: Arc<dyn Auditoria>,
    tx: SyncSender<EscrituraAuditoria>,
    pendientes: Arc<AtomicUsize>,
    descartadas: AtomicU64,
    capacidad: usize,
}

impl AuditoriaEnCola {
    pub fn nueva(backend: Arc<dyn Auditoria>, capacidad: usize) -> Self {
        let capacidad = capacidad.max(1);
        let (tx, rx) = mpsc::sync_channel(capacidad);
        let pendientes = Arc::new(AtomicUsize::new(0));
        let pendientes_worker = pendientes.clone();
        let backend_worker = backend.clone();
        std::thread::Builder::new()
            .name("mayab-persistence".to_string())
            .spawn(move || {
                while let Ok(escritura) = rx.recv() {
                    let resultado = match escritura {
                        EscrituraAuditoria::Operacion(v) => backend_worker.registrar_operacion(&v),
                        EscrituraAuditoria::Evento(v) => backend_worker.registrar_evento(&v),
                        EscrituraAuditoria::Rebalanceo(v) => {
                            backend_worker.registrar_rebalanceo(&v)
                        }
                        EscrituraAuditoria::Oportunidades(v) => {
                            backend_worker.registrar_oportunidades(&v)
                        }
                        EscrituraAuditoria::Auditorias(v) => {
                            backend_worker.registrar_auditorias(&v)
                        }
                        EscrituraAuditoria::Ejecucion(v) => backend_worker.registrar_ejecucion(&v),
                    };
                    pendientes_worker.fetch_sub(1, Ordering::Relaxed);
                    if let Err(error) = resultado {
                        tracing::warn!(%error, "fallo del worker de persistencia");
                    }
                }
            })
            .expect("no se pudo iniciar worker de persistencia");
        Self {
            backend,
            tx,
            pendientes,
            descartadas: AtomicU64::new(0),
            capacidad,
        }
    }

    fn encolar(&self, escritura: EscrituraAuditoria) -> Result<()> {
        self.pendientes.fetch_add(1, Ordering::Relaxed);
        match self.tx.try_send(escritura) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.pendientes.fetch_sub(1, Ordering::Relaxed);
                self.descartadas.fetch_add(1, Ordering::Relaxed);
                Err(anyhow!("cola de persistencia llena"))
            }
            Err(TrySendError::Disconnected(_)) => {
                self.pendientes.fetch_sub(1, Ordering::Relaxed);
                Err(anyhow!("worker de persistencia detenido"))
            }
        }
    }

    /// Espera de forma acotada a que el worker procese las escrituras ya
    /// aceptadas. Es útil antes de sellar exports o apagar una demo; nunca
    /// bloquea indefinidamente si el backend está degradado.
    pub fn esperar_drenado(&self, timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while self.pendientes.load(Ordering::Acquire) > 0 {
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        true
    }
}

impl Auditoria for AuditoriaEnCola {
    fn registrar_operacion(&self, v: &Operacion) -> Result<()> {
        self.encolar(EscrituraAuditoria::Operacion(v.clone()))
    }
    fn registrar_evento(&self, v: &EventoEjecucion) -> Result<()> {
        self.encolar(EscrituraAuditoria::Evento(v.clone()))
    }
    fn registrar_rebalanceo(&self, v: &Rebalanceo) -> Result<()> {
        self.encolar(EscrituraAuditoria::Rebalanceo(v.clone()))
    }
    fn registrar_oportunidades(&self, v: &[Oportunidad]) -> Result<()> {
        self.encolar(EscrituraAuditoria::Oportunidades(v.to_vec()))
    }
    fn registrar_auditorias(&self, v: &[AuditoriaDecision]) -> Result<()> {
        self.encolar(EscrituraAuditoria::Auditorias(v.to_vec()))
    }
    fn registrar_ejecucion(&self, v: &ExecutionReport) -> Result<()> {
        self.encolar(EscrituraAuditoria::Ejecucion(v.clone()))
    }
    fn estado(&self) -> EstadoPersistencia {
        let mut estado = self.backend.estado();
        estado.queue_capacity = self.capacidad;
        estado.queue_pending = self.pendientes.load(Ordering::Relaxed);
        estado.queue_dropped = self.descartadas.load(Ordering::Relaxed);
        estado
    }
    fn total_pnl(&self) -> f64 {
        self.backend.total_pnl()
    }
    fn win_rate(&self) -> f64 {
        self.backend.win_rate()
    }
    fn ultimas_operaciones(&self, limite: usize) -> Vec<Operacion> {
        self.backend.ultimas_operaciones(limite)
    }
    fn resumen_agregado(&self) -> serde_json::Value {
        self.backend.resumen_agregado()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc, Condvar, Mutex,
        },
        thread,
        time::{Duration, Instant},
    };

    use super::*;

    struct BackendControlado {
        entered: mpsc::SyncSender<()>,
        release: Arc<(Mutex<bool>, Condvar)>,
        writes: AtomicUsize,
        fail: AtomicBool,
    }

    impl BackendControlado {
        fn wait_if_blocked(&self) -> Result<()> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            let _ = self.entered.try_send(());
            let (lock, wake) = &*self.release;
            let mut released = lock.lock().expect("release lock poisoned");
            while !*released {
                released = wake.wait(released).expect("release wait poisoned");
            }
            if self.fail.load(Ordering::SeqCst) {
                Err(anyhow!("backend failure fixture"))
            } else {
                Ok(())
            }
        }
    }

    impl Auditoria for BackendControlado {
        fn registrar_operacion(&self, _: &Operacion) -> Result<()> {
            self.wait_if_blocked()
        }
        fn registrar_evento(&self, _: &EventoEjecucion) -> Result<()> {
            self.wait_if_blocked()
        }
        fn registrar_rebalanceo(&self, _: &Rebalanceo) -> Result<()> {
            self.wait_if_blocked()
        }
        fn registrar_oportunidades(&self, _: &[Oportunidad]) -> Result<()> {
            self.wait_if_blocked()
        }
        fn registrar_auditorias(&self, _: &[AuditoriaDecision]) -> Result<()> {
            self.wait_if_blocked()
        }
        fn registrar_ejecucion(&self, _: &ExecutionReport) -> Result<()> {
            self.wait_if_blocked()
        }
        fn estado(&self) -> EstadoPersistencia {
            EstadoPersistencia::inactiva("test://controlled")
        }
        fn total_pnl(&self) -> f64 {
            42.5
        }
        fn win_rate(&self) -> f64 {
            0.625
        }
        fn ultimas_operaciones(&self, _: usize) -> Vec<Operacion> {
            Vec::new()
        }
        fn resumen_agregado(&self) -> serde_json::Value {
            serde_json::json!({"fixture": true})
        }
    }

    fn backend(
        initially_released: bool,
        fail: bool,
    ) -> (Arc<BackendControlado>, mpsc::Receiver<()>) {
        let (entered, receiver) = mpsc::sync_channel(8);
        (
            Arc::new(BackendControlado {
                entered,
                release: Arc::new((Mutex::new(initially_released), Condvar::new())),
                writes: AtomicUsize::new(0),
                fail: AtomicBool::new(fail),
            }),
            receiver,
        )
    }

    fn release(backend: &BackendControlado) {
        let (lock, wake) = &*backend.release;
        *lock.lock().expect("release lock poisoned") = true;
        wake.notify_all();
    }

    fn wait_pending(queue: &AuditoriaEnCola, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while queue.estado().queue_pending != expected && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(queue.estado().queue_pending, expected);
    }

    #[test]
    fn queue_normalizes_zero_capacity_and_delegates_read_models() {
        let (backend, _) = backend(true, false);
        let queue = AuditoriaEnCola::nueva(backend, 0);
        assert_eq!(queue.estado().queue_capacity, 1);
        assert_eq!(queue.total_pnl(), 42.5);
        assert_eq!(queue.win_rate(), 0.625);
        assert_eq!(
            queue.resumen_agregado(),
            serde_json::json!({"fixture": true})
        );
    }

    #[test]
    fn full_queue_fails_fast_and_exposes_dropped_counter() {
        let (backend, entered) = backend(false, false);
        let queue = AuditoriaEnCola::nueva(backend.clone(), 1);
        queue.registrar_auditorias(&[]).unwrap();
        entered.recv_timeout(Duration::from_secs(1)).unwrap();
        queue.registrar_auditorias(&[]).unwrap();
        let error = queue.registrar_auditorias(&[]).unwrap_err();
        assert!(error.to_string().contains("cola de persistencia llena"));
        assert_eq!(queue.estado().queue_dropped, 1);
        assert_eq!(queue.estado().queue_pending, 2);
        release(&backend);
        wait_pending(&queue, 0);
        assert_eq!(backend.writes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn backend_failure_still_decrements_pending_without_deadlock() {
        let (backend, entered) = backend(false, true);
        let queue = AuditoriaEnCola::nueva(backend.clone(), 2);
        queue.registrar_auditorias(&[]).unwrap();
        entered.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(queue.estado().queue_pending, 1);
        release(&backend);
        wait_pending(&queue, 0);
        assert_eq!(backend.writes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn bounded_flush_times_out_then_succeeds_after_release() {
        let (backend, entered) = backend(false, false);
        let queue = AuditoriaEnCola::nueva(backend.clone(), 1);
        queue.registrar_auditorias(&[]).unwrap();
        entered.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(!queue.esperar_drenado(Duration::from_millis(5)));
        release(&backend);
        assert!(queue.esperar_drenado(Duration::from_secs(1)));
    }

    #[test]
    fn successful_worker_drains_multiple_write_kinds() {
        let (backend, entered) = backend(false, false);
        let queue = AuditoriaEnCola::nueva(backend.clone(), 4);
        queue.registrar_oportunidades(&[]).unwrap();
        entered.recv_timeout(Duration::from_secs(1)).unwrap();
        queue.registrar_auditorias(&[]).unwrap();
        release(&backend);
        wait_pending(&queue, 0);
        assert_eq!(backend.writes.load(Ordering::SeqCst), 2);
        assert_eq!(queue.estado().queue_dropped, 0);
    }
}
