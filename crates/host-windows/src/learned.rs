use std::fs;
use std::path::PathBuf;

use nfidb_core::{AppConfig, AutoBenchmarkObservation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
struct LearnedFile {
    schema_version: u32,
    results: Vec<AutoBenchmarkObservation>,
}

pub(crate) fn load() -> Vec<AutoBenchmarkObservation> {
    let Some(path) = path() else {
        return Vec::new();
    };
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    serde_json::from_slice::<LearnedFile>(&bytes)
        .map(|file| file.results)
        .unwrap_or_default()
}

pub(crate) fn save(results: &[AutoBenchmarkObservation]) -> Result<(), String> {
    let path = path().ok_or_else(|| "NFiDB configuration directory is unavailable".to_owned())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(&LearnedFile {
        schema_version: 1,
        results: results.to_vec(),
    })
    .map_err(|error| error.to_string())?;
    // This cache is advisory and all callers already serialize access through
    // VideoControlState. Writing the destination directly is more reliable on
    // Windows than renaming over an existing file (which fails unless replace
    // semantics are requested explicitly).
    fs::write(&path, bytes).map_err(|error| error.to_string())
}

fn path() -> Option<PathBuf> {
    AppConfig::path()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("codec-benchmarks.json")))
}
