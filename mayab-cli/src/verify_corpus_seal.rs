use std::path::PathBuf;

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .context("uso: verify-corpus-seal DIR")?;
    let seal = mayab_arbitrage::tape::verify_corpus_evidence_seal(&root)?;
    println!("{}", serde_json::to_string_pretty(&seal)?);
    Ok(())
}
