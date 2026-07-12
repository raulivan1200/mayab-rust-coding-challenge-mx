//! Tape reproducible de libros públicos y verificación offline.

use crate::{
    mercado,
    motor::calcular_costos_canonicos,
    types::{Cotizacion, MapaCostos, NivelOrden},
};
use anyhow::{bail, Context};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

type BookSides = (BTreeMap<i64, f64>, BTreeMap<i64, f64>);

pub const EVENTS_FILE: &str = "events.jsonl";
pub const MANIFEST_FILE: &str = "manifest.json";
pub const CONFIG_FILE: &str = "capture-config.json";

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("ruta de artefacto sin nombre UTF-8")?;
    let nonce = Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let temporary = parent.join(format!(".{file_name}.tmp-{}-{nonce}", std::process::id()));
    let result = (|| -> anyhow::Result<()> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        serde_json::to_writer_pretty(&mut file, value)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum TapeSource {
    WebSocket { url: String },
    Rest { url: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Snapshot,
    Delta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrityState {
    pub status: String,
    pub gap: bool,
    pub resync: bool,
    #[serde(default)]
    pub connection_epoch: u64,
    #[serde(default)]
    pub reconnected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TapeEvent {
    pub schema_version: u32,
    pub exchange_timestamp: Option<DateTime<Utc>>,
    pub local_timestamp: DateTime<Utc>,
    pub exchange: String,
    pub pair: String,
    pub source: TapeSource,
    pub kind: EventKind,
    pub sequence_id: Option<u64>,
    pub previous_sequence: Option<u64>,
    pub bids: Vec<NivelOrden>,
    pub asks: Vec<NivelOrden>,
    pub integrity: IntegrityState,
    pub observed_latency_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureConfig {
    pub schema_version: u32,
    pub pair: String,
    pub exchanges: Vec<String>,
    pub depth: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TapeManifest {
    pub schema_version: u32,
    #[serde(default)]
    pub dataset_id: String,
    #[serde(default = "default_source_classification")]
    pub source_classification: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub exchanges: Vec<String>,
    pub pairs: Vec<String>,
    pub events: u64,
    pub snapshots: u64,
    pub sequence_gaps: u64,
    pub rest_fallback_events: u64,
    #[serde(default)]
    pub reconnect_events: u64,
    #[serde(default = "default_delivery_policy")]
    pub delivery_policy: String,
    #[serde(default)]
    pub events_by_exchange: BTreeMap<String, u64>,
    #[serde(default)]
    pub uncompressed_bytes: u64,
    #[serde(default)]
    pub duration_ms: i64,
    pub sha256: String,
    pub git_commit: String,
    pub config_sha256: String,
}

pub async fn capture(
    output: &Path,
    duration: Duration,
    config: CaptureConfig,
) -> anyhow::Result<TapeManifest> {
    if output.exists() {
        bail!("la salida ya existe: {}", output.display());
    }
    fs::create_dir_all(output)?;
    let config_bytes = serde_json::to_vec_pretty(&config)?;
    fs::write(output.join(CONFIG_FILE), &config_bytes)?;
    let config_sha256 = hex_sha(&config_bytes);
    let started_at = Utc::now();
    let (tx, mut rx) = tokio::sync::mpsc::channel(8192);
    mercado::capture_public_books(
        config.pair.clone(),
        config.exchanges.clone(),
        config.depth,
        tx,
    )
    .await?;
    let mut writer = BufWriter::new(File::create(output.join(EVENTS_FILE))?);
    let deadline = tokio::time::Instant::now() + duration;
    let mut events = 0;
    let mut snapshots = 0;
    let mut gaps = 0;
    let mut rest = 0;
    let mut reconnects = 0;
    let mut events_by_exchange = BTreeMap::<String, u64>::new();
    loop {
        let event = tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break,
            event = rx.recv() => event.context("todos los capturadores terminaron")?,
            _ = tokio::signal::ctrl_c() => break,
        };
        serde_json::to_writer(&mut writer, &event)?;
        writer.write_all(b"\n")?;
        events += 1;
        snapshots += u64::from(event.kind == EventKind::Snapshot);
        gaps += u64::from(event.integrity.gap);
        rest += u64::from(matches!(event.source, TapeSource::Rest { .. }));
        reconnects += u64::from(event.integrity.reconnected);
        *events_by_exchange
            .entry(event.exchange.clone())
            .or_default() += 1;
    }
    writer.flush()?;
    if events == 0 {
        bail!("captura vacía; verifica conectividad y exchanges");
    }
    let sha256 = file_sha(&output.join(EVENTS_FILE))?;
    let ended_at = Utc::now();
    let uncompressed_bytes = fs::metadata(output.join(EVENTS_FILE))?.len();
    let manifest = TapeManifest {
        schema_version: 1,
        dataset_id: format!(
            "mayab-public-{}-{}",
            started_at.format("%Y%m%dT%H%M%SZ"),
            &sha256[..12]
        ),
        source_classification: default_source_classification(),
        started_at,
        ended_at,
        exchanges: config.exchanges,
        pairs: vec![config.pair],
        events,
        snapshots,
        sequence_gaps: gaps,
        rest_fallback_events: rest,
        reconnect_events: reconnects,
        delivery_policy: default_delivery_policy(),
        events_by_exchange,
        uncompressed_bytes,
        duration_ms: (ended_at - started_at).num_milliseconds().max(0),
        sha256,
        git_commit: git_commit(),
        config_sha256,
    };
    fs::write(
        output.join(MANIFEST_FILE),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(manifest)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Verification {
    pub path: PathBuf,
    pub dataset_id: String,
    pub source_classification: String,
    pub events: u64,
    pub snapshots: u64,
    pub sequence_gaps: u64,
    pub rest_fallback_events: u64,
    pub reconnect_events: u64,
    pub delivery_policy: String,
    pub books_reconstructed: usize,
    pub exchanges: Vec<String>,
    pub pairs: Vec<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub duration_ms: i64,
    pub uncompressed_bytes: u64,
    pub events_by_exchange: BTreeMap<String, u64>,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CorpusTape {
    pub dataset_id: String,
    pub relative_path: PathBuf,
    pub events: u64,
    pub sequence_gaps: u64,
    pub reconnect_events: u64,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub exchanges: Vec<String>,
    pub pairs: Vec<String>,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CorpusReport {
    pub schema_version: u32,
    pub classification: String,
    pub corpus_sha256: String,
    pub generated_at: DateTime<Utc>,
    pub root: PathBuf,
    pub unique_tapes: usize,
    pub total_events: u64,
    pub total_sequence_gaps: u64,
    pub total_reconnect_events: u64,
    pub total_rest_fallback_events: u64,
    pub total_uncompressed_bytes: u64,
    pub earliest_event: DateTime<Utc>,
    pub latest_event: DateTime<Utc>,
    pub observed_span_ms: i64,
    pub total_capture_duration_ms: i64,
    pub events_by_exchange: BTreeMap<String, u64>,
    pub pairs: Vec<String>,
    pub evidence_gates: EvidenceGates,
    pub tapes: Vec<CorpusTape>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceGates {
    pub multi_venue: bool,
    pub minimum_ten_shards: bool,
    pub preliminary_100k_events: bool,
    pub million_event_scale: bool,
    pub twenty_four_captured_hours: bool,
    pub delivery_is_loss_accounted: bool,
    pub sequence_gap_rate_below_one_percent: bool,
    pub publishable_scale: bool,
    pub status: &'static str,
    pub note: &'static str,
}

/// Verifica y agrega un corpus de tapes sin permitir inflación por duplicados.
/// Cada subdirectorio directo que contiene `manifest.json` se considera un
/// shard. El hash del corpus depende de la lista ordenada de hashes de eventos.
pub fn verify_corpus(root: &Path) -> anyhow::Result<CorpusReport> {
    verify_corpus_classified(root, &["public_market_capture"])
}

fn verify_corpus_classified(
    root: &Path,
    allowed_classifications: &[&str],
) -> anyhow::Result<CorpusReport> {
    let mut directories = fs::read_dir(root)
        .with_context(|| format!("no se pudo leer corpus {}", root.display()))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir())
                .map(|_| entry.path())
        })
        .filter(|path| is_corpus_shard(path))
        .collect::<Vec<_>>();
    directories.sort();
    if directories.is_empty() {
        bail!("el corpus no contiene tapes con manifest.json");
    }

    let mut seen_hashes = std::collections::HashSet::new();
    let mut verifications = Vec::with_capacity(directories.len());
    let mut source_classification: Option<String> = None;
    for directory in directories {
        let verified = verify(&directory)?;
        if !allowed_classifications.contains(&verified.source_classification.as_str()) {
            bail!(
                "tape {} tiene clasificación no permitida: {}",
                verified.dataset_id,
                verified.source_classification
            );
        }
        if source_classification
            .as_ref()
            .is_some_and(|classification| classification != &verified.source_classification)
        {
            bail!("el corpus mezcla clasificaciones de procedencia");
        }
        source_classification.get_or_insert_with(|| verified.source_classification.clone());
        if !seen_hashes.insert(verified.sha256.clone()) {
            bail!("tape duplicado por sha256: {}", verified.sha256);
        }
        verifications.push(verified);
    }

    verifications.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then_with(|| a.sha256.cmp(&b.sha256))
    });
    for left_index in 0..verifications.len() {
        for right in &verifications[left_index + 1..] {
            let left = &verifications[left_index];
            if right.started_at >= left.ended_at {
                break;
            }
            if markets_overlap(left, right) {
                bail!(
                    "ventanas solapadas para el mismo mercado: {} y {}",
                    left.dataset_id,
                    right.dataset_id
                );
            }
        }
    }
    let earliest_event = verifications
        .iter()
        .map(|v| v.started_at)
        .min()
        .context("corpus sin inicio")?;
    let latest_event = verifications
        .iter()
        .map(|v| v.ended_at)
        .max()
        .context("corpus sin fin")?;
    let mut events_by_exchange = BTreeMap::<String, u64>::new();
    let mut pairs = std::collections::BTreeSet::new();
    let mut corpus_hashes = Vec::with_capacity(verifications.len());
    let mut tapes = Vec::with_capacity(verifications.len());
    let mut total_events = 0_u64;
    let mut total_uncompressed_bytes = 0_u64;
    let mut total_capture_duration_ms = 0_i64;
    let mut total_sequence_gaps = 0_u64;
    let mut total_reconnect_events = 0_u64;
    let mut total_rest_fallback_events = 0_u64;
    let mut delivery_is_loss_accounted = true;

    for verified in verifications {
        total_events = total_events
            .checked_add(verified.events)
            .context("overflow al agregar eventos del corpus")?;
        total_uncompressed_bytes = total_uncompressed_bytes
            .checked_add(verified.uncompressed_bytes)
            .context("overflow al agregar bytes del corpus")?;
        total_capture_duration_ms = total_capture_duration_ms
            .checked_add(verified.duration_ms)
            .context("overflow al agregar duración del corpus")?;
        total_sequence_gaps = total_sequence_gaps
            .checked_add(verified.sequence_gaps)
            .context("overflow al agregar gaps del corpus")?;
        total_reconnect_events = total_reconnect_events
            .checked_add(verified.reconnect_events)
            .context("overflow al agregar reconexiones del corpus")?;
        total_rest_fallback_events = total_rest_fallback_events
            .checked_add(verified.rest_fallback_events)
            .context("overflow al agregar fallbacks REST del corpus")?;
        delivery_is_loss_accounted &=
            verified.delivery_policy == "bounded_channel_await_no_application_drop";
        for (exchange, count) in &verified.events_by_exchange {
            let entry = events_by_exchange.entry(exchange.clone()).or_default();
            *entry = entry
                .checked_add(*count)
                .context("overflow en eventsByExchange")?;
        }
        pairs.extend(verified.pairs.iter().cloned());
        corpus_hashes.push(verified.sha256.clone());
        tapes.push(CorpusTape {
            dataset_id: verified.dataset_id,
            relative_path: verified
                .path
                .strip_prefix(root)
                .unwrap_or(&verified.path)
                .to_path_buf(),
            events: verified.events,
            sequence_gaps: verified.sequence_gaps,
            reconnect_events: verified.reconnect_events,
            started_at: verified.started_at,
            ended_at: verified.ended_at,
            exchanges: verified.exchanges,
            pairs: verified.pairs,
            sha256: verified.sha256,
        });
    }

    let multi_venue = events_by_exchange.len() >= 2;
    let minimum_ten_shards = tapes.len() >= 10;
    let preliminary_100k_events = total_events >= 100_000;
    let million_event_scale = total_events >= 1_000_000;
    let twenty_four_captured_hours = total_capture_duration_ms >= 24 * 60 * 60 * 1_000;
    let sequence_gap_rate_below_one_percent =
        total_sequence_gaps as f64 / total_events.max(1) as f64 <= 0.01;
    let corpus_classification = match source_classification.as_deref() {
        Some("public_market_capture") => "public_market_capture_corpus",
        Some("synthetic_benchmark") => "synthetic_benchmark_corpus",
        _ => "unsupported_corpus",
    };
    let scale_requirements_met = multi_venue
        && minimum_ten_shards
        && million_event_scale
        && twenty_four_captured_hours
        && delivery_is_loss_accounted
        && sequence_gap_rate_below_one_percent;
    let (publishable_scale, publication_status) =
        corpus_publication_status(corpus_classification, scale_requirements_met);
    Ok(CorpusReport {
        schema_version: 1,
        classification: corpus_classification.into(),
        corpus_sha256: corpus_sha256(&corpus_hashes),
        generated_at: Utc::now(),
        root: root.to_path_buf(),
        unique_tapes: tapes.len(),
        total_events,
        total_sequence_gaps,
        total_reconnect_events,
        total_rest_fallback_events,
        total_uncompressed_bytes,
        earliest_event,
        latest_event,
        observed_span_ms: (latest_event - earliest_event).num_milliseconds().max(0),
        total_capture_duration_ms,
        events_by_exchange,
        pairs: pairs.into_iter().collect(),
        evidence_gates: EvidenceGates {
            multi_venue,
            minimum_ten_shards,
            preliminary_100k_events,
            million_event_scale,
            twenty_four_captured_hours,
            delivery_is_loss_accounted,
            sequence_gap_rate_below_one_percent,
            publishable_scale,
            status: publication_status,
            note: "escala verificada no implica rentabilidad ni un millón de dislocaciones; solo volumen de eventos públicos deduplicados",
        },
        tapes,
    })
}

fn corpus_publication_status(
    classification: &str,
    scale_requirements_met: bool,
) -> (bool, &'static str) {
    if classification != "public_market_capture_corpus" {
        (false, "synthetic_only")
    } else if scale_requirements_met {
        (true, "scale_verified")
    } else {
        (false, "insufficient_scale")
    }
}

/// Materializa un índice SQLite transaccional del corpus. Los eventos crudos
/// permanecen en shards append-only; SQLite guarda únicamente metadatos y
/// conteos para consultas rápidas sin introducir contención en la captura.
pub fn index_corpus_sqlite(report: &CorpusReport, database: &Path) -> anyhow::Result<()> {
    if let Some(parent) = database.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut connection = Connection::open(database)?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS corpus (
            corpus_sha256 TEXT PRIMARY KEY,
            classification TEXT NOT NULL,
            generated_at TEXT NOT NULL,
            unique_tapes INTEGER NOT NULL CHECK(unique_tapes >= 0),
            total_events INTEGER NOT NULL CHECK(total_events >= 0),
            total_bytes INTEGER NOT NULL CHECK(total_bytes >= 0),
            total_capture_duration_ms INTEGER NOT NULL CHECK(total_capture_duration_ms >= 0),
            earliest_event TEXT NOT NULL,
            latest_event TEXT NOT NULL,
            publishable_scale INTEGER NOT NULL CHECK(publishable_scale IN (0,1)),
            report_json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS shard (
            corpus_sha256 TEXT NOT NULL,
            sha256 TEXT NOT NULL,
            dataset_id TEXT NOT NULL,
            relative_path TEXT NOT NULL,
            events INTEGER NOT NULL CHECK(events > 0),
            sequence_gaps INTEGER NOT NULL CHECK(sequence_gaps >= 0),
            reconnect_events INTEGER NOT NULL CHECK(reconnect_events >= 0),
            started_at TEXT NOT NULL,
            ended_at TEXT NOT NULL,
            exchanges_json TEXT NOT NULL,
            pairs_json TEXT NOT NULL,
            PRIMARY KEY(corpus_sha256, sha256),
            FOREIGN KEY(corpus_sha256) REFERENCES corpus(corpus_sha256) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS exchange_count (
            corpus_sha256 TEXT NOT NULL,
            exchange TEXT NOT NULL,
            events INTEGER NOT NULL CHECK(events >= 0),
            PRIMARY KEY(corpus_sha256, exchange),
            FOREIGN KEY(corpus_sha256) REFERENCES corpus(corpus_sha256) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_shard_time ON shard(corpus_sha256, started_at, ended_at);
        CREATE INDEX IF NOT EXISTS idx_shard_dataset ON shard(dataset_id);
        CREATE INDEX IF NOT EXISTS idx_exchange_events ON exchange_count(corpus_sha256, events DESC);",
    )?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT OR REPLACE INTO corpus (
            corpus_sha256, classification, generated_at, unique_tapes,
            total_events, total_bytes, total_capture_duration_ms, earliest_event,
            latest_event, publishable_scale, report_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            &report.corpus_sha256,
            &report.classification,
            report.generated_at.to_rfc3339(),
            i64::try_from(report.unique_tapes)?,
            i64::try_from(report.total_events)?,
            i64::try_from(report.total_uncompressed_bytes)?,
            report.total_capture_duration_ms,
            report.earliest_event.to_rfc3339(),
            report.latest_event.to_rfc3339(),
            i64::from(report.evidence_gates.publishable_scale),
            serde_json::to_string(report)?,
        ],
    )?;
    transaction.execute(
        "DELETE FROM shard WHERE corpus_sha256 = ?1",
        params![&report.corpus_sha256],
    )?;
    transaction.execute(
        "DELETE FROM exchange_count WHERE corpus_sha256 = ?1",
        params![&report.corpus_sha256],
    )?;
    for shard in &report.tapes {
        let relative_path = shard.relative_path.to_string_lossy().into_owned();
        let exchanges_json = serde_json::to_string(&shard.exchanges)?;
        let pairs_json = serde_json::to_string(&shard.pairs)?;
        transaction.execute(
            "INSERT INTO shard (
                corpus_sha256, sha256, dataset_id, relative_path, events,
                sequence_gaps, reconnect_events, started_at, ended_at,
                exchanges_json, pairs_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                &report.corpus_sha256,
                &shard.sha256,
                &shard.dataset_id,
                relative_path,
                i64::try_from(shard.events)?,
                i64::try_from(shard.sequence_gaps)?,
                i64::try_from(shard.reconnect_events)?,
                shard.started_at.to_rfc3339(),
                shard.ended_at.to_rfc3339(),
                exchanges_json,
                pairs_json,
            ],
        )?;
    }
    for (exchange, events) in &report.events_by_exchange {
        transaction.execute(
            "INSERT INTO exchange_count (corpus_sha256, exchange, events) VALUES (?1, ?2, ?3)",
            params![&report.corpus_sha256, exchange, i64::try_from(*events)?],
        )?;
    }
    transaction.commit()?;
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CorpusScanReport {
    pub schema_version: u32,
    pub source_classification: String,
    pub corpus_sha256: String,
    pub cost_model_sha256: String,
    pub generated_at: DateTime<Utc>,
    pub processing_duration_ms: u64,
    pub events_per_second: f64,
    pub max_active_books: usize,
    pub max_levels_in_memory: usize,
    pub algorithm: &'static str,
    pub peak_state_policy: &'static str,
    pub raw_events: u64,
    pub valid_books: u64,
    pub comparable_candidates: u64,
    pub gross_dislocations: u64,
    pub net_dislocations: u64,
    pub liquid_net_dislocations: u64,
    pub gross_per_million_events: f64,
    pub net_per_million_events: f64,
    pub liquid_net_per_million_events: f64,
    pub gross_rate_95: RateEstimate,
    pub net_rate_95: RateEstimate,
    pub liquid_net_rate_95: RateEstimate,
    pub events_by_exchange: BTreeMap<String, u64>,
    pub rejected_by_cause: BTreeMap<String, u64>,
    pub definition: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RateEstimate {
    pub count: u64,
    pub denominator: u64,
    pub rate: f64,
    pub per_million: f64,
    pub lower_95: f64,
    pub upper_95: f64,
    pub lower_per_million_95: f64,
    pub upper_per_million_95: f64,
    pub method: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorpusEvidenceSeal {
    pub schema_version: u32,
    pub classification: String,
    pub generated_at: DateTime<Utc>,
    pub corpus_sha256: String,
    pub corpus_report_sha256: String,
    pub quantitative_scan_sha256: String,
    pub sqlite_index_sha256: String,
    pub cost_model_sha256: String,
    pub total_events: u64,
    pub net_dislocations: u64,
    pub liquid_net_dislocations: u64,
    pub statement: String,
}

pub fn seal_corpus_artifacts(
    root: &Path,
    corpus: &CorpusReport,
    scan: &CorpusScanReport,
) -> anyhow::Result<CorpusEvidenceSeal> {
    if corpus.classification != "public_market_capture_corpus"
        || scan.source_classification != "public_market_capture_corpus"
    {
        bail!("solo un corpus y scan públicos pueden generar sello de evidencia");
    }
    if corpus.corpus_sha256 != scan.corpus_sha256 {
        bail!("corpusSha256 del scan no coincide con el corpus");
    }
    let report_path = root.join("corpus.json");
    let scan_path = root.join("corpus-scan.json");
    let sqlite_path = root.join("corpus.sqlite");
    for path in [&report_path, &scan_path, &sqlite_path] {
        if !path.is_file() {
            bail!("falta artefacto para sello: {}", path.display());
        }
    }
    Ok(CorpusEvidenceSeal {
        schema_version: 1,
        classification: "public_market_evidence_seal".into(),
        generated_at: Utc::now(),
        corpus_sha256: corpus.corpus_sha256.clone(),
        corpus_report_sha256: file_sha(&report_path)?,
        quantitative_scan_sha256: file_sha(&scan_path)?,
        sqlite_index_sha256: file_sha(&sqlite_path)?,
        cost_model_sha256: scan.cost_model_sha256.clone(),
        total_events: corpus.total_events,
        net_dislocations: scan.net_dislocations,
        liquid_net_dislocations: scan.liquid_net_dislocations,
        statement: "hashes encadenan corpus verificado, scan streaming y su índice reconstruible; no demuestran rentabilidad real".into(),
    })
}

pub fn verify_corpus_evidence_seal(root: &Path) -> anyhow::Result<CorpusEvidenceSeal> {
    let seal_path = root.join("evidence-seal.json");
    let seal: CorpusEvidenceSeal = serde_json::from_slice(&fs::read(&seal_path)?)?;
    if seal.classification != "public_market_evidence_seal" {
        bail!("clasificación de sello inválida");
    }
    for (path, expected) in [
        (root.join("corpus.json"), seal.corpus_report_sha256.as_str()),
        (
            root.join("corpus-scan.json"),
            seal.quantitative_scan_sha256.as_str(),
        ),
        (
            root.join("corpus.sqlite"),
            seal.sqlite_index_sha256.as_str(),
        ),
    ] {
        let actual = file_sha(&path)?;
        if actual != expected {
            bail!("sello no coincide para {}", path.display());
        }
    }
    Ok(seal)
}

/// Escanea un corpus verificado en una sola pasada y memoria acotada al número
/// de libros activos. No entrena ni selecciona estrategias: produce únicamente
/// el embudo observable de mercado bajo el modelo de costos configurado.
pub fn scan_corpus_streaming(root: &Path, costs: &MapaCostos) -> anyhow::Result<CorpusScanReport> {
    scan_corpus_streaming_classified(root, costs, &["public_market_capture"])
}

/// Variante exclusiva para benchmarks sintéticos. El corpus resultante queda
/// clasificado como `synthetic_benchmark_corpus` y no satisface gates públicos.
pub fn scan_synthetic_benchmark_corpus(
    root: &Path,
    costs: &MapaCostos,
) -> anyhow::Result<CorpusScanReport> {
    scan_corpus_streaming_classified(root, costs, &["synthetic_benchmark"])
}

fn scan_corpus_streaming_classified(
    root: &Path,
    costs: &MapaCostos,
    allowed_classifications: &[&str],
) -> anyhow::Result<CorpusScanReport> {
    let scan_started = std::time::Instant::now();
    let corpus = verify_corpus_classified(root, allowed_classifications)?;
    let mut books = HashMap::<(String, String), BookSides>::new();
    let mut latest = HashMap::<(String, String), Cotizacion>::new();
    let mut raw_events = 0_u64;
    let mut valid_books = 0_u64;
    let mut comparable_candidates = 0_u64;
    let mut gross_dislocations = 0_u64;
    let mut net_dislocations = 0_u64;
    let mut liquid_net_dislocations = 0_u64;
    let mut events_by_exchange = BTreeMap::<String, u64>::new();
    let mut rejected_by_cause = BTreeMap::<String, u64>::new();
    let mut max_active_books = 0_usize;
    let mut max_levels_in_memory = 0_usize;

    for shard in &corpus.tapes {
        let events_path = root.join(&shard.relative_path).join(EVENTS_FILE);
        for (line_no, line) in BufReader::new(File::open(&events_path)?)
            .lines()
            .enumerate()
        {
            let event: TapeEvent = serde_json::from_str(&line?)
                .with_context(|| format!("{}:{}", events_path.display(), line_no + 1))?;
            raw_events = raw_events.checked_add(1).context("overflow de eventos")?;
            *events_by_exchange
                .entry(event.exchange.clone())
                .or_default() += 1;
            let key = (event.exchange.clone(), event.pair.clone());
            let (best_bid, best_ask) = {
                let book = books.entry(key.clone()).or_default();
                if event.kind == EventKind::Snapshot {
                    book.0.clear();
                    book.1.clear();
                }
                apply(&mut book.0, &event.bids);
                apply(&mut book.1, &event.asks);
                trim_scan_book(&mut book.0, &mut book.1, 50);
                (
                    book.0
                        .last_key_value()
                        .map(|(&price, &quantity)| (price, quantity)),
                    book.1
                        .first_key_value()
                        .map(|(&price, &quantity)| (price, quantity)),
                )
            };
            max_active_books = max_active_books.max(books.len());
            max_levels_in_memory = max_levels_in_memory.max(
                books
                    .values()
                    .map(|(bids, asks)| bids.len() + asks.len())
                    .sum(),
            );
            let Some((bid_scaled, bid_quantity)) = best_bid else {
                *rejected_by_cause.entry("libro_sin_bid".into()).or_default() += 1;
                continue;
            };
            let Some((ask_scaled, ask_quantity)) = best_ask else {
                *rejected_by_cause.entry("libro_sin_ask".into()).or_default() += 1;
                continue;
            };
            let bid = bid_scaled as f64 / 100_000_000.0;
            let ask = ask_scaled as f64 / 100_000_000.0;
            if bid <= 0.0 || ask <= 0.0 || bid >= ask {
                *rejected_by_cause.entry("bbo_invalido".into()).or_default() += 1;
                continue;
            }
            valid_books += 1;
            let timestamp_confiable = event.exchange_timestamp.is_some();
            let event_time = event.exchange_timestamp.unwrap_or(event.local_timestamp);
            latest.insert(
                key,
                Cotizacion {
                    exchange: event.exchange.clone(),
                    par: event.pair.clone(),
                    bid,
                    bid_cantidad: bid_quantity,
                    ask,
                    ask_cantidad: ask_quantity,
                    bids: Vec::<NivelOrden>::new().into(),
                    asks: Vec::<NivelOrden>::new().into(),
                    evento_unix_ms: event_time.timestamp_millis(),
                    recibida_en: event.local_timestamp,
                    latencia_ms: event.observed_latency_ms.unwrap_or(0).min(i64::MAX as u64) as i64,
                    secuencia: event.sequence_id.unwrap_or(raw_events),
                    exchange_sequence: event.sequence_id,
                    integrity_status: event.integrity.status.clone(),
                    resyncs: u64::from(event.integrity.resync),
                    sequence_gaps: u64::from(event.integrity.gap),
                    checksum_failures: 0,
                    invalidated_ms: 0,
                    timestamp_confiable,
                    conectado: true,
                    ultimo_mensaje: String::new(),
                },
            );
            let candidates = latest
                .values()
                .filter(|quote| {
                    quote.par == event.pair
                        && (event.local_timestamp - quote.recibida_en)
                            .num_milliseconds()
                            .abs()
                            <= costs.stale_ms
                })
                .collect::<Vec<_>>();
            let Some(buy) = candidates
                .iter()
                .copied()
                .min_by(|a, b| a.ask.total_cmp(&b.ask))
            else {
                continue;
            };
            let Some(sell) = candidates
                .iter()
                .copied()
                .filter(|quote| quote.exchange != buy.exchange)
                .max_by(|a, b| a.bid.total_cmp(&b.bid))
            else {
                *rejected_by_cause
                    .entry("sin_segundo_venue".into())
                    .or_default() += 1;
                continue;
            };
            comparable_candidates += 1;
            let mid = (buy.ask + sell.bid) / 2.0;
            let gross_bps = (sell.bid - buy.ask) / mid * 10_000.0;
            if gross_bps <= 0.0 {
                continue;
            }
            gross_dislocations += 1;
            let available = buy.ask_cantidad.min(sell.bid_cantidad).max(0.0);
            let quantity = available.min(costs.max_operacion_btc);
            let cost = calcular_costos_canonicos(
                quantity,
                buy,
                sell,
                buy.latencia_ms.max(sell.latencia_ms),
                costs,
            );
            let capital = buy.ask * quantity;
            let cost_bps = if capital > 0.0 {
                cost.total_usd / capital * 10_000.0
            } else {
                f64::INFINITY
            };
            if gross_bps <= cost_bps {
                continue;
            }
            net_dislocations += 1;
            if available > 0.0 {
                liquid_net_dislocations += 1;
            }
        }
    }
    if raw_events != corpus.total_events {
        bail!(
            "scan leyó {raw_events} eventos pero corpus declara {}",
            corpus.total_events
        );
    }
    let per_million = |count: u64| count as f64 / raw_events.max(1) as f64 * 1_000_000.0;
    let elapsed = scan_started.elapsed();
    let processing_duration_ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64;
    let events_per_second = raw_events as f64 / elapsed.as_secs_f64().max(1e-9);
    Ok(CorpusScanReport {
        schema_version: 1,
        source_classification: corpus.classification,
        corpus_sha256: corpus.corpus_sha256,
        cost_model_sha256: cost_model_sha256(costs),
        generated_at: Utc::now(),
        processing_duration_ms,
        events_per_second,
        max_active_books,
        max_levels_in_memory,
        algorithm: "single_pass_best_cross_venue_v1",
        peak_state_policy: "un libro reconstruido por exchange/par; memoria O(venues*pairs*depth)",
        raw_events,
        valid_books,
        comparable_candidates,
        gross_dislocations,
        net_dislocations,
        liquid_net_dislocations,
        gross_per_million_events: per_million(gross_dislocations),
        net_per_million_events: per_million(net_dislocations),
        liquid_net_per_million_events: per_million(liquid_net_dislocations),
        gross_rate_95: wilson_rate_95(gross_dislocations, raw_events),
        net_rate_95: wilson_rate_95(net_dislocations, raw_events),
        liquid_net_rate_95: wilson_rate_95(liquid_net_dislocations, raw_events),
        events_by_exchange,
        rejected_by_cause,
        definition: "por evento se evalúa el mejor candidato fresco cross-venue; neto exige superar costos canónicos; liquid_net exige cantidad positiva",
    })
}

fn wilson_rate_95(count: u64, denominator: u64) -> RateEstimate {
    if denominator == 0 {
        return RateEstimate {
            count,
            denominator,
            rate: 0.0,
            per_million: 0.0,
            lower_95: 0.0,
            upper_95: 1.0,
            lower_per_million_95: 0.0,
            upper_per_million_95: 1_000_000.0,
            method: "wilson_score_z_1.96",
        };
    }
    let n = denominator as f64;
    let successes = count.min(denominator) as f64;
    let p = successes / n;
    let z = 1.96_f64;
    let z2 = z * z;
    let denominator_wilson = 1.0 + z2 / n;
    let center = (p + z2 / (2.0 * n)) / denominator_wilson;
    let margin = z * ((p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt()) / denominator_wilson;
    let lower = (center - margin).clamp(0.0, 1.0);
    let upper = (center + margin).clamp(0.0, 1.0);
    RateEstimate {
        count,
        denominator,
        rate: p,
        per_million: p * 1_000_000.0,
        lower_95: lower,
        upper_95: upper,
        lower_per_million_95: lower * 1_000_000.0,
        upper_per_million_95: upper * 1_000_000.0,
        method: "wilson_score_z_1.96",
    }
}

fn trim_scan_book(bids: &mut BTreeMap<i64, f64>, asks: &mut BTreeMap<i64, f64>, depth: usize) {
    while bids.len() > depth {
        if let Some(lowest) = bids.first_key_value().map(|(price, _)| *price) {
            bids.remove(&lowest);
        }
    }
    while asks.len() > depth {
        if let Some(highest) = asks.last_key_value().map(|(price, _)| *price) {
            asks.remove(&highest);
        }
    }
}

fn cost_model_sha256(costs: &MapaCostos) -> String {
    let mut exchanges = BTreeMap::new();
    for (name, config) in &costs.exchanges {
        exchanges.insert(name, config);
    }
    let value = serde_json::json!({
        "maxOperacionBtc": costs.max_operacion_btc,
        "minUtilidadUsd": costs.min_utilidad_usd,
        "minDiferencialNetoBps": costs.min_diferencial_neto_bps,
        "deslizamientoBps": costs.deslizamiento_bps,
        "latenciaRiesgoBps": costs.latencia_riesgo_bps,
        "retiroAmortizadoBps": costs.retiro_amortizado_bps,
        "usdtUsdPremiumBps": costs.usdt_usd_premium_bps,
        "permitirCruceUsdUsdt": costs.permitir_cruce_usd_usdt,
        "staleMs": costs.stale_ms,
        "exchanges": exchanges,
    });
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&value).unwrap_or_default())
    )
}

fn markets_overlap(left: &Verification, right: &Verification) -> bool {
    if right.started_at >= left.ended_at || left.started_at >= right.ended_at {
        return false;
    }
    let same_exchange = left
        .exchanges
        .iter()
        .any(|exchange| right.exchanges.contains(exchange));
    let same_pair = left.pairs.iter().any(|pair| right.pairs.contains(pair));
    same_exchange && same_pair
}

pub fn is_corpus_shard(path: &Path) -> bool {
    let quarantined = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("failed-"));
    path.is_dir() && !quarantined && path.join(MANIFEST_FILE).is_file()
}

fn corpus_sha256(hashes: &[String]) -> String {
    let mut hasher = Sha256::new();
    for hash in hashes {
        hasher.update(hash.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

pub fn verify(path: &Path) -> anyhow::Result<Verification> {
    let manifest: TapeManifest = serde_json::from_slice(
        &fs::read(path.join(MANIFEST_FILE)).context("falta manifest.json")?,
    )?;
    if manifest.schema_version != 1 {
        bail!("schemaVersion de manifiesto no soportada");
    }
    let config_bytes = fs::read(path.join(CONFIG_FILE)).context("falta capture-config.json")?;
    if hex_sha(&config_bytes) != manifest.config_sha256 {
        bail!("configSha256 no coincide");
    }
    let actual_sha = file_sha(&path.join(EVENTS_FILE))?;
    if actual_sha != manifest.sha256 {
        bail!("sha256 de events.jsonl no coincide");
    }
    let mut books: HashMap<(String, String), BookSides> = HashMap::new();
    let mut last_local = None;
    let mut last_seq: HashMap<(String, String), u64> = HashMap::new();
    let mut count = 0;
    let mut snapshots = 0;
    let mut gaps = 0;
    let mut rest = 0;
    let mut reconnects = 0;
    let mut events_by_exchange = BTreeMap::<String, u64>::new();
    for (line_no, line) in BufReader::new(File::open(path.join(EVENTS_FILE))?)
        .lines()
        .enumerate()
    {
        let event: TapeEvent = serde_json::from_str(&line?)
            .with_context(|| format!("evento inválido en línea {}", line_no + 1))?;
        if event.schema_version != 1 {
            bail!("schemaVersion inválida en línea {}", line_no + 1);
        }
        if last_local.is_some_and(|v| event.local_timestamp < v) {
            bail!("orden temporal inválido en línea {}", line_no + 1);
        }
        last_local = Some(event.local_timestamp);
        let key = (event.exchange.clone(), event.pair.clone());
        if let Some(seq) = event.sequence_id {
            if !event.integrity.resync && last_seq.get(&key).is_some_and(|old| seq < *old) {
                bail!("secuencia no monótona en línea {}", line_no + 1);
            }
            last_seq.insert(key.clone(), seq);
        }
        validate_levels(&event.bids, &event.asks, line_no + 1)?;
        let book = books.entry(key).or_default();
        if event.kind == EventKind::Snapshot {
            book.0.clear();
            book.1.clear();
            snapshots += 1;
        }
        apply(&mut book.0, &event.bids);
        apply(&mut book.1, &event.asks);
        if book.0.is_empty() || book.1.is_empty() {
            bail!("libro no reconstruible en línea {}", line_no + 1);
        }
        count += 1;
        gaps += u64::from(event.integrity.gap);
        rest += u64::from(matches!(event.source, TapeSource::Rest { .. }));
        reconnects += u64::from(event.integrity.reconnected);
        *events_by_exchange.entry(event.exchange).or_default() += 1;
    }
    if count != manifest.events
        || snapshots != manifest.snapshots
        || gaps != manifest.sequence_gaps
        || rest != manifest.rest_fallback_events
        || reconnects != manifest.reconnect_events
    {
        bail!("conteos del manifiesto no coinciden con el tape");
    }
    if !manifest.events_by_exchange.is_empty() && events_by_exchange != manifest.events_by_exchange
    {
        bail!("eventsByExchange del manifiesto no coincide con el tape");
    }
    let actual_bytes = fs::metadata(path.join(EVENTS_FILE))?.len();
    if manifest.uncompressed_bytes > 0 && actual_bytes != manifest.uncompressed_bytes {
        bail!("uncompressedBytes del manifiesto no coincide con events.jsonl");
    }
    if last_local.is_none_or(|v| v < manifest.started_at || v > manifest.ended_at) {
        bail!("ventana temporal del manifiesto no contiene los eventos");
    }
    Ok(Verification {
        path: path.to_path_buf(),
        dataset_id: if manifest.dataset_id.is_empty() {
            format!("legacy-{}", &actual_sha[..12])
        } else {
            manifest.dataset_id
        },
        source_classification: manifest.source_classification,
        events: count,
        snapshots,
        sequence_gaps: gaps,
        rest_fallback_events: rest,
        reconnect_events: reconnects,
        delivery_policy: manifest.delivery_policy,
        books_reconstructed: books.len(),
        exchanges: manifest.exchanges,
        pairs: manifest.pairs,
        started_at: manifest.started_at,
        ended_at: manifest.ended_at,
        duration_ms: (manifest.ended_at - manifest.started_at)
            .num_milliseconds()
            .max(0),
        uncompressed_bytes: actual_bytes,
        events_by_exchange,
        sha256: actual_sha,
    })
}

fn default_source_classification() -> String {
    "public_market_capture".to_string()
}

fn default_delivery_policy() -> String {
    "bounded_channel_await_no_application_drop".to_string()
}

fn validate_levels(bids: &[NivelOrden], asks: &[NivelOrden], line: usize) -> anyhow::Result<()> {
    if bids.len() > 50
        || asks.len() > 50
        || bids.iter().chain(asks).any(|n| {
            !n.precio.is_finite() || !n.cantidad.is_finite() || n.precio <= 0.0 || n.cantidad < 0.0
        })
    {
        bail!("niveles inválidos en línea {line}");
    }
    if bids.windows(2).any(|w| w[0].precio < w[1].precio)
        || asks.windows(2).any(|w| w[0].precio > w[1].precio)
    {
        bail!("niveles desordenados en línea {line}");
    }
    Ok(())
}
fn apply(book: &mut BTreeMap<i64, f64>, levels: &[NivelOrden]) {
    for n in levels {
        let p = (n.precio * 100_000_000.0).round() as i64;
        if n.cantidad == 0.0 {
            book.remove(&p);
        } else {
            book.insert(p, n.cantidad);
        }
    }
}
fn hex_sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn file_sha(path: &Path) -> anyhow::Result<String> {
    Ok(hex_sha(&fs::read(path)?))
}
fn git_commit() -> String {
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

pub fn parse_duration(value: &str) -> anyhow::Result<Duration> {
    let split = value
        .find(|c: char| !c.is_ascii_digit())
        .context("duración inválida (ej. 6h, 30m, 10s)")?;
    let n: u64 = value[..split].parse()?;
    let unit = &value[split..];
    match unit {
        "h" => Ok(Duration::from_secs(n * 3600)),
        "m" => Ok(Duration::from_secs(n * 60)),
        "s" => Ok(Duration::from_secs(n)),
        _ => bail!("unidad de duración inválida: {unit}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verified_window(
        id: &str,
        start_ms: i64,
        end_ms: i64,
        exchange: &str,
        pair: &str,
    ) -> Verification {
        Verification {
            path: PathBuf::from(id),
            dataset_id: id.into(),
            source_classification: "public_market_capture".into(),
            events: 1,
            snapshots: 1,
            sequence_gaps: 0,
            rest_fallback_events: 0,
            reconnect_events: 0,
            delivery_policy: default_delivery_policy(),
            books_reconstructed: 1,
            exchanges: vec![exchange.into()],
            pairs: vec![pair.into()],
            started_at: DateTime::from_timestamp_millis(start_ms).unwrap(),
            ended_at: DateTime::from_timestamp_millis(end_ms).unwrap(),
            duration_ms: end_ms - start_ms,
            uncompressed_bytes: 1,
            events_by_exchange: BTreeMap::from([(exchange.into(), 1)]),
            sha256: id.into(),
        }
    }

    #[test]
    fn duration_units() {
        assert_eq!(parse_duration("6h").unwrap(), Duration::from_secs(21600));
        assert!(parse_duration("6x").is_err());
    }

    #[test]
    fn duration_rejects_missing_or_partial_units() {
        assert!(parse_duration("600").is_err());
        assert!(parse_duration("s").is_err());
        assert!(parse_duration("1.5h").is_err());
        assert!(parse_duration("10ms").is_err());
    }

    #[test]
    fn levels_reject_non_finite_negative_and_unsorted_books() {
        let valid_bid = NivelOrden {
            precio: 100.0,
            cantidad: 1.0,
        };
        let valid_ask = NivelOrden {
            precio: 101.0,
            cantidad: 1.0,
        };
        assert!(validate_levels(
            &[NivelOrden {
                precio: f64::NAN,
                cantidad: 1.0
            }],
            std::slice::from_ref(&valid_ask),
            1
        )
        .is_err());
        assert!(validate_levels(
            &[NivelOrden {
                precio: 100.0,
                cantidad: -0.1
            }],
            std::slice::from_ref(&valid_ask),
            1
        )
        .is_err());
        assert!(validate_levels(
            &[
                valid_bid.clone(),
                NivelOrden {
                    precio: 101.0,
                    cantidad: 1.0
                }
            ],
            std::slice::from_ref(&valid_ask),
            1
        )
        .is_err());
    }

    #[test]
    fn apply_zero_quantity_removes_level_deterministically() {
        let mut side = BTreeMap::new();
        apply(
            &mut side,
            &[NivelOrden {
                precio: 100.0,
                cantidad: 2.5,
            }],
        );
        assert_eq!(side.len(), 1);
        apply(
            &mut side,
            &[NivelOrden {
                precio: 100.0,
                cantidad: 0.0,
            }],
        );
        assert!(side.is_empty());
    }

    #[test]
    fn legacy_manifest_gets_explicit_public_market_classification() {
        let raw = serde_json::json!({
            "schemaVersion": 1,
            "startedAt": "2026-01-01T00:00:00Z",
            "endedAt": "2026-01-01T00:00:01Z",
            "exchanges": ["Kraken"],
            "pairs": ["BTC/USD"],
            "events": 1,
            "snapshots": 1,
            "sequenceGaps": 0,
            "restFallbackEvents": 0,
            "sha256": "abc",
            "gitCommit": "deadbeef",
            "configSha256": "def"
        });
        let manifest: TapeManifest = serde_json::from_value(raw).unwrap();
        assert_eq!(manifest.source_classification, "public_market_capture");
        assert_eq!(
            manifest.delivery_policy,
            "bounded_channel_await_no_application_drop"
        );
        assert_eq!(manifest.reconnect_events, 0);
        assert!(manifest.dataset_id.is_empty());
        assert!(manifest.events_by_exchange.is_empty());
    }

    #[test]
    fn legacy_integrity_state_defaults_connection_epoch_without_hiding_gap() {
        let state: IntegrityState = serde_json::from_value(serde_json::json!({
            "status": "snapshot",
            "gap": true,
            "resync": true
        }))
        .unwrap();
        assert!(state.gap);
        assert!(state.resync);
        assert_eq!(state.connection_epoch, 0);
        assert!(!state.reconnected);
    }

    #[test]
    fn corpus_digest_is_deterministic_for_the_same_ordered_shards() {
        let hashes = vec!["aaa".to_string(), "bbb".to_string()];
        assert_eq!(corpus_sha256(&hashes), corpus_sha256(&hashes));
        assert_eq!(corpus_sha256(&[]), corpus_sha256(&[]));
    }

    #[test]
    fn corpus_digest_changes_when_a_shard_or_order_changes() {
        let original = vec!["aaa".to_string(), "bbb".to_string()];
        let changed = vec!["aaa".to_string(), "bbc".to_string()];
        let reversed = vec!["bbb".to_string(), "aaa".to_string()];
        assert_ne!(corpus_sha256(&original), corpus_sha256(&changed));
        assert_ne!(corpus_sha256(&original), corpus_sha256(&reversed));
    }

    #[test]
    fn corpus_never_accepts_quarantined_shard_even_with_manifest() {
        let root =
            std::env::temp_dir().join(format!("mayab-corpus-quarantine-{}", std::process::id()));
        let shard = root.join("failed-shard-000001");
        fs::create_dir_all(&shard).unwrap();
        fs::write(shard.join(MANIFEST_FILE), b"{}").unwrap();
        assert!(!is_corpus_shard(&shard));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn corpus_accepts_regular_directory_only_when_manifest_exists() {
        let root =
            std::env::temp_dir().join(format!("mayab-corpus-regular-{}", std::process::id()));
        let shard = root.join("shard-000001");
        fs::create_dir_all(&shard).unwrap();
        assert!(!is_corpus_shard(&shard));
        fs::write(shard.join(MANIFEST_FILE), b"{}").unwrap();
        assert!(is_corpus_shard(&shard));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn corpus_detects_overlapping_windows_for_the_same_market() {
        let first = verified_window("one", 1_000, 2_000, "Kraken", "BTC/USD");
        let overlap = verified_window("two", 1_500, 2_500, "Kraken", "BTC/USD");
        assert!(markets_overlap(&first, &overlap));
    }

    #[test]
    fn corpus_allows_touching_windows_or_distinct_markets() {
        let first = verified_window("one", 1_000, 2_000, "Kraken", "BTC/USD");
        let touching = verified_window("two", 2_000, 3_000, "Kraken", "BTC/USD");
        let other_pair = verified_window("three", 1_500, 2_500, "Kraken", "ETH/USD");
        let other_exchange = verified_window("four", 1_500, 2_500, "Coinbase", "BTC/USD");
        assert!(!markets_overlap(&first, &touching));
        assert!(!markets_overlap(&first, &other_pair));
        assert!(!markets_overlap(&first, &other_exchange));
    }

    #[test]
    fn sqlite_corpus_index_is_transactional_queryable_and_idempotent() {
        let root = std::env::temp_dir().join(format!("mayab-corpus-index-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let start = DateTime::<Utc>::from_timestamp_millis(1_000).unwrap();
        let end = DateTime::<Utc>::from_timestamp_millis(2_000).unwrap();
        let report = CorpusReport {
            schema_version: 1,
            classification: "public_market_capture_corpus".into(),
            corpus_sha256: "corpus-fixture".into(),
            generated_at: Utc::now(),
            root: root.clone(),
            unique_tapes: 1,
            total_events: 10,
            total_sequence_gaps: 0,
            total_reconnect_events: 0,
            total_rest_fallback_events: 0,
            total_uncompressed_bytes: 1_024,
            earliest_event: start,
            latest_event: end,
            observed_span_ms: 1_000,
            total_capture_duration_ms: 1_000,
            events_by_exchange: BTreeMap::from([("Kraken".into(), 10)]),
            pairs: vec!["BTC/USD".into()],
            evidence_gates: EvidenceGates {
                multi_venue: false,
                minimum_ten_shards: false,
                preliminary_100k_events: false,
                million_event_scale: false,
                twenty_four_captured_hours: false,
                delivery_is_loss_accounted: true,
                sequence_gap_rate_below_one_percent: true,
                publishable_scale: false,
                status: "insufficient_scale",
                note: "fixture",
            },
            tapes: vec![CorpusTape {
                dataset_id: "dataset-fixture".into(),
                relative_path: PathBuf::from("shard-000001"),
                events: 10,
                sequence_gaps: 0,
                reconnect_events: 0,
                started_at: start,
                ended_at: end,
                exchanges: vec!["Kraken".into()],
                pairs: vec!["BTC/USD".into()],
                sha256: "shard-fixture".into(),
            }],
        };
        let database = root.join("corpus.sqlite");
        index_corpus_sqlite(&report, &database).unwrap();
        index_corpus_sqlite(&report, &database).unwrap();
        let connection = Connection::open(&database).unwrap();
        let corpus_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM corpus", [], |row| row.get(0))
            .unwrap();
        let shard_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM shard", [], |row| row.get(0))
            .unwrap();
        let indexed_events: i64 = connection
            .query_row("SELECT total_events FROM corpus", [], |row| row.get(0))
            .unwrap();
        assert_eq!((corpus_count, shard_count, indexed_events), (1, 1, 10));
        drop(connection);
        fs::write(
            root.join("corpus.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
        let scan = CorpusScanReport {
            schema_version: 1,
            source_classification: "public_market_capture_corpus".into(),
            corpus_sha256: report.corpus_sha256.clone(),
            cost_model_sha256: "cost-fixture".into(),
            generated_at: Utc::now(),
            processing_duration_ms: 1,
            events_per_second: 10_000.0,
            max_active_books: 1,
            max_levels_in_memory: 2,
            algorithm: "fixture",
            peak_state_policy: "fixture",
            raw_events: 10,
            valid_books: 10,
            comparable_candidates: 8,
            gross_dislocations: 2,
            net_dislocations: 1,
            liquid_net_dislocations: 1,
            gross_per_million_events: 200_000.0,
            net_per_million_events: 100_000.0,
            liquid_net_per_million_events: 100_000.0,
            gross_rate_95: wilson_rate_95(2, 10),
            net_rate_95: wilson_rate_95(1, 10),
            liquid_net_rate_95: wilson_rate_95(1, 10),
            events_by_exchange: BTreeMap::from([("Kraken".into(), 10)]),
            rejected_by_cause: BTreeMap::new(),
            definition: "fixture",
        };
        fs::write(
            root.join("corpus-scan.json"),
            serde_json::to_vec_pretty(&scan).unwrap(),
        )
        .unwrap();
        let seal = seal_corpus_artifacts(&root, &report, &scan).unwrap();
        fs::write(
            root.join("evidence-seal.json"),
            serde_json::to_vec_pretty(&seal).unwrap(),
        )
        .unwrap();
        assert!(verify_corpus_evidence_seal(&root).is_ok());
        fs::write(root.join("corpus-scan.json"), b"{}").unwrap();
        assert!(verify_corpus_evidence_seal(&root).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn streaming_scan_trims_books_to_best_bids_and_asks() {
        let mut bids = (1..=100).map(|price| (price, 1.0)).collect();
        let mut asks = (101..=200).map(|price| (price, 1.0)).collect();
        trim_scan_book(&mut bids, &mut asks, 10);
        assert_eq!(bids.len(), 10);
        assert_eq!(asks.len(), 10);
        assert_eq!(bids.first_key_value().map(|(price, _)| *price), Some(91));
        assert_eq!(asks.last_key_value().map(|(price, _)| *price), Some(110));
    }

    #[test]
    fn streaming_scan_cost_hash_ignores_exchange_insertion_order() {
        let exchange = |name: &str| crate::types::ExchangeConfig {
            nombre: name.into(),
            fee_taker: 0.001,
            retiro_btc: 0.0001,
            confiabilidad: 0.99,
        };
        let mut first = MapaCostos::default();
        first.exchanges.insert("Kraken".into(), exchange("Kraken"));
        first
            .exchanges
            .insert("Binance".into(), exchange("Binance"));
        let mut second = MapaCostos::default();
        second
            .exchanges
            .insert("Binance".into(), exchange("Binance"));
        second.exchanges.insert("Kraken".into(), exchange("Kraken"));
        assert_eq!(cost_model_sha256(&first), cost_model_sha256(&second));
    }

    #[test]
    fn synthetic_corpus_can_never_satisfy_publication_gate() {
        assert_eq!(
            corpus_publication_status("synthetic_benchmark_corpus", true),
            (false, "synthetic_only")
        );
        assert_eq!(
            corpus_publication_status("public_market_capture_corpus", true),
            (true, "scale_verified")
        );
        assert_eq!(
            corpus_publication_status("public_market_capture_corpus", false),
            (false, "insufficient_scale")
        );
    }

    #[test]
    fn atomic_json_publish_replaces_complete_document_without_temp_leaks() {
        let root = std::env::temp_dir().join(format!("mayab-atomic-json-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("artifact.json");
        write_json_atomic(&path, &serde_json::json!({"version": 1})).unwrap();
        write_json_atomic(&path, &serde_json::json!({"version": 2})).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["version"], 2);
        assert_eq!(
            fs::read_dir(&root)
                .unwrap()
                .filter_map(std::result::Result::ok)
                .count(),
            1
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn wilson_interval_contains_observed_dislocation_rate() {
        let estimate = wilson_rate_95(42, 1_000);
        assert!(estimate.lower_95 <= estimate.rate);
        assert!(estimate.rate <= estimate.upper_95);
        assert_eq!(estimate.count, 42);
        assert_eq!(estimate.denominator, 1_000);
        assert!(estimate.lower_95 >= 0.0 && estimate.upper_95 <= 1.0);
    }

    #[test]
    fn wilson_interval_handles_zero_and_full_counts_without_nan() {
        for estimate in [wilson_rate_95(0, 100), wilson_rate_95(100, 100)] {
            assert!(estimate.rate.is_finite());
            assert!(estimate.lower_95.is_finite());
            assert!(estimate.upper_95.is_finite());
            assert!(estimate.lower_95 <= estimate.upper_95);
        }
        let empty = wilson_rate_95(0, 0);
        assert_eq!((empty.lower_95, empty.upper_95), (0.0, 1.0));
    }
}
