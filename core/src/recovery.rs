//! Provenance records and restore support for the temp fallback directory.
//!
//! Every item moved into the fallback directory gets a
//! `<dest>.trashinfo.json` sidecar recording where it came from and when it
//! was moved, so users can find and move items back later — even after the
//! process that moved them is gone. Items that went to the real OS trash are
//! not recorded here: the OS trash keeps its own provenance (Recycle Bin
//! "Restore", FreeDesktop `.trashinfo`, ...).

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Sidecar format version.
const SIDECAR_VERSION: u32 = 1;

/// What kind of filesystem entry a recorded item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemKind {
    File,
    Directory,
    Symlink,
}

impl ItemKind {
    pub fn is_dir(self) -> bool {
        matches!(self, ItemKind::Directory)
    }

    pub fn of(file_type: fs::FileType) -> ItemKind {
        if file_type.is_symlink() {
            ItemKind::Symlink
        } else if file_type.is_dir() {
            ItemKind::Directory
        } else {
            ItemKind::File
        }
    }
}

/// The on-disk provenance record.
///
/// The path is stored twice: a lossy string humans can read, and the exact
/// encoded bytes of the original [`OsStr`] (non-UTF-8 paths survive intact).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Sidecar {
    version: u32,
    original_path: String,
    original_path_bytes: Vec<u8>,
    deleted_at: String,
    kind: ItemKind,
}

impl Sidecar {
    fn new(original: &Path, kind: ItemKind) -> Sidecar {
        Sidecar {
            version: SIDECAR_VERSION,
            original_path: original.to_string_lossy().into_owned(),
            original_path_bytes: original.as_os_str().as_encoded_bytes().to_vec(),
            deleted_at: jiff::Timestamp::now().to_string(),
            kind,
        }
    }

    fn original(&self) -> PathBuf {
        if self.original_path_bytes.is_empty() {
            return PathBuf::from(&self.original_path);
        }
        // Bytes were produced by `OsStr::as_encoded_bytes` of a real path, so
        // the round-trip is lossless (see the `encoded_bytes` Rust docs).
        #[allow(unused_unsafe)]
        unsafe {
            PathBuf::from(OsString::from_encoded_bytes_unchecked(
                self.original_path_bytes.clone(),
            ))
        }
    }
}

/// An item in the fallback directory that can be moved back to its origin.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RecoverableItem {
    /// Current location inside the fallback directory.
    pub item: PathBuf,
    /// Absolute path the item was moved from.
    pub original: PathBuf,
    /// When it was moved (RFC 3339, UTC).
    pub deleted_at: String,
    /// Filesystem entry kind.
    pub kind: ItemKind,
}

/// Per-item result of [`restore_all`].
#[derive(Debug)]
pub struct RestoreOutcome {
    /// The fallback item this outcome belongs to.
    pub item: PathBuf,
    /// Where the item was moved back to, or why it could not be restored.
    pub result: Result<PathBuf, RestoreError>,
}

/// Error of a single restore.
#[derive(Debug)]
pub enum RestoreError {
    /// The item has no provenance record (it was not moved by trash-pro).
    NotManaged { item: PathBuf },

    /// The provenance record exists but cannot be used.
    BadRecord { item: PathBuf, message: String },

    /// The recorded item itself is gone.
    MissingItem { item: PathBuf },

    /// The original location is occupied and `overwrite` was not set.
    Conflict { item: PathBuf, at: PathBuf },

    /// A filesystem operation while restoring failed.
    Io { item: PathBuf, source: io::Error },
}

impl std::fmt::Display for RestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RestoreError::NotManaged { item } => write!(
                f,
                "{} has no trash-pro provenance record (not moved by trash-pro)",
                item.display()
            ),
            RestoreError::BadRecord { item, message } => {
                write!(f, "unreadable provenance record for {}: {}", item.display(), message)
            }
            RestoreError::MissingItem { item } => {
                write!(f, "{} no longer exists in the fallback directory", item.display())
            }
            RestoreError::Conflict { item, at } => write!(
                f,
                "cannot restore {}: original location {} is occupied (use overwrite)",
                item.display(),
                at.display()
            ),
            RestoreError::Io { item, source } => {
                write!(f, "cannot restore {}: {}", item.display(), source)
            }
        }
    }
}

impl std::error::Error for RestoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RestoreError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// `<item>.trashinfo.json` next to a fallback item.
pub(crate) fn sidecar_path(item: &Path) -> PathBuf {
    let mut name: OsString = item.as_os_str().to_os_string();
    name.push(".trashinfo.json");
    PathBuf::from(name)
}

/// Writes the provenance record for an item that is about to be moved into
/// the fallback directory.
pub(crate) fn write_sidecar(dest: &Path, original: &Path, kind: ItemKind) -> io::Result<()> {
    let record = Sidecar::new(original, kind);
    let json = serde_json::to_vec_pretty(&record)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(sidecar_path(dest), json)
}

