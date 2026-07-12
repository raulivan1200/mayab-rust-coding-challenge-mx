use std::{fs, path::PathBuf};

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let mut root = None;
    let mut output = None;
    let mut sqlite_index = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next().map(PathBuf::from),
            "--output" => output = args.next().map(PathBuf::from),
            "--sqlite-index" => sqlite_index = args.next().map(PathBuf::from),
            "-h" | "--help" => {
                println!("verify-corpus --root artifacts/tapes --output artifacts/corpus.json [--sqlite-index artifacts/corpus.sqlite]");
                return Ok(());
            }
            other => anyhow::bail!("argumento desconocido: {other}"),
        }
    }

    let root = root.context("falta --root")?;
    let report = mayab_arbitrage::tape::verify_corpus(&root)?;
    if let Some(database) = &sqlite_index {
        mayab_arbitrage::tape::index_corpus_sqlite(&report, database)?;
        eprintln!("Índice SQLite: {}", database.display());
    }
    let json = serde_json::to_vec_pretty(&report)?;
    if let Some(output) = output {
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        mayab_arbitrage::tape::write_json_atomic(&output, &report)?;
        println!("Corpus verificado: {}", output.display());
    } else {
        println!("{}", String::from_utf8(json)?);
    }
    Ok(())
}
