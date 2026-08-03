//! The process-scoped `0700` directory a downloaded bundle lives in (`CON-005`).
//!
//! A secure-connect-bundle contains a private key. When cdm-rs downloads one it must land
//! somewhere, and `CON-005` fixes both the permissions — `0700`, owner only — and the lifetime:
//! removed when the run finishes **and** when it does not.
//!
//! Two mechanisms, because one is not enough:
//!
//! * a [`Drop`] guard, which covers normal completion, an early `return`, and a panic that
//!   unwinds (the release profile keeps `panic = "unwind"` precisely so this holds);
//! * a signal handler, which covers `SIGINT`, `SIGTERM` and `SIGHUP`, where nothing is dropped
//!   because the process does not unwind at all.
//!
//! The signal handler is installed once per process, on first use, and holds only the paths — not
//! the guards — so that it can run without owning anything the rest of the program is using.
//!
//! On a platform without Unix permissions the directory is still created and still removed; the
//! `0700` step has no equivalent and is skipped, which is documented rather than silently
//! different.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use cdm_core::{CdmError, ErrorKind, Side};

use crate::errors::side_error_from;

/// Paths the signal handler must remove. Registered by every live [`BundleTempDir`].
static REGISTERED: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();

fn registry() -> &'static Mutex<Vec<PathBuf>> {
    REGISTERED.get_or_init(|| Mutex::new(Vec::new()))
}

/// A temporary directory holding downloaded credential material (`CON-005`).
///
/// Dropping it removes the directory and everything in it.
#[derive(Debug)]
pub struct BundleTempDir {
    path: PathBuf,
}

impl BundleTempDir {
    /// Creates `<tmp>/cdm-scb-<pid>-<n>` with `0700` permissions and registers it for removal.
    pub fn new(side: Side) -> Result<Self, CdmError> {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "cdm-scb-{}-{}-{unique}",
            std::process::id(),
            side.as_str()
        ));

        std::fs::create_dir_all(&path).map_err(|e| {
            side_error_from(
                ErrorKind::Config,
                side,
                format!(
                    "cannot create the temporary bundle directory {}",
                    path.display()
                ),
                e,
            )
        })?;
        restrict_permissions(side, &path)?;

        if let Ok(mut registered) = registry().lock() {
            registered.push(path.clone());
        }
        install_signal_handler();

        Ok(Self { path })
    }

    /// The directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes a file into the directory, `0600`, and returns its path.
    pub fn write(&self, side: Side, name: &str, contents: &[u8]) -> Result<PathBuf, CdmError> {
        let path = self.path.join(name);
        std::fs::write(&path, contents).map_err(|e| {
            side_error_from(
                ErrorKind::Config,
                side,
                format!("cannot write {}", path.display()),
                e,
            )
        })?;
        restrict_file_permissions(side, &path)?;
        Ok(path)
    }
}

impl Drop for BundleTempDir {
    fn drop(&mut self) {
        remove(&self.path);
        if let Ok(mut registered) = registry().lock() {
            registered.retain(|path| path != &self.path);
        }
    }
}

/// Removes a registered directory, ignoring the failure modes that mean "already gone".
fn remove(path: &Path) {
    if let Err(error) = std::fs::remove_dir_all(path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(
                rule = "CON-005",
                "could not remove the temporary bundle directory {}: {error}",
                path.display()
            );
        }
    }
}

/// Removes every registered directory. Called by the signal handler, and safe to call twice.
pub fn cleanup_all() {
    let paths = match registry().lock() {
        Ok(mut registered) => std::mem::take(&mut *registered),
        Err(_) => return,
    };
    for path in paths {
        remove(&path);
    }
}

