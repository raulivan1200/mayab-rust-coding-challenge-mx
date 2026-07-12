use std::{fs, path::PathBuf, time::Duration};

use anyhow::{bail, Context, Result};
use chrono::Utc;
use mayab_arbitrage::config::Config;
use mayab_arbitrage::tape::{
    capture, index_corpus_sqlite, is_corpus_shard, parse_duration, scan_corpus_streaming,
    seal_corpus_artifacts, verify, verify_corpus, write_json_atomic, CaptureConfig,
};

struct Args {
    root: PathBuf,
    total: Duration,
    shard: Duration,
    pair: String,
    exchanges: Vec<String>,
    depth: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let Some(args) = parse_args()? else {
        return Ok(());
    };
    fs::create_dir_all(&args.root)?;
    let deadline = tokio::time::Instant::now() + args.total;
    let mut shard_index = next_shard_index(&args.root)?;
    let mut completed = 0_usize;
    let mut consecutive_failures = 0_u8;

    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining < Duration::from_secs(5) {
            break;
        }
        let duration = remaining.min(args.shard);
        let shard_name = format!(
            "shard-{:06}-{}",
            shard_index,
            Utc::now().format("%Y%m%dT%H%M%SZ")
        );
        let output = args.root.join(&shard_name);
        let config = CaptureConfig {
            schema_version: 1,
            pair: args.pair.clone(),
            exchanges: args.exchanges.clone(),
            depth: args.depth,
        };
        eprintln!(
            "capturando {shard_name} durante {} s ({} exchanges)",
            duration.as_secs(),
            config.exchanges.len()
        );

        let captured_and_verified = match capture(&output, duration, config).await {
            Ok(manifest) => verify(&output)
                .with_context(|| format!("el shard recién capturado {shard_name} no verificó"))
                .map(|verified| (manifest, verified)),
            Err(error) => Err(error),
        };
        match captured_and_verified {
            Ok((manifest, verified)) => {
                eprintln!(
                    "verificado {}: {} eventos, sha256 {}",
                    manifest.dataset_id, verified.events, verified.sha256
                );
                completed += 1;
                consecutive_failures = 0;
            }
            Err(error) => {
                let failed = args.root.join(format!("failed-{shard_name}"));
                if output.exists() {
                    fs::rename(&output, &failed).with_context(|| {
                        format!(
                            "falló captura y no se pudo poner en cuarentena {}",
                            output.display()
                        )
                    })?;
                }
                eprintln!("shard en cuarentena: {error:#}");
                consecutive_failures = consecutive_failures.saturating_add(1);
                if consecutive_failures >= 3 {
                    bail!(
                        "captura abortada tras {consecutive_failures} shards fallidos consecutivos; último error: {error:#}"
                    );
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
        shard_index += 1;
    }

    if completed == 0 && !has_verified_candidate(&args.root)? {
        bail!("no se produjo ningún shard verificable");
    }
    let report = verify_corpus(&args.root)?;
    let report_path = args.root.join("corpus.json");
    let sqlite_path = args.root.join("corpus.sqlite");
    let scan_path = args.root.join("corpus-scan.json");
    let seal_path = args.root.join("evidence-seal.json");
    write_json_atomic(&report_path, &report)?;
    index_corpus_sqlite(&report, &sqlite_path)?;
    let scan = scan_corpus_streaming(&args.root, &Config::from_env().costos)?;
    write_json_atomic(&scan_path, &scan)?;
    let seal = seal_corpus_artifacts(&args.root, &report, &scan)?;
    write_json_atomic(&seal_path, &seal)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    eprintln!("reporte de corpus: {}", report_path.display());
    eprintln!("índice SQLite: {}", sqlite_path.display());
    eprintln!("scan streaming: {}", scan_path.display());
    eprintln!("sello de evidencia: {}", seal_path.display());
    Ok(())
}

fn parse_args() -> Result<Option<Args>> {
    let mut root = None;
    let mut total = None;
    let mut shard = Duration::from_secs(30 * 60);
    let mut pair = "BTC/USD".to_string();
    let mut exchanges = vec!["Binance", "Kraken", "Coinbase", "OKX"]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut depth = 10_usize;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next().map(PathBuf::from),
            "--total" => total = Some(parse_duration(&args.next().context("falta --total")?)?),
            "--shard" => shard = parse_duration(&args.next().context("falta --shard")?)?,
            "--pair" => pair = args.next().context("falta --pair")?,
            "--exchanges" => {
                exchanges = args
                    .next()
                    .context("falta --exchanges")?
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            "--depth" => depth = args.next().context("falta --depth")?.parse()?,
            "-h" | "--help" => {
                println!("capture-corpus --root DIR --total 24h [--shard 30m] [--pair BTC/USD] [--exchanges Binance,Kraken,Coinbase,OKX] [--depth 10]");
                return Ok(None);
            }
            other => bail!("argumento desconocido: {other}"),
        }
    }
    if !(10..=50).contains(&depth) {
        bail!("--depth debe estar entre 10 y 50");
    }
    if exchanges.len() < 2 {
        bail!("--exchanges requiere al menos dos venues para arbitraje cross-venue");
    }
    exchanges.sort();
    exchanges.dedup();
    if exchanges.len() < 2 {
        bail!("--exchanges requiere al menos dos venues distintos");
    }
    if shard < Duration::from_secs(30) {
        bail!("--shard debe durar al menos 30s");
    }
    let total = total.context("falta --total")?;
    if total < Duration::from_secs(30) {
        bail!("--total debe durar al menos 30s");
    }
    Ok(Some(Args {
        root: root.context("falta --root")?,
        total,
        shard,
        pair,
        exchanges,
        depth,
    }))
}

fn next_shard_index(root: &PathBuf) -> Result<u64> {
    let mut max_index = None;
    for entry in fs::read_dir(root)? {
        let name = entry?.file_name();
        let name = name.to_string_lossy();
        if let Some(raw) = name
            .strip_prefix("shard-")
            .and_then(|rest| rest.split('-').next())
        {
            if let Ok(index) = raw.parse::<u64>() {
                max_index = Some(max_index.map_or(index, |current: u64| current.max(index)));
            }
        }
    }
    Ok(max_index.map_or(0, |index| index + 1))
}

fn has_verified_candidate(root: &PathBuf) -> Result<bool> {
    Ok(fs::read_dir(root)?
        .filter_map(std::result::Result::ok)
        .any(|entry| is_corpus_shard(&entry.path())))
}
