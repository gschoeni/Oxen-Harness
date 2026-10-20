//! Swapping a verified archive in for the running program. Both installs
//! stage the new files beside the old ones (same volume, so the final step
//! is a rename) and only then replace them, so a failure part-way leaves
//! the current version working.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::UpdateError;

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> UpdateError + '_ {
    move |source| UpdateError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Replace the CLI binary at `exe` with the `oxen-harness` binary inside a
/// release `.tar.gz`. The running process keeps its old inode, so this is
/// safe mid-run; the next launch is the new version.
pub fn install_cli(archive: &[u8], exe: &Path) -> Result<(), UpdateError> {
    let binary = binary_from_archive(archive)?;
    let staged = exe.with_file_name(".oxen-harness.update");
    fs::write(&staged, &binary).map_err(io(&staged))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755)).map_err(io(&staged))?;
    }
    fs::rename(&staged, exe).map_err(|source| {
        // Best-effort: the rename failure is what the user needs to see.
        let _ = fs::remove_file(&staged);
        UpdateError::Io {
            path: exe.to_path_buf(),
            source,
        }
    })
}

/// The `oxen-harness` file from a CLI release archive
/// (`oxen-harness-<v>-<target>/oxen-harness`).
fn binary_from_archive(archive: &[u8]) -> Result<Vec<u8>, UpdateError> {
    let bad =
        |e: std::io::Error| UpdateError::Install(format!("the update archive is unreadable: {e}"));
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries().map_err(bad)? {
        let mut entry = entry.map_err(bad)?;
        let is_binary = entry
            .path()
            .map_err(bad)?
            .file_name()
            .is_some_and(|n| n == "oxen-harness");
        if is_binary && entry.header().entry_type().is_file() {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(bad)?;
            return Ok(bytes);
        }
    }
    Err(UpdateError::Install(
        "the update archive has no oxen-harness binary in it".into(),
    ))
}

/// The `.app` bundle a macOS executable runs from
/// (`…/oxen-harness.app/Contents/MacOS/oxen-harness-app`).
pub fn app_bundle_of(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
}

/// Replace the `.app` bundle at `bundle` with the one inside a release
/// `<name>.app.tar.gz`. The app is relaunched by the caller afterwards.
pub fn install_app_bundle(archive: &[u8], bundle: &Path) -> Result<(), UpdateError> {
    // macOS runs a downloaded app that was never moved out of its disk image
    // or Downloads from a read-only, randomized copy; replacing that copy
    // would do nothing.
    if bundle
        .components()
        .any(|c| c.as_os_str() == "AppTranslocation")
    {
        return Err(UpdateError::Install(
            "macOS is running this copy from a temporary location: move oxen-harness to \
             Applications, reopen it, and update again"
                .into(),
        ));
    }
    let parent = bundle
        .parent()
        .ok_or_else(|| UpdateError::Install(format!("{} has no folder", bundle.display())))?;
    let staging = parent.join(format!(".oxen-harness-update-{}", std::process::id()));
    let result = swap_bundle(archive, bundle, &staging);
    // Best-effort cleanup of the staging folder (the old bundle, or a
    // half-unpacked new one); the swap's own result is what matters.
    let _ = fs::remove_dir_all(&staging);
    result
}

fn swap_bundle(archive: &[u8], bundle: &Path, staging: &Path) -> Result<(), UpdateError> {
    let unpacked = staging.join("new");
    fs::create_dir_all(&unpacked).map_err(io(&unpacked))?;
    // `unpack` refuses entries that would land outside the folder.
    tar::Archive::new(flate2::read::GzDecoder::new(archive))
        .unpack(&unpacked)
        .map_err(|e| UpdateError::Install(format!("the update archive is unreadable: {e}")))?;
    let fresh = fs::read_dir(&unpacked)
        .map_err(io(&unpacked))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.is_dir() && p.extension().is_some_and(|e| e == "app"))
        .ok_or_else(|| {
            UpdateError::Install("the update archive has no .app bundle in it".into())
        })?;
    let old = staging.join("old.app");
    fs::rename(bundle, &old).map_err(io(bundle))?;
    if let Err(source) = fs::rename(&fresh, bundle) {
        // Put the running version back before reporting.
        if let Err(restore) = fs::rename(&old, bundle) {
            return Err(UpdateError::Install(format!(
                "could not install the update ({source}) or restore {} ({restore}); \
                 the previous version is at {}",
                bundle.display(),
                old.display()
            )));
        }
        return Err(UpdateError::Io {
            path: bundle.to_path_buf(),
            source,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `.tar.gz` holding `files` (path, contents).
    fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (path, body) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, path, *body).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn replaces_the_cli_binary_with_the_one_in_the_archive() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("oxen-harness");
        fs::write(&exe, b"old").unwrap();
        let tgz = archive(&[
            ("oxen-harness-0.0.9-t/README.md", b"readme"),
            ("oxen-harness-0.0.9-t/oxen-harness", b"new"),
        ]);
        install_cli(&tgz, &exe).unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&exe).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }

        let err = install_cli(&archive(&[("x/README.md", b"r")]), &exe).unwrap_err();
        assert!(err.to_string().contains("no oxen-harness binary"), "{err}");
        assert_eq!(
            fs::read(&exe).unwrap(),
            b"new",
            "a bad archive leaves the binary alone"
        );
    }

    #[test]
    fn swaps_the_app_bundle_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("oxen-harness.app");
        fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        fs::write(bundle.join("Contents/MacOS/oxen-harness-app"), b"old").unwrap();
        fs::write(bundle.join("Contents/stale.txt"), b"gone after").unwrap();

        let tgz = archive(&[("oxen-harness.app/Contents/MacOS/oxen-harness-app", b"new")]);
        install_app_bundle(&tgz, &bundle).unwrap();
        assert_eq!(
            fs::read(bundle.join("Contents/MacOS/oxen-harness-app")).unwrap(),
            b"new"
        );
        assert!(
            !bundle.join("Contents/stale.txt").exists(),
            "the old bundle is replaced whole"
        );
        let left: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(left.len(), 1, "only the bundle remains: {left:?}");

        let err = install_app_bundle(&archive(&[("readme", b"r")]), &bundle).unwrap_err();
        assert!(err.to_string().contains("no .app bundle"), "{err}");
        assert!(bundle.join("Contents/MacOS/oxen-harness-app").exists());
    }

    #[test]
    fn finds_the_bundle_of_an_executable_and_refuses_translocated_copies() {
        let exe = Path::new("/Applications/oxen-harness.app/Contents/MacOS/oxen-harness-app");
        assert_eq!(
            app_bundle_of(exe).as_deref(),
            Some(Path::new("/Applications/oxen-harness.app"))
        );
        assert_eq!(
            app_bundle_of(Path::new("/usr/local/bin/oxen-harness")),
            None
        );
        let moved = Path::new("/private/var/folders/x/AppTranslocation/abc/d/oxen-harness.app");
        let err = install_app_bundle(b"", moved).unwrap_err();
        assert!(
            err.to_string()
                .contains("move oxen-harness to Applications"),
            "{err}"
        );
    }
}