/// Installs the `SIGINT`/`SIGTERM`/`SIGHUP` handler, once per process (`CON-005`).
///
/// A Tokio runtime is required to install one; when there is none — a synchronous unit test, say
/// — the `Drop` guard is the whole story and nothing is installed.
fn install_signal_handler() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    if INSTALLED.get().is_some() {
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    INSTALLED.get_or_init(|| ());

    handle.spawn(async {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut interrupt = match signal(SignalKind::interrupt()) {
                Ok(stream) => stream,
                Err(e) => return tracing::warn!(rule = "CON-005", "cannot watch SIGINT: {e}"),
            };
            let mut terminate = match signal(SignalKind::terminate()) {
                Ok(stream) => stream,
                Err(e) => return tracing::warn!(rule = "CON-005", "cannot watch SIGTERM: {e}"),
            };
            let mut hangup = match signal(SignalKind::hangup()) {
                Ok(stream) => stream,
                Err(e) => return tracing::warn!(rule = "CON-005", "cannot watch SIGHUP: {e}"),
            };
            tokio::select! {
                _ = interrupt.recv() => {}
                _ = terminate.recv() => {}
                _ = hangup.recv() => {}
            }
        }
        #[cfg(not(unix))]
        {
            if tokio::signal::ctrl_c().await.is_err() {
                return;
            }
        }
        tracing::info!(
            rule = "CON-005",
            "removing downloaded secure-connect-bundles before terminating"
        );
        cleanup_all();
    });
}

#[cfg(unix)]
fn restrict_permissions(side: Side, path: &Path) -> Result<(), CdmError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
        side_error_from(
            ErrorKind::Config,
            side,
            format!("cannot set 0700 on {}", path.display()),
            e,
        )
    })
}

#[cfg(not(unix))]
fn restrict_permissions(_side: Side, _path: &Path) -> Result<(), CdmError> {
    // No Unix mode bits here. The directory is still process-scoped and still removed.
    Ok(())
}

#[cfg(unix)]
fn restrict_file_permissions(side: Side, path: &Path) -> Result<(), CdmError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| {
        side_error_from(
            ErrorKind::Config,
            side,
            format!("cannot set 0600 on {}", path.display()),
            e,
        )
    })
}

#[cfg(not(unix))]
fn restrict_file_permissions(_side: Side, _path: &Path) -> Result<(), CdmError> {
    Ok(())
}

// Tests may panic freely: a failed assertion *is* the reporting mechanism, and the no-panic rule
// (ERR-004) exists to protect production paths, not test bodies.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    #[test]
    fn con_005_the_directory_is_0700_and_the_file_0600() {
        let dir = BundleTempDir::new(Side::Origin).unwrap();
        let path = dir.write(Side::Origin, "scb.zip", b"pretend zip").unwrap();
        assert!(path.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir_mode = std::fs::metadata(dir.path()).unwrap().permissions().mode();
            assert_eq!(dir_mode & 0o777, 0o700, "{dir_mode:o}");
            let file_mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(file_mode & 0o777, 0o600, "{file_mode:o}");
        }
    }

    #[test]
    fn con_005_dropping_the_guard_removes_the_directory() {
        let path = {
            let dir = BundleTempDir::new(Side::Target).unwrap();
            dir.write(Side::Target, "scb.zip", b"credentials").unwrap();
            dir.path().to_path_buf()
        };
        assert!(
            !path.exists(),
            "the Drop guard must remove {}",
            path.display()
        );
    }

    #[test]
    fn con_005_cleanup_all_removes_a_registered_directory() {
        let dir = BundleTempDir::new(Side::Origin).unwrap();
        let path = dir.path().to_path_buf();
        cleanup_all();
        assert!(!path.exists());
        // And dropping the now-removed guard is not an error.
        drop(dir);
    }

    #[test]
    fn con_005_two_directories_do_not_collide() {
        let first = BundleTempDir::new(Side::Origin).unwrap();
        let second = BundleTempDir::new(Side::Origin).unwrap();
        assert_ne!(first.path(), second.path());
    }

    #[tokio::test]
    async fn con_005_the_signal_handler_installs_under_a_runtime() {
        // Installing twice must be harmless; the second call is a no-op.
        let _first = BundleTempDir::new(Side::Origin).unwrap();
        let _second = BundleTempDir::new(Side::Target).unwrap();
    }

    #[test]
    fn con_005_writing_to_a_removed_directory_is_an_error_not_a_panic() {
        let dir = BundleTempDir::new(Side::Origin).unwrap();
        std::fs::remove_dir_all(dir.path()).unwrap();
        let err = dir.write(Side::Origin, "scb.zip", b"x").unwrap_err();
        assert_eq!(err.kind(), cdm_core::ErrorKind::Config);
    }
}