/// Removes the provenance record of a fallback item, if any (best effort).
pub(crate) fn remove_sidecar(item: &Path) {
    let _ = fs::remove_file(sidecar_path(item));
}

fn read_sidecar(item: &Path) -> Result<Sidecar, RestoreError> {
    let path = sidecar_path(item);
    let data = fs::read_to_string(&path).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            RestoreError::NotManaged {
                item: item.to_path_buf(),
            }
        } else {
            RestoreError::Io {
                item: item.to_path_buf(),
                source: e,
            }
        }
    })?;
    serde_json::from_str(&data).map_err(|e| RestoreError::BadRecord {
        item: item.to_path_buf(),
        message: e.to_string(),
    })
}

/// Lists every recoverable item in the fallback directory
/// (`<system temp dir>/trash-pro` by default).
///
/// Items without a provenance record — files dropped in by other tools — and
/// orphaned records whose item is gone are silently skipped.
pub fn list_recoverable(temp_dir: Option<PathBuf>) -> io::Result<Vec<RecoverableItem>> {
    let base = temp_dir.unwrap_or_else(crate::default_temp_root);
    let mut items = Vec::new();
    let Ok(entries) = fs::read_dir(&base) else {
        return Ok(items);
    };
    for entry in entries {
        let item = entry?.path();
        if item.symlink_metadata().is_err() {
            continue; // orphaned sidecar
        }
        let Ok(record) = read_sidecar(&item) else {
            continue; // foreign or corrupt: treat as unmanaged
        };
        items.push(RecoverableItem {
            item,
            original: record.original(),
            deleted_at: record.deleted_at,
            kind: record.kind,
        });
    }
    items.sort();
    Ok(items)
}

/// Moves a fallback item back to the path it was moved from.
///
/// The original location is claimed atomically: when it is already occupied
/// the restore fails with [`RestoreError::Conflict`] unless `overwrite` is
/// set. Returns the restored path on success and removes the provenance
/// record.
pub fn restore(item: &Path, overwrite: bool) -> Result<PathBuf, RestoreError> {
    let meta = fs::symlink_metadata(item).map_err(|_| RestoreError::MissingItem {
        item: item.to_path_buf(),
    })?;
    let kind = ItemKind::of(meta.file_type());
    let record = read_sidecar(item)?;
    let original = record.original();

    if original.as_os_str().is_empty() || original.file_name().is_none() {
        return Err(RestoreError::BadRecord {
            item: item.to_path_buf(),
            message: format!("refusing to restore to {}", original.display()),
        });
    }
    if let Some(parent) = original.parent() {
        fs::create_dir_all(parent).map_err(|source| RestoreError::Io {
            item: item.to_path_buf(),
            source,
        })?;
    }

    claim_original(item, &original, kind, overwrite)?;

    if let Err(source) = crate::move_into_placeholder(item, &original, kind) {
        // The placeholder slot is ours; drop it before reporting the failure.
        crate::abandon_claim(&original);
        return Err(RestoreError::Io {
            item: item.to_path_buf(),
            source,
        });
    }
    remove_sidecar(item);
    Ok(original)
}

/// Claims the exact restore slot at `original`, failing with
/// [`RestoreError::Conflict`] when it is occupied and `overwrite` is off.
fn claim_original(
    item: &Path,
    original: &Path,
    kind: ItemKind,
    overwrite: bool,
) -> Result<(), RestoreError> {
    match crate::claim_dest(original, kind) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists && overwrite => {
            let _ = crate::remove_recursive(original);
            crate::claim_dest(original, kind).map_err(|source| RestoreError::Io {
                item: item.to_path_buf(),
                source,
            })
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(RestoreError::Conflict {
            item: item.to_path_buf(),
            at: original.to_path_buf(),
        }),
        Err(source) => Err(RestoreError::Io {
            item: item.to_path_buf(),
            source,
        }),
    }
}

