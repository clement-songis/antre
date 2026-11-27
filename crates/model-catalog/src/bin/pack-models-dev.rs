//! Build the bundled snapshot from a downloaded models.dev `api.json`:
//! `pack-models-dev <api.json> <out.json.gz> [--diff <old.json.gz>]`.
//!
//! The whole file is validated (parses, at least `MIN_MODELS` models) and
//! compressed. `--diff` prints a Markdown summary of the changes against a
//! previous snapshot, used as the body of the scheduled refresh PR.

use std::process::ExitCode;

use moltis_model_catalog::{
    diff::CatalogDiff,
    models_dev::ModelsDevCatalog,
    refresh::MIN_MODELS,
    snapshot::{MAX_CATALOG_BYTES, gunzip, gzip},
};

struct Args {
    input: String,
    output: String,
    diff_against: Option<String>,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let usage =
        || "usage: pack-models-dev <api.json> <out.json.gz> [--diff <old.json.gz>]".to_string();
    let mut positional = Vec::new();
    let mut diff_against = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--diff" => diff_against = Some(iter.next().ok_or_else(usage)?.clone()),
            _ => positional.push(arg.clone()),
        }
    }
    let [input, output] = <[String; 2]>::try_from(positional).map_err(|_| usage())?;
    Ok(Args {
        input,
        output,
        diff_against,
    })
}

fn load_gz(path: &str) -> Result<ModelsDevCatalog, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?;
    let json = gunzip(&bytes, MAX_CATALOG_BYTES).map_err(|e| e.to_string())?;
    ModelsDevCatalog::parse(&json).map_err(|e| e.to_string())
}

fn run(args: &[String]) -> Result<String, String> {
    let args = parse_args(args)?;
    let old = args.diff_against.as_deref().map(load_gz).transpose()?;
    let raw = std::fs::read(&args.input).map_err(|e| format!("read {}: {e}", args.input))?;
    let catalog = ModelsDevCatalog::parse(&raw).map_err(|e| e.to_string())?;
    if catalog.model_count() < MIN_MODELS {
        return Err(format!(
            "refusing a catalog with {} models (minimum {MIN_MODELS})",
            catalog.model_count()
        ));
    }
    let packed = gzip(&raw).map_err(|e| e.to_string())?;
    std::fs::write(&args.output, &packed).map_err(|e| format!("write {}: {e}", args.output))?;
    let summary = format!(
        "{} providers, {} models, {} bytes compressed ({} raw)",
        catalog.providers.len(),
        catalog.model_count(),
        packed.len(),
        raw.len()
    );
    Ok(match old {
        Some(old) => format!(
            "{}\n{summary}",
            CatalogDiff::between(&old, &catalog).to_markdown(50)
        ),
        None => summary,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| (*a).to_string()).collect()
    }

    #[test]
    fn parses_positional_and_diff_args() {
        let args = parse_args(&strings(&["in.json", "out.gz", "--diff", "old.gz"])).unwrap();
        assert_eq!(args.input, "in.json");
        assert_eq!(args.output, "out.gz");
        assert_eq!(args.diff_against.as_deref(), Some("old.gz"));
        assert!(parse_args(&strings(&["in.json"])).is_err());
        assert!(parse_args(&strings(&["in.json", "out.gz", "--diff"])).is_err());
    }

    #[test]
    fn rejects_small_catalogs_and_packs_valid_ones() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("api.json");
        let output = dir.path().join("out.json.gz");
        std::fs::write(
            &input,
            r#"{"p": {"id": "p", "models": {"m": {"id": "m"}}}}"#,
        )
        .unwrap();
        let err = run(&strings(&[
            input.to_str().unwrap(),
            output.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(err.contains("refusing"));
        assert!(!output.exists());
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(summary) => {
            println!("{summary}");
            ExitCode::SUCCESS
        },
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        },
    }
}
