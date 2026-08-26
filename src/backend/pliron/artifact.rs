//! Failure-atomic artifact writes: every binary artifact is produced in a
//! uniquely named temporary file in the destination directory (same
//! filesystem, so the final `rename` is atomic) and renamed over the
//! destination only on success. An interrupted or failed build never leaves
//! a truncated artifact and always preserves an existing output.

use std::path::{Path, PathBuf};

use super::{PlironError, PlironErrorKind};

/// Run `produce` against a temporary sibling of `dest`, then atomically
/// rename the result over `dest`. On failure the temporary file is removed
/// and any existing `dest` is left untouched. The temporary name keeps the
/// destination's extension: external tools (clang) infer file kinds from
/// suffixes.
pub(super) fn write_atomic<T>(
    dest: &Path,
    produce: impl FnOnce(&Path) -> Result<T, PlironError>,
) -> Result<T, PlironError> {
    let temp = temp_sibling(dest);
    match produce(&temp) {
        Ok(value) => match std::fs::rename(&temp, dest) {
            Ok(()) => Ok(value),
            Err(error) => {
                let _ = std::fs::remove_file(&temp);
                Err(artifact_error(format!(
                    "cannot move finished artifact into place at {}: {error}",
                    dest.display()
                )))
            }
        },
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(error)
        }
    }
}

/// A hidden, per-process-and-call unique temporary sibling of `dest`,
/// preserving `dest`'s extension.
fn temp_sibling(dest: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = dest.file_name().unwrap_or_default().to_string_lossy();
    let ext = dest
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()))
        .unwrap_or_default();
    dest.with_file_name(format!(".{name}.{}.{unique}.tmp{ext}", std::process::id()))
}

fn artifact_error(message: String) -> PlironError {
    PlironError {
        function: None,
        kind: PlironErrorKind::Emit(message),
        location: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emit_error(message: &str) -> PlironError {
        artifact_error(message.to_string())
    }

    #[test]
    fn pliron_write_atomic_renames_on_success() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("out.o");
        write_atomic(&dest, |tmp| {
            std::fs::write(tmp, b"fresh").map_err(|e| emit_error(&e.to_string()))
        })
        .expect("atomic write succeeds");
        assert_eq!(std::fs::read(&dest).expect("dest exists"), b"fresh");
        let residue: Vec<_> = std::fs::read_dir(dir.path())
            .expect("readdir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(residue, vec![std::ffi::OsString::from("out.o")]);
    }

    #[test]
    fn pliron_write_atomic_failure_preserves_existing_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("out.o");
        std::fs::write(&dest, b"prior").expect("seed prior output");
        let result: Result<(), _> = write_atomic(&dest, |tmp| {
            std::fs::write(tmp, b"partial").map_err(|e| emit_error(&e.to_string()))?;
            Err(emit_error("tool failed"))
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(&dest).expect("dest survives"), b"prior");
        let residue: Vec<_> = std::fs::read_dir(dir.path())
            .expect("readdir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(residue, vec![std::ffi::OsString::from("out.o")]);
    }

    #[test]
    fn pliron_write_atomic_temp_keeps_extension() {
        let temp = temp_sibling(Path::new("/some/dir/out.bc"));
        let name = temp.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with(".out.bc."), "{name}");
        assert!(name.ends_with(".bc"), "{name}");
        assert_eq!(temp.parent(), Some(Path::new("/some/dir")));
    }
}
