//! trash-pro: move files and folders to the OS trash / recycle bin / wastebasket.
//!
//! Built on [`trash`] (Byron/trash-rs) with a fallback for systems that have no
//! usable trash (headless servers, containers): when the trash backend fails,
//! the item is moved into the system temp directory instead of being lost or
//! left behind.
//!
//! Supported platforms: Linux (FreeDesktop Trash / XDG), macOS (Finder),
//! Windows (Recycle Bin) — on both x64 and arm64.
//!
//! ```
//! use trash_pro::{delete_all, Options};
//! let opts = Options::default(); // fallback to temp enabled
//! let outcomes = delete_all(["some-file", "some-dir"], &opts);
//! for outcome in outcomes {
//!     println!("{:?} -> {:?}", outcome.path, outcome.result);
//! }
//! ```

use std::error::Error as StdError;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

mod recovery;

pub use recovery::{
    list_recoverable, restore, restore_all, ItemKind, RecoverableItem, RestoreError,
    RestoreOutcome,
};

/// Where an item was moved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// Moved into the OS trash / recycle bin / wastebasket.
    Trash,
    /// Moved into the temp fallback directory; carries the final path
    /// (after de-duplicating name collisions).
    Temp(PathBuf),
}

/// Error of a single delete operation.
#[derive(Debug)]
pub enum Error {
    /// The target is missing, is a root / parent-escaping path, or is otherwise
    /// invalid. This is a hard error: the temp fallback is never attempted.
    InvalidTarget { path: PathBuf, message: String },

    /// The OS trash operation failed and the temp fallback was disabled.
    Trash { path: PathBuf, source: trash::Error },

    /// The temp fallback move failed (while `force_temp` is set, or after the
    /// trash backend was found unusable).
    TempMove { path: PathBuf, source: io::Error },

    /// The OS trash operation failed and the temp fallback also failed.
    Both {
        path: PathBuf,
        trash: trash::Error,
        temp: io::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidTarget { path, message } => {
                write!(f, "invalid target {}: {}", path.display(), message)
            }
            Error::Trash { path, source } => {
                write!(f, "cannot move {} to trash: {}", path.display(), source)
            }
            Error::TempMove { path, source } => {
                write!(f, "cannot move {} to temp fallback: {}", path.display(), source)
            }
            Error::Both { path, trash, temp } => write!(
                f,
                "cannot move {} to trash ({}) nor to temp fallback ({})",
                path.display(),
                trash,
                temp
            ),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Error::InvalidTarget { .. } => None,
            Error::Trash { source, .. } => Some(source),
            Error::TempMove { source, .. } => Some(source),
            Error::Both { trash, .. } => Some(trash),
        }
    }
}

/// Behavior of a delete operation.
#[derive(Debug, Clone)]
pub struct Options {
    /// When the OS trash is unavailable or fails with a backend error, move the
    /// item into [`Options::temp_dir`] instead of returning an error.
    /// Default: `true`.
    pub fallback_to_temp: bool,

    /// Skip the OS trash entirely and always move items into the temp fallback
    /// directory. Default: `false`.
    pub force_temp: bool,

    /// Fallback destination directory. Default: `<system temp dir>/trash-pro`.
    pub temp_dir: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            fallback_to_temp: true,
            force_temp: false,
            temp_dir: None,
        }
    }
}

/// Default fallback destination: `<system temp dir>/trash-pro`.
pub(crate) fn default_temp_root() -> PathBuf {
    std::env::temp_dir().join("trash-pro")
}

impl Options {
    /// Resolved fallback destination directory.
    pub fn temp_root(&self) -> PathBuf {
        match &self.temp_dir {
            Some(dir) => dir.clone(),
            None => default_temp_root(),
        }
    }
}

/// Per-item result of [`delete_all`].
#[derive(Debug)]
pub struct ItemOutcome {
    /// The input path this outcome belongs to.
    pub path: PathBuf,
    /// Where the item ended up, or why it could not be moved.
    pub result: Result<Destination, Error>,
}

