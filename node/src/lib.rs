//! Node.js bindings for trash-pro (napi-rs).
//!
//! Exposes batch trash operations as `trashSync` / `trash`, with per-item
//! results and the same temp-directory fallback as the Rust core.

use std::path::PathBuf;

use napi_derive::napi;
use trash_pro::{delete_all, list_recoverable, restore_all, Destination, ItemKind, Options};

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

/// An item in the fallback directory that can be moved back to its origin.
#[napi(object)]
pub struct RecoverableItemInfo {
    /// Current location inside the fallback directory.
    pub item: String,
    /// Absolute path the item was moved from.
    pub original_path: String,
    /// When it was moved (RFC 3339, UTC).
    pub deleted_at: String,
    /// Filesystem entry kind: `"file"`, `"directory"` or `"symlink"`.
    pub kind: String,
}

/// Options for `restore` / `restoreSync`.
#[napi(object)]
pub struct RestoreOptions {
    /// Replace whatever now occupies an item's original location.
    /// Default: `false` (the restore fails with a conflict error).
    pub overwrite: Option<bool>,
}

/// Per-item result of a restore operation.
#[napi(object)]
pub struct RestoreItemResult {
    /// The fallback item this result belongs to.
    pub item: String,
    /// Whether the item was moved back successfully.
    pub ok: bool,
    /// The restored (original) path when `ok` is `true`.
    pub restored_to: Option<String>,
    /// Error message when `ok` is `false`.
    pub error: Option<String>,
}

fn kind_str(kind: ItemKind) -> String {
    match kind {
        ItemKind::File => "file".into(),
        ItemKind::Directory => "directory".into(),
        ItemKind::Symlink => "symlink".into(),
    }
}

/// Lists every recoverable item in the fallback directory (`tempDir`, or the
/// system temp dir + `/trash-pro` by default) together with the path it was
/// moved from. Items without a trash-pro provenance record are skipped.
#[napi]
pub fn list(temp_dir: Option<String>) -> napi::Result<Vec<RecoverableItemInfo>> {
    list_recoverable(temp_dir.map(PathBuf::from))
        .map(|items| {
            items
                .into_iter()
                .map(|it| RecoverableItemInfo {
                    item: it.item.display().to_string(),
                    original_path: it.original.display().to_string(),
                    deleted_at: it.deleted_at,
                    kind: kind_str(it.kind),
                })
                .collect()
        })
        .map_err(|e| napi::Error::from_reason(format!("cannot list fallback directory: {e}")))
}

fn run_restore(items: Vec<String>, options: Option<RestoreOptions>) -> Vec<RestoreItemResult> {
    let overwrite = options.and_then(|o| o.overwrite).unwrap_or(false);
    restore_all(items.iter().map(PathBuf::from), overwrite)
        .into_iter()
        .map(|outcome| match outcome.result {
            Ok(original) => RestoreItemResult {
                item: outcome.item.display().to_string(),
                ok: true,
                restored_to: Some(original.display().to_string()),
                error: None,
            },
            Err(e) => RestoreItemResult {
                item: outcome.item.display().to_string(),
                ok: false,
                restored_to: None,
                error: Some(e.to_string()),
            },
        })
        .collect()
}

/// Moves fallback items back to the paths they were moved from. Runs
/// synchronously on the calling thread.
#[napi(js_name = "restoreSync")]
pub fn restore_sync(items: Vec<String>, options: Option<RestoreOptions>) -> Vec<RestoreItemResult> {
    run_restore(items, options)
}

/// Moves fallback items back to the paths they were moved from. Resolves on
/// a worker thread (non-blocking for the JS event loop).
#[napi]
pub async fn restore(
    items: Vec<String>,
    options: Option<RestoreOptions>,
) -> Vec<RestoreItemResult> {
    run_restore(items, options)
}
