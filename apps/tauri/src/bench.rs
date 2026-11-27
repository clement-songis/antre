//! Benchmark pages bundled in `dist/` (built from `bench/`), and the IPC
//! command they call to hand their report back to the shell.

use std::{path::PathBuf, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchPage {
    Stream,
    Transcript,
    Canvas,
}

impl BenchPage {
    #[must_use]
    pub fn path(self) -> &'static str {
        match self {
            Self::Stream => "stream.html",
            Self::Transcript => "transcript.html",
            Self::Canvas => "canvas.html",
        }
    }
}

impl FromStr for BenchPage {
    type Err = anyhow::Error;

    fn from_str(name: &str) -> anyhow::Result<Self> {
        match name {
            "stream" => Ok(Self::Stream),
            "transcript" => Ok(Self::Transcript),
            "canvas" => Ok(Self::Canvas),
            other => anyhow::bail!("unknown bench page `{other}` (stream, transcript, canvas)"),
        }
    }
}

/// Where the report goes; `None` prints it to stdout.
pub struct BenchOutput(pub Option<PathBuf>);

/// Called once by a bench page when it has finished. Writes the report,
/// then quits the app so a harness can run pages back to back.
#[tauri::command]
pub fn bench_report(
    app: tauri::AppHandle,
    output: tauri::State<'_, BenchOutput>,
    report: serde_json::Value,
) -> Result<(), String> {
    write_report(output.0.as_deref(), &report).map_err(|error| error.to_string())?;
    app.exit(0);
    Ok(())
}

/// Progress lines from a bench page, so a harness can follow a run.
#[tauri::command]
pub fn bench_log(line: String) {
    eprintln!("[bench] {line}");
}

fn write_report(out: Option<&std::path::Path>, report: &serde_json::Value) -> anyhow::Result<()> {
    let text = serde_json::to_string_pretty(report)?;
    match out {
        Some(path) => std::fs::write(path, text)?,
        None => println!("{text}"),
    }
    Ok(())
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_round_trip_through_their_names() {
        for (name, page) in [
            ("stream", BenchPage::Stream),
            ("transcript", BenchPage::Transcript),
            ("canvas", BenchPage::Canvas),
        ] {
            assert_eq!(name.parse::<BenchPage>().unwrap(), page);
            assert_eq!(page.path(), format!("{name}.html"));
        }
    }

    #[test]
    fn report_is_written_as_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.json");
        write_report(Some(&path), &serde_json::json!({ "fps": 120 })).unwrap();
        let back: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(back["fps"], 120);
    }
}