/// Moves a single file or folder to the OS trash, with optional temp fallback.
pub fn delete<T: AsRef<Path>>(path: T, opts: &Options) -> Result<Destination, Error> {
    delete_inner(path.as_ref(), opts, &mut false)
}

/// Moves every path to the OS trash (with optional temp fallback).
///
/// Items are processed independently: one failing item never blocks the
/// others, and every item gets its own [`ItemOutcome`].
pub fn delete_all<I, T>(paths: I, opts: &Options) -> Vec<ItemOutcome>
where
    I: IntoIterator<Item = T>,
    T: AsRef<Path>,
{
    // Once a non-hard trash error proves the backend unusable (e.g. headless
    // server without a trash), skip straight to the temp fallback for the
    // remaining items instead of retrying a doomed trash call each time.
    let mut trash_unavailable = false;
    paths
        .into_iter()
        .map(|p| {
            let path = p.as_ref();
            let result = delete_inner(path, opts, &mut trash_unavailable);
            ItemOutcome {
                path: path.to_path_buf(),
                result,
            }
        })
        .collect()
}

fn delete_inner(
    path: &Path,
    opts: &Options,
    trash_unavailable: &mut bool,
) -> Result<Destination, Error> {
    delete_inner_with(path, opts, trash_unavailable, os_trash)
}

