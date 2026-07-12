use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use mayab_arbitrage::{
    config::Config,
    tape::{
        scan_synthetic_benchmark_corpus, CaptureConfig, EventKind, IntegrityState, TapeEvent,
        TapeManifest, TapeSource, CONFIG_FILE, EVENTS_FILE, MANIFEST_FILE,
    },
    types::NivelOrden,
};
use sha2::{Digest, Sha256};

const EVENTS: u64 = 100_000;

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn fixture_root() -> PathBuf {
    std::env::temp_dir().join(format!("mayab-corpus-scan-bench-{}", std::process::id()))
}

fn build_fixture(root: &Path) {
    let _ = fs::remove_dir_all(root);
    let shard = root.join("shard-000001-benchmark");
    fs::create_dir_all(&shard).unwrap();
    let config = CaptureConfig {
        schema_version: 1,
        pair: "BTC/USD".into(),
        exchanges: vec![
            "Binance".into(),
            "Kraken".into(),
            "Coinbase".into(),
            "OKX".into(),
        ],
        depth: 10,
    };
    let config_bytes = serde_json::to_vec_pretty(&config).unwrap();
    fs::write(shard.join(CONFIG_FILE), &config_bytes).unwrap();
    let start = DateTime::<Utc>::from_timestamp_millis(1_700_000_000_000).unwrap();
    let exchanges = ["Binance", "Kraken", "Coinbase", "OKX"];
    let file = File::create(shard.join(EVENTS_FILE)).unwrap();
    let mut writer = BufWriter::new(file);
    for index in 0..EVENTS {
        let venue_index = index as usize % exchanges.len();
        let exchange = exchanges[venue_index];
        let timestamp = start + chrono::Duration::milliseconds(index as i64);
        let base = 60_000.0 + venue_index as f64 * 2.0 + (index % 17) as f64 * 0.01;
        let event = TapeEvent {
            schema_version: 1,
            exchange_timestamp: Some(timestamp),
            local_timestamp: timestamp + chrono::Duration::milliseconds(2),
            exchange: exchange.into(),
            pair: "BTC/USD".into(),
            source: TapeSource::WebSocket {
                url: "wss://synthetic.benchmark.invalid".into(),
            },
            kind: EventKind::Snapshot,
            sequence_id: Some(index + 1),
            previous_sequence: index.checked_sub(1),
            bids: vec![NivelOrden {
                precio: base,
                cantidad: 1.0,
            }],
            asks: vec![NivelOrden {
                precio: base + 1.0,
                cantidad: 1.0,
            }],
            integrity: IntegrityState {
                status: "synthetic_benchmark_snapshot".into(),
                gap: false,
                resync: false,
                connection_epoch: 0,
                reconnected: false,
            },
            observed_latency_ms: Some(2),
        };
        serde_json::to_writer(&mut writer, &event).unwrap();
        writer.write_all(b"\n").unwrap();
    }
    writer.flush().unwrap();
    let events_bytes = fs::read(shard.join(EVENTS_FILE)).unwrap();
    let end = start + chrono::Duration::milliseconds(EVENTS as i64 + 2);
    let events_by_exchange = exchanges
        .into_iter()
        .map(|exchange| (exchange.to_string(), EVENTS / 4))
        .collect::<BTreeMap<_, _>>();
    let manifest = TapeManifest {
        schema_version: 1,
        dataset_id: "synthetic-benchmark-100k".into(),
        source_classification: "synthetic_benchmark".into(),
        started_at: start,
        ended_at: end,
        exchanges: config.exchanges,
        pairs: vec!["BTC/USD".into()],
        events: EVENTS,
        snapshots: EVENTS,
        sequence_gaps: 0,
        rest_fallback_events: 0,
        reconnect_events: 0,
        delivery_policy: "bounded_channel_await_no_application_drop".into(),
        events_by_exchange,
        uncompressed_bytes: events_bytes.len() as u64,
        duration_ms: (end - start).num_milliseconds(),
        sha256: sha256(&events_bytes),
        git_commit: "synthetic-benchmark".into(),
        config_sha256: sha256(&config_bytes),
    };
    fs::write(
        shard.join(MANIFEST_FILE),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
}

fn corpus_scan(c: &mut Criterion) {
    let root = fixture_root();
    build_fixture(&root);
    let costs = Config::from_env().costos;
    let mut group = c.benchmark_group("corpus_scan_streaming");
    group.sample_size(10);
    group.throughput(Throughput::Elements(EVENTS));
    group.bench_function("verify_reconstruct_cost_funnel_100k", |b| {
        b.iter(|| {
            let report =
                scan_synthetic_benchmark_corpus(black_box(&root), black_box(&costs)).unwrap();
            black_box(report.raw_events)
        })
    });
    group.finish();
    let _ = fs::remove_dir_all(root);
}

criterion_group!(benches, corpus_scan);
criterion_main!(benches);
