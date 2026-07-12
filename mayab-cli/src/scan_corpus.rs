use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use mayab_arbitrage::{config::Config, tape::scan_corpus_streaming};

fn main() -> Result<()> {
    let mut root = None;
    let mut output = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next().map(PathBuf::from),
            "--output" => output = args.next().map(PathBuf::from),
            "-h" | "--help" => {
                println!("scan-corpus --root artifacts/tapes/btc-usd --output artifacts/evidence/corpus-scan.json");
                return Ok(());
            }
            other => anyhow::bail!("argumento desconocido: {other}"),
        }
    }
    let root = root.context("falta --root")?;
    let report = scan_corpus_streaming(&root, &Config::from_env().costos)?;
    let json = serde_json::to_vec_pretty(&report)?;
    if let Some(output) = output {
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        mayab_arbitrage::tape::write_json_atomic(&output, &report)?;
        eprintln!("Scan streaming: {}", output.display());
    }
    println!("{}", String::from_utf8(json)?);
    Ok(())
}