/// Restores every item independently; one failure never blocks the others.
pub fn restore_all<I, T>(items: I, overwrite: bool) -> Vec<RestoreOutcome>
where
    I: IntoIterator<Item = T>,
    T: AsRef<Path>,
{
    items
        .into_iter()
        .map(|p| {
            let item = p.as_ref();
            RestoreOutcome {
                item: item.to_path_buf(),
                result: restore(item, overwrite),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{delete, Destination, Options};

    struct Scratch {
        root: PathBuf,
        origin: PathBuf,
        fallback: PathBuf,
    }

    impl Scratch {
        fn new() -> Scratch {
            let root = std::env::temp_dir().join(format!(
                "trash-pro-recovery-test-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&root);
            let origin = root.join("origin");
            let fallback = root.join("fallback");
            fs::create_dir_all(&origin).unwrap();
            Scratch {
                root,
                origin,
                fallback,
            }
        }

        fn file(&self, rel: &str, contents: &str) -> PathBuf {
            let path = self.origin.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, contents).unwrap();
            path
        }

        fn opts(&self) -> Options {
            Options {
                fallback_to_temp: true,
                force_temp: true,
                temp_dir: Some(self.fallback.clone()),
            }
        }

        fn moved_item(&self, src: &Path) -> PathBuf {
            let Destination::Temp(dest) = delete(src, &self.opts()).unwrap() else {
                panic!("expected Temp destination");
            };
            dest
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn sidecar_records_origin_and_listing_shows_it() {
        let scratch = Scratch::new();
        let src = scratch.file("dir/remember.txt", "R");
        let dest = scratch.moved_item(&src);

        let sc = sidecar_path(&dest);
        assert!(sc.exists(), "sidecar must be written next to the item");
        let record: Sidecar = serde_json::from_str(&fs::read_to_string(&sc).unwrap()).unwrap();
        assert_eq!(record.version, SIDECAR_VERSION);
        assert_eq!(record.original(), src);
        assert_eq!(record.kind, ItemKind::File);
        assert!(!record.deleted_at.is_empty());

        let listed = list_recoverable(Some(scratch.fallback.clone())).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].item, dest);
        assert_eq!(listed[0].original, src);
    }

    #[test]
    fn restore_moves_file_back_and_clears_record() {
        let scratch = Scratch::new();
        let src = scratch.file("back.txt", "payload");
        let dest = scratch.moved_item(&src);
        assert!(!src.exists());

        let restored = restore(&dest, false).unwrap();

        assert_eq!(restored, src);
        assert_eq!(fs::read_to_string(&src).unwrap(), "payload");
        assert!(!dest.exists());
        assert!(!sidecar_path(&dest).exists());
        assert!(list_recoverable(Some(scratch.fallback.clone())).unwrap().is_empty());
    }

    #[test]
    fn restore_roundtrips_directory_trees() {
        let scratch = Scratch::new();
        scratch.file("tree/inner/deep.txt", "deep");
        scratch.file("tree/top.txt", "top");
        let src = scratch.origin.join("tree");
        let dest = scratch.moved_item(&src);

        let restored = restore(&dest, false).unwrap();

        assert_eq!(restored, src);
        assert_eq!(fs::read_to_string(src.join("top.txt")).unwrap(), "top");
        assert_eq!(fs::read_to_string(src.join("inner/deep.txt")).unwrap(), "deep");
    }

    #[test]
    fn restore_conflicts_unless_overwrite() {
        let scratch = Scratch::new();
        let src = scratch.file("taken.txt", "old");
        let dest = scratch.moved_item(&src);
        fs::write(&src, "new occupant").unwrap();

        let err = restore(&dest, false).unwrap_err();
        assert!(matches!(err, RestoreError::Conflict { .. }), "got {err:?}");
        assert_eq!(fs::read_to_string(&src).unwrap(), "new occupant");
        assert!(dest.exists(), "item must stay in fallback after a conflict");

        let restored = restore(&dest, true).unwrap();
        assert_eq!(restored, src);
        assert_eq!(fs::read_to_string(&src).unwrap(), "old");
    }

    #[test]
    fn restore_rejects_unmanaged_items() {
        let scratch = Scratch::new();
        let foreign = scratch.fallback.join("dropped-by-someone-else");
        fs::create_dir_all(&scratch.fallback).unwrap();
        fs::write(&foreign, "not ours").unwrap();

        let err = restore(&foreign, false).unwrap_err();
        assert!(matches!(err, RestoreError::NotManaged { .. }), "got {err:?}");
    }

    #[test]
    fn listing_skips_orphan_records_and_foreign_files() {
        let scratch = Scratch::new();
        let src = scratch.file("real.txt", "x");
        let dest = scratch.moved_item(&src);

        // Orphaned record: sidecar without its item.
        fs::write(scratch.fallback.join("gone.trashinfo.json"), "{}").unwrap();
        // Foreign file without a record.
        fs::write(scratch.fallback.join("stranger.txt"), "?").unwrap();

        let listed = list_recoverable(Some(scratch.fallback.clone())).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].item, dest);
    }

    #[cfg(unix)]
    #[test]
    fn restore_roundtrips_symlinks() {
        use std::os::unix::fs::symlink;

        let scratch = Scratch::new();
        let target = scratch.file("target.txt", "T");
        let link = scratch.origin.join("link");
        symlink(&target, &link).unwrap();

        let dest = scratch.moved_item(&link);
        let restored = restore(&dest, false).unwrap();

        assert_eq!(restored, link);
        assert!(restored.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(&restored).unwrap(), target);
        assert!(target.exists(), "symlink target must stay intact");
    }
}
