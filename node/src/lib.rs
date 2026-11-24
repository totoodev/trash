//! Node.js bindings for trash-pro (napi-rs).
//!
//! Exposes batch trash operations as `trashSync` / `trash`, with per-item
//! results and the same temp-directory fallback as the Rust core.

use std::path::PathBuf;

use napi_derive::napi;
use trash_pro::{delete_all, Destination, Options};

/// Options for `trash` / `trashSync`.
#[napi(object)]
pub struct TrashOptions {
    /// When the OS trash is unavailable (e.g. headless server), move items to
    /// `tempDir` instead of reporting an error. Default: `true`.
    pub fallback_to_temp: Option<bool>,

    /// Skip the OS trash entirely and always move items to `tempDir`.
    /// Default: `false`.
    pub force_temp: Option<bool>,

    /// Fallback destination directory. Default: system temp dir + `/trash-pro`.
    pub temp_dir: Option<String>,
}

/// Per-item result of a trash operation.
#[napi(object)]
pub struct TrashItemResult {
    /// The input path this result belongs to.
    pub path: String,

    /// Whether the item was moved successfully.
    pub ok: bool,

    /// Where the item went: `"trash"` (OS trash) or `"temp"` (fallback).
    pub method: Option<String>,

    /// Final path when `method` is `"temp"` (after name de-duplication).
    pub destination: Option<String>,

    /// Error message when `ok` is `false`.
    pub error: Option<String>,
}

fn to_options(options: Option<TrashOptions>) -> Options {
    let defaults = Options::default();
    match options {
        Some(o) => Options {
            fallback_to_temp: o.fallback_to_temp.unwrap_or(defaults.fallback_to_temp),
            force_temp: o.force_temp.unwrap_or(defaults.force_temp),
            temp_dir: o.temp_dir.map(PathBuf::from),
        },
        None => defaults,
    }
}

fn run(paths: Vec<String>, options: Option<TrashOptions>) -> Vec<TrashItemResult> {
    let opts = to_options(options);
    delete_all(paths.iter().map(PathBuf::from), &opts)
        .into_iter()
        .map(|outcome| match outcome.result {
            Ok(Destination::Trash) => TrashItemResult {
                path: outcome.path.display().to_string(),
                ok: true,
                method: Some("trash".into()),
                destination: None,
                error: None,
            },
            Ok(Destination::Temp(dest)) => TrashItemResult {
                path: outcome.path.display().to_string(),
                ok: true,
                method: Some("temp".into()),
                destination: Some(dest.display().to_string()),
                error: None,
            },
            Err(e) => TrashItemResult {
                path: outcome.path.display().to_string(),
                ok: false,
                method: None,
                destination: None,
                error: Some(e.to_string()),
            },
        })
        .collect()
}

/// Moves files and folders to the OS trash/recycle bin, with an optional
/// temp-directory fallback. Runs synchronously on the calling thread.
#[napi(js_name = "trashSync")]
pub fn trash_sync(paths: Vec<String>, options: Option<TrashOptions>) -> Vec<TrashItemResult> {
    run(paths, options)
}

/// Moves files and folders to the OS trash/recycle bin, with an optional
/// temp-directory fallback. Resolves on a worker thread (non-blocking for
/// the JS event loop).
#[napi]
pub async fn trash(paths: Vec<String>, options: Option<TrashOptions>) -> Vec<TrashItemResult> {
    run(paths, options)
}