/// [`delete_inner`] with an injectable trash backend, so tests can exercise
/// the fallback classification without breaking the real OS trash.
fn delete_inner_with<F>(
    path: &Path,
    opts: &Options,
    trash_unavailable: &mut bool,
    backend: F,
) -> Result<Destination, Error>
where
    F: FnOnce(&Path) -> Result<(), trash::Error>,
{
    validate_target(path)?;

    if opts.force_temp {
        return move_to_temp(path, opts)
            .map(Destination::Temp)
            .map_err(|source| Error::TempMove {
                path: path.to_path_buf(),
                source,
            });
    }

    if *trash_unavailable && opts.fallback_to_temp {
        return move_to_temp(path, opts)
            .map(Destination::Temp)
            .map_err(|source| Error::TempMove {
                path: path.to_path_buf(),
                source,
            });
    }

    match backend(path) {
        Ok(()) => Ok(Destination::Trash),
        Err(source) if !is_hard_error(&source) => {
            // The trash backend itself is unusable (missing trash directory,
            // no desktop environment, ...): fall back to the temp directory.
            if opts.fallback_to_temp {
                *trash_unavailable = true;
                move_to_temp(path, opts)
                    .map(Destination::Temp)
                    .map_err(|temp| Error::Both {
                        path: path.to_path_buf(),
                        trash: source,
                        temp,
                    })
            } else {
                Err(Error::Trash {
                    path: path.to_path_buf(),
                    source,
                })
            }
        }
        Err(source) => Err(Error::Trash {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Moves one item into the real OS trash.
///
/// On macOS this uses `NSFileManager.trashItemAtURL` (`NsFileManager` method)
/// instead of the crate default (`Finder` via `osascript`): the Finder method
/// needs extra Automation permissions and blocks for minutes in headless /
/// non-interactive sessions, while `NsFileManager` has no extra permission
/// requirements. The trade-off is that Finder's "Put Back" may not be
/// available for the trashed item.
#[cfg(target_os = "macos")]
fn os_trash(path: &Path) -> Result<(), trash::Error> {
    use trash::macos::{DeleteMethod, TrashContextExtMacos};
    let mut ctx = trash::TrashContext::new();
    ctx.set_delete_method(DeleteMethod::NsFileManager);
    ctx.delete(path)
}

#[cfg(not(target_os = "macos"))]
fn os_trash(path: &Path) -> Result<(), trash::Error> {
    trash::delete(path)
}

/// Errors that are about the target itself (missing, root, bad path) rather
/// than an unusable trash backend. These never trigger the temp fallback.
///
/// Everything else — `Unknown`, `Os`, and the platform-gated `FileSystem` /
/// `Portal` variants — is treated as "trash backend unavailable".
fn is_hard_error(e: &trash::Error) -> bool {
    matches!(
        e,
        trash::Error::TargetedRoot
            | trash::Error::CouldNotAccess { .. }
            | trash::Error::CanonicalizePath { .. }
            | trash::Error::ConvertOsString { .. }
    )
}

fn validate_target(path: &Path) -> Result<(), Error> {
    let invalid = |message: String| Error::InvalidTarget {
        path: path.to_path_buf(),
        message,
    };

    if path.as_os_str().is_empty() {
        return Err(invalid("empty path".into()));
    }
    if path.file_name().is_none() {
        // `.`, `..`, `/`, `C:\` — a filesystem root or a parent-escaping path.
        return Err(invalid("refusing to operate on a root or parent-escaping path".into()));
    }

    let meta = fs::symlink_metadata(path)
        .map_err(|e| invalid(format!("cannot access target: {e}")))?;

    // Symlinks are trashed as links (the target is kept intact), so only the
    // lexical checks above apply to them.
    if !meta.file_type().is_symlink() {
        if let Ok(canonical) = path.canonicalize() {
            if canonical.parent().is_none() {
                return Err(invalid("refusing to operate on a filesystem root".into()));
            }
        }
    }
    Ok(())
}

/// Moves `path` into the fallback directory (`<temp_root>/<file name>`),
/// de-duplicating name collisions with `.1`, `.2`, ... suffixes.
///
/// The destination name is *reserved atomically* before the move, so two
/// concurrent processes moving items with the same name can never overwrite
/// each other. A provenance sidecar (`<dest>.trashinfo.json`) is written
/// before the move — a reported success is always recoverable via
/// [`restore`] / [`list_recoverable`]. Tries an atomic `rename` first and
/// falls back to copy + delete when the source and the temp directory live
/// on different filesystems.
fn move_to_temp(path: &Path, opts: &Options) -> io::Result<PathBuf> {
    let base = opts.temp_root();
    fs::create_dir_all(&base)?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let meta = fs::symlink_metadata(path)?;
    let kind = ItemKind::of(meta.file_type());
    let dest = reserve_unique_dest(&base, name, kind)?;

    if let Err(e) = recovery::write_sidecar(&dest, &absolute_path(path), kind) {
        abandon_claim(&dest);
        return Err(e);
    }
    if let Err(e) = move_into_placeholder(path, &dest, kind) {
        abandon_claim(&dest);
        recovery::remove_sidecar(&dest);
        return Err(e);
    }
    Ok(dest)
}

/// Absolute (but not canonicalized) form of `path`, for provenance records.
fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

/// Atomically claims `<base>/<name>` (then `<base>/<name>.1`, `.2`, ...) by
/// creating an empty placeholder — an empty directory when the source is a
/// directory, an empty file otherwise — and returns the claimed path.
///
/// Ownership is established by *creation* (`create_new` / `create_dir`), never
/// by probing with a separate existence check, so two concurrent callers can
/// never walk away with the same destination and overwrite each other.
fn reserve_unique_dest(base: &Path, name: &OsStr, kind: ItemKind) -> io::Result<PathBuf> {
    let mut n: u32 = 0;
    loop {
        let dest = if n == 0 {
            base.join(name)
        } else {
            let mut suffixed = OsString::from(name);
            suffixed.push(format!(".{n}"));
            base.join(suffixed)
        };
        match claim_dest(&dest, kind) {
            Ok(()) => return Ok(dest),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                n = n.checked_add(1)
                    .ok_or_else(|| io::Error::other("too many name collisions"))?;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Creates the empty placeholder at `dest` (empty dir for directories, empty
/// file otherwise). Fails with `AlreadyExists` when the slot is taken.
pub(crate) fn claim_dest(dest: &Path, kind: ItemKind) -> io::Result<()> {
    if kind.is_dir() {
        fs::create_dir(dest)
    } else {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dest)
            .map(|_| ())
    }
}

/// Cleans up a claimed slot after a failed move (best effort). The slot may
/// hold a partial copy, so it is removed recursively.
pub(crate) fn abandon_claim(dest: &Path) {
    let _ = remove_recursive(dest);
}

/// Moves `src` onto the placeholder we own at `dest`.
pub(crate) fn move_into_placeholder(src: &Path, dest: &Path, kind: ItemKind) -> io::Result<()> {
    match fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Typically EXDEV (cross-device): copy then remove. `fs::copy`
            // over our placeholder file is fine, but a symlink cannot be
            // created over an existing entry.
            if kind == ItemKind::Symlink {
                let _ = fs::remove_file(dest);
            }
            copy_recursive(src, dest)?;
            remove_recursive(src)
        }
    }
}

fn copy_recursive(src: &Path, dest: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(src)?;
    let file_type = meta.file_type();
    if file_type.is_symlink() {
        copy_symlink(src, dest)
    } else if file_type.is_dir() {
        fs::create_dir_all(dest)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dest.join(entry.file_name()))?;
        }
        // Best effort: preserving permissions must not fail the move.
        let _ = fs::set_permissions(dest, meta.permissions());
        Ok(())
    } else {
        fs::copy(src, dest)?;
        Ok(())
    }
}

fn remove_recursive(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

#[cfg(unix)]
fn copy_symlink(src: &Path, dest: &Path) -> io::Result<()> {
    let target = fs::read_link(src)?;
    std::os::unix::fs::symlink(target, dest)
}

#[cfg(windows)]
fn copy_symlink(src: &Path, dest: &Path) -> io::Result<()> {
    let target = fs::read_link(src)?;
    if src.is_dir() {
        std::os::windows::fs::symlink_dir(target, dest)
    } else {
        std::os::windows::fs::symlink_file(target, dest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates an empty scratch area under the system temp dir.
    struct Scratch {
        root: PathBuf,
    }

    impl Scratch {
        fn new() -> Scratch {
            let root = std::env::temp_dir().join(format!(
                "trash-pro-test-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Scratch { root }
        }

        fn file(&self, rel: &str, contents: &str) -> PathBuf {
            let path = self.root.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, contents).unwrap();
            path
        }

        /// Options that never touch the real OS trash: straight to a private
        /// temp dir inside the scratch area.
        fn force_temp_opts(&self) -> Options {
            Options {
                fallback_to_temp: true,
                force_temp: true,
                temp_dir: Some(self.root.join("fallback")),
            }
        }

        /// Options that consult the trash backend first (for injected-backend
        /// tests) but keep the fallback destination private.
        fn trash_then_fallback_opts(&self) -> Options {
            Options {
                fallback_to_temp: true,
                force_temp: false,
                temp_dir: Some(self.root.join("fallback")),
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn force_temp_moves_file() {
        let scratch = Scratch::new();
        let src = scratch.file("a.txt", "hello");
        let opts = scratch.force_temp_opts();

        let dest = delete(&src, &opts).unwrap();
        let Destination::Temp(dest_path) = dest else {
            panic!("expected Temp destination, got {dest:?}");
        };

        assert!(!src.exists());
        assert_eq!(fs::read_to_string(&dest_path).unwrap(), "hello");
        assert_eq!(dest_path, opts.temp_root().join("a.txt"));
    }

    #[test]
    fn force_temp_moves_directory_tree() {
        let scratch = Scratch::new();
        scratch.file("dir/inner/deep.txt", "deep");
        scratch.file("dir/top.txt", "top");
        let src = scratch.root.join("dir");
        let opts = scratch.force_temp_opts();

        let dest = delete(&src, &opts).unwrap();
        let Destination::Temp(dest_path) = dest else {
            panic!("expected Temp destination, got {dest:?}");
        };

        assert!(!src.exists());
        assert_eq!(fs::read_to_string(dest_path.join("top.txt")).unwrap(), "top");
        assert_eq!(
            fs::read_to_string(dest_path.join("inner/deep.txt")).unwrap(),
            "deep"
        );
    }

    #[test]
    fn batch_isolates_failures() {
        let scratch = Scratch::new();
        let good1 = scratch.file("one.txt", "1");
        let good2 = scratch.file("two.txt", "2");
        let missing = scratch.root.join("nope");
        let opts = scratch.force_temp_opts();

        let outcomes = delete_all([&good1, &missing, &good2], &opts);
        assert_eq!(outcomes.len(), 3);

        assert!(matches!(
            outcomes[0].result,
            Ok(Destination::Temp(_))
        ));
        assert!(matches!(
            outcomes[1].result,
            Err(Error::InvalidTarget { .. })
        ));
        assert!(matches!(
            outcomes[2].result,
            Ok(Destination::Temp(_))
        ));
        assert!(!good1.exists());
        assert!(!good2.exists());
    }

    #[test]
    fn name_collisions_get_suffixes() {
        let scratch = Scratch::new();
        let a = scratch.file("x/same.txt", "A");
        let b = scratch.file("y/same.txt", "B");
        let opts = scratch.force_temp_opts();

        let dest_a = delete(&a, &opts).unwrap();
        let dest_b = delete(&b, &opts).unwrap();
        let (Destination::Temp(a_path), Destination::Temp(b_path)) = (dest_a, dest_b) else {
            panic!("expected Temp destinations");
        };

        assert_eq!(a_path, opts.temp_root().join("same.txt"));
        assert_eq!(b_path, opts.temp_root().join("same.txt.1"));
        assert_eq!(fs::read_to_string(&a_path).unwrap(), "A");
        assert_eq!(fs::read_to_string(&b_path).unwrap(), "B");
    }

    #[test]
    fn missing_target_is_hard_error_without_fallback() {
        let scratch = Scratch::new();
        let missing = scratch.root.join("gone");
        // Default options: fallback enabled — a missing file must still be a
        // hard error, and nothing may be dropped into the fallback dir.
        let opts = Options {
            temp_dir: Some(scratch.root.join("fallback")),
            ..Options::default()
        };

        let err = delete(&missing, &opts).unwrap_err();
        assert!(matches!(err, Error::InvalidTarget { .. }));
        assert!(!opts.temp_root().exists() || fs::read_dir(opts.temp_root()).unwrap().next().is_none());
    }

    #[test]
    fn root_targets_are_rejected() {
        let scratch = Scratch::new();
        let opts = scratch.force_temp_opts();

        for target in [PathBuf::from("/"), PathBuf::from(".."), PathBuf::from(".")] {
            let err = delete(&target, &opts).unwrap_err();
            assert!(matches!(err, Error::InvalidTarget { .. }), "{target:?}");
        }
    }

    /// With the default options the item must disappear from its original
    /// location: into the real OS trash where one exists, otherwise into the
    /// temp fallback (both are `Ok`).
    #[test]
    fn default_options_remove_the_item() {
        let scratch = Scratch::new();
        let src = scratch.file("real-trash-probe.txt", "probe");
        let opts = Options::default();

        let dest = delete(&src, &opts).unwrap();
        assert!(!src.exists());
        match dest {
            Destination::Trash | Destination::Temp(_) => {}
        }
    }

    #[test]
    fn unicode_and_spaced_names_roundtrip() {
        let scratch = Scratch::new();
        let src = scratch.file("空 目录/文件 名.txt", "内容");
        let opts = scratch.force_temp_opts();

        let dest = delete(&src, &opts).unwrap();
        let Destination::Temp(dest_path) = dest else {
            panic!("expected Temp destination, got {dest:?}");
        };

        assert_eq!(dest_path.file_name().unwrap().to_str().unwrap(), "文件 名.txt");
        assert_eq!(fs::read_to_string(&dest_path).unwrap(), "内容");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_moves_as_a_link_and_keeps_its_target() {
        let scratch = Scratch::new();
        let target = scratch.file("target.txt", "T");
        let link = scratch.root.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let opts = scratch.force_temp_opts();

        let dest = delete(&link, &opts).unwrap();
        let Destination::Temp(dest_path) = dest else {
            panic!("expected Temp destination, got {dest:?}");
        };

        assert!(dest_path.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(&dest_path).unwrap(), target);
        assert!(target.exists(), "symlink target must stay intact");
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_can_be_moved() {
        let scratch = Scratch::new();
        let link = scratch.root.join("dangling");
        std::os::unix::fs::symlink(scratch.root.join("gone-target"), &link).unwrap();
        let opts = scratch.force_temp_opts();

        let dest = delete(&link, &opts).unwrap();
        let Destination::Temp(dest_path) = dest else {
            panic!("expected Temp destination, got {dest:?}");
        };
        assert!(dest_path.symlink_metadata().unwrap().file_type().is_symlink());
    }

    #[test]
    fn empty_batch_returns_empty() {
        let outcomes: Vec<ItemOutcome> = delete_all(Vec::<PathBuf>::new(), &Options::default());
        assert!(outcomes.is_empty());
    }

    #[test]
    fn duplicate_paths_second_attempt_reports_missing() {
        let scratch = Scratch::new();
        let f = scratch.file("dup.txt", "d");
        let opts = scratch.force_temp_opts();

        let outcomes = delete_all([&f, &f], &opts);
        assert!(matches!(outcomes[0].result, Ok(Destination::Temp(_))));
        assert!(matches!(
            outcomes[1].result,
            Err(Error::InvalidTarget { .. })
        ));
    }

    #[test]
    fn hidden_files_are_moved() {
        let scratch = Scratch::new();
        let f = scratch.file(".hidden", "h");
        let opts = scratch.force_temp_opts();

        let dest = delete(&f, &opts).unwrap();
        let Destination::Temp(dest_path) = dest else {
            panic!("expected Temp destination, got {dest:?}");
        };
        assert_eq!(dest_path.file_name().unwrap().to_str().unwrap(), ".hidden");
    }

    #[cfg(unix)]
    #[test]
    fn unwritable_parent_fails_with_an_error() {
        use std::os::unix::fs::PermissionsExt;

        let scratch = Scratch::new();
        let dir = scratch.root.join("ro-dir");
        fs::create_dir(&dir).unwrap();
        let f = scratch.file("ro-dir/locked.txt", "L");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();

        let result = delete(&f, &scratch.force_temp_opts());

        // Restore permissions first so scratch cleanup succeeds even if the
        // assertions below panic.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(result.is_err(), "moving out of an unwritable dir must fail");
        assert!(f.exists(), "source must remain on failure");
    }

    #[test]
    fn relative_paths_resolve_against_cwd() {
        // Uses a unique child of the *current* directory, since `delete_all`
        // must accept relative input like trash-rs does.
        let scratch = Scratch::new();
        let unique = format!(".trash-pro-cwd-probe-{}", std::process::id());
        let abs = std::env::current_dir().unwrap().join(&unique);
        fs::write(&abs, "rel").unwrap();

        let opts = Options {
            force_temp: true,
            temp_dir: Some(scratch.root.join("fallback")),
            ..Options::default()
        };
        let dest = delete(&unique, &opts);

        let Destination::Temp(dest_path) = dest.unwrap() else {
            panic!("expected Temp destination");
        };
        assert!(!abs.exists());
        assert_eq!(dest_path.file_name().unwrap().to_str().unwrap(), unique);
    }

    #[test]
    fn concurrent_reservations_never_collide() {
        use std::sync::{Arc, Barrier};

        let scratch = Scratch::new();
        let base = scratch.root.join("dest");
        fs::create_dir_all(&base).unwrap();
        let barrier = Arc::new(Barrier::new(8));

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let base = base.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    reserve_unique_dest(&base, OsStr::new("race.txt"), ItemKind::File).unwrap()
                })
            })
            .collect();

        let mut dests: Vec<PathBuf> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        dests.sort();
        let unique = dests.len();
        dests.dedup();
        assert_eq!(unique, 8, "every racer must get its own name: {dests:?}");
        for d in &dests {
            assert!(d.symlink_metadata().is_ok(), "reservation must exist: {d:?}");
        }
    }

    // ---- fallback classification (injected trash backend) ----
    //
    // The real OS trash cannot be forced to fail portably (on macOS,
    // NSFileManager even ignores `$HOME`), so these tests drive
    // `delete_inner_with` with fake backends to pin down the fallback rules.

    fn unavailable() -> trash::Error {
        trash::Error::Unknown {
            description: "no trash on this system".into(),
        }
    }

    #[test]
    fn backend_unavailable_falls_back_to_temp() {
        let scratch = Scratch::new();
        let src = scratch.file("fall.txt", "F");
        let opts = scratch.trash_then_fallback_opts();
        let mut flag = false;

        let dest = delete_inner_with(&src, &opts, &mut flag, |_| Err(unavailable())).unwrap();

        let Destination::Temp(dest_path) = dest else {
            panic!("expected Temp fallback, got {dest:?}");
        };
        assert!(flag, "backend outage must be remembered for the batch");
        assert!(!src.exists());
        assert_eq!(fs::read_to_string(&dest_path).unwrap(), "F");
    }

    #[test]
    fn backend_unavailable_without_fallback_reports_error() {
        let scratch = Scratch::new();
        let src = scratch.file("nofall.txt", "N");
        let opts = Options {
            fallback_to_temp: false,
            force_temp: false,
            temp_dir: Some(scratch.root.join("fallback")),
        };
        let mut flag = false;

        let err = delete_inner_with(&src, &opts, &mut flag, |_| Err(unavailable())).unwrap_err();

        assert!(matches!(err, Error::Trash { .. }));
        assert!(src.exists(), "source must remain when nothing can take it");
    }

    #[test]
    fn hard_errors_never_fall_back() {
        let scratch = Scratch::new();
        let src = scratch.file("hard.txt", "H");
        let opts = scratch.trash_then_fallback_opts();
        let mut flag = false;

        let err = delete_inner_with(&src, &opts, &mut flag, |_| {
            Err(trash::Error::CouldNotAccess {
                target: "hard.txt".into(),
            })
        })
        .unwrap_err();

        assert!(matches!(err, Error::Trash { .. }));
        assert!(!flag, "per-item errors must not poison the batch cache");
        assert!(src.exists());
        assert!(
            fs::read_dir(opts.temp_root()).map(|mut d| d.next().is_none()).unwrap_or(true),
            "hard errors must not touch the fallback dir"
        );
    }

    #[test]
    fn cached_outage_skips_the_backend_for_later_items() {
        use std::cell::Cell;
        use std::rc::Rc;

        let scratch = Scratch::new();
        let a = scratch.file("cache-a.txt", "a");
        let b = scratch.file("cache-b.txt", "b");
        let opts = scratch.trash_then_fallback_opts();
        let calls = Rc::new(Cell::new(0));
        let mut flag = false;

        let first_calls = Rc::clone(&calls);
        let first = delete_inner_with(&a, &opts, &mut flag, move |_| {
            first_calls.set(first_calls.get() + 1);
            Err(unavailable())
        });
        let second = delete_inner_with(&b, &opts, &mut flag, |_| {
            calls.set(calls.get() + 1);
            Err(unavailable())
        });

        assert!(matches!(first.unwrap(), Destination::Temp(_)));
        assert!(matches!(second.unwrap(), Destination::Temp(_)));
        assert_eq!(calls.get(), 1, "second item must not retry a dead backend");
    }

    #[cfg(unix)]
    #[test]
    fn both_trash_and_fallback_failing_reports_both() {
        use std::os::unix::fs::PermissionsExt;

        let scratch = Scratch::new();
        let src = scratch.file("both.txt", "B");
        let ro = scratch.root.join("ro-tmp");
        fs::create_dir(&ro).unwrap();
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o555)).unwrap();
        let opts = Options {
            fallback_to_temp: true,
            force_temp: false,
            temp_dir: Some(ro.join("child")),
        };
        let mut flag = false;

        let err = delete_inner_with(&src, &opts, &mut flag, |_| Err(unavailable())).unwrap_err();
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(matches!(err, Error::Both { .. }), "got {err:?}");
        assert!(src.exists());
    }
}
