use anyhow::{Context, Result};
use std::fs;
use std::io;
use std::io::Cursor;
use std::path::Path;

/// Estimated install size in bytes (~200MB for typical Tauri app with WebView)
pub(super) const ESTIMATED_INSTALL_SIZE: u64 = 200 * 1024 * 1024;

/// Extract a zip archive to the target directory with an entry filter.
pub(super) fn extract_zip_with_filter(
    archive_path: &Path,
    target_dir: &Path,
    should_extract: fn(&Path) -> bool,
) -> Result<()> {
    let file = fs::File::open(archive_path)
        .with_context(|| format!("Failed to open archive: {}", archive_path.display()))?;

    let archive = zip::ZipArchive::new(file).with_context(|| "Failed to read zip archive")?;
    extract_zip_archive(archive, target_dir, should_extract)
}

/// Extract a zip archive from in-memory bytes with an entry filter.
pub(super) fn extract_zip_bytes_with_filter(
    archive_bytes: &[u8],
    target_dir: &Path,
    should_extract: fn(&Path) -> bool,
) -> Result<()> {
    let reader = Cursor::new(archive_bytes);
    let archive = zip::ZipArchive::new(reader).with_context(|| "Failed to read embedded zip")?;
    extract_zip_archive(archive, target_dir, should_extract)
}

fn extract_zip_archive<R: io::Read + io::Seek>(
    mut archive: zip::ZipArchive<R>,
    target_dir: &Path,
    should_extract: fn(&Path) -> bool,
) -> Result<()> {
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let name = file.name().context("Failed to decode zip entry path")?;
        let name = name.trim_end_matches('/');
        let rel_path = super::transaction::relative_path(name).map_err(anyhow::Error::msg)?;
        if !should_extract(&rel_path) {
            continue;
        }
        let out_path = target_dir.join(&rel_path);

        if file.is_dir() {
            fs::create_dir_all(&out_path).with_context(|| {
                format!(
                    "Failed to create installation directory: {}",
                    out_path.display()
                )
            })?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!(
                        "Failed to create installation directory: {}",
                        parent.display()
                    )
                })?;
            }
            let mut outfile = fs::File::create(&out_path).with_context(|| {
                format!("Failed to create installation file: {}", out_path.display())
            })?;
            io::copy(&mut file, &mut outfile).with_context(|| {
                format!(
                    "Failed to extract installation file: {}",
                    out_path.display()
                )
            })?;
        }
    }

    Ok(())
}

/// Copy files from source to target with a relative-path file filter.
pub(super) fn copy_directory_with_filter(
    source: &Path,
    target: &Path,
    should_copy_file: fn(&Path) -> bool,
) -> Result<u64> {
    copy_directory_internal(source, target, Path::new(""), should_copy_file)
}

fn copy_directory_internal(
    source: &Path,
    target: &Path,
    relative_prefix: &Path,
    should_copy_file: fn(&Path) -> bool,
) -> Result<u64> {
    let mut bytes_copied: u64 = 0;

    if !target.exists() {
        fs::create_dir_all(target).with_context(|| {
            format!(
                "Failed to create installation directory: {}",
                target.display()
            )
        })?;
    }

    for entry in fs::read_dir(source)
        .with_context(|| format!("Failed to read payload directory: {}", source.display()))?
    {
        let entry = entry.with_context(|| {
            format!(
                "Failed to read payload directory entry: {}",
                source.display()
            )
        })?;
        let file_type = entry.file_type().with_context(|| {
            format!(
                "Failed to inspect payload entry: {}",
                entry.path().display()
            )
        })?;
        let name = entry.file_name();
        let rel = relative_prefix.join(&name);
        let dest = target.join(&name);

        anyhow::ensure!(
            !file_type.is_symlink(),
            "Payload contains a symbolic link: {}",
            entry.path().display()
        );

        if file_type.is_dir() {
            bytes_copied += copy_directory_internal(&entry.path(), &dest, &rel, should_copy_file)?;
        } else {
            if !should_copy_file(&rel) {
                continue;
            }
            let size = entry
                .metadata()
                .with_context(|| {
                    format!("Failed to inspect payload file: {}", entry.path().display())
                })?
                .len();
            fs::copy(entry.path(), &dest).with_context(|| {
                format!(
                    "Failed to copy payload file from {} to {}",
                    entry.path().display(),
                    dest.display()
                )
            })?;
            bytes_copied += size;
        }
    }

    Ok(bytes_copied)
}

#[cfg(test)]
mod tests {
    use super::{
        copy_directory_with_filter, extract_zip_bytes_with_filter, extract_zip_with_filter,
    };
    use std::{fs, io, path::Path};

    fn assert_io_diagnostic(error: anyhow::Error, path: &Path) {
        let cause = error.downcast_ref::<io::Error>().unwrap();
        let os_error = cause.raw_os_error().unwrap();
        let message = format!("{error:#}");
        assert!(message.contains(&path.display().to_string()), "{message}");
        assert!(message.contains(&cause.to_string()), "{message}");
        assert!(
            message.contains(&format!("os error {os_error}")),
            "{message}"
        );
    }

    #[test]
    fn extraction_failure_preserves_target_path_and_os_error() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("installed");
        let blocked_file = target.join("resources/readme.txt");
        fs::create_dir_all(&blocked_file).unwrap();

        let error = extract_zip_bytes_with_filter(
            include_bytes!("fixtures/legacy-payload-zip-0.6.6.zip"),
            &target,
            |path| path == Path::new("resources/readme.txt"),
        )
        .unwrap_err();

        assert_io_diagnostic(error, &blocked_file);
    }

    #[test]
    fn extraction_directory_failure_preserves_path_and_os_error() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("installed");
        fs::create_dir(&target).unwrap();
        let blocked_directory = target.join("resources");
        fs::write(&blocked_directory, b"not a directory").unwrap();

        let error = extract_zip_bytes_with_filter(
            include_bytes!("fixtures/legacy-payload-zip-0.6.6.zip"),
            &target,
            |path| path == Path::new("resources/readme.txt"),
        )
        .unwrap_err();

        assert_io_diagnostic(error, &blocked_directory);
    }

    #[test]
    fn copy_failure_preserves_source_target_and_os_error() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("payload");
        fs::create_dir(&source).unwrap();
        let source_file = source.join("application.exe");
        fs::write(&source_file, b"new application").unwrap();
        let target = root.path().join("installed");
        let blocked_file = target.join("application.exe");
        fs::create_dir_all(&blocked_file).unwrap();

        let error = copy_directory_with_filter(&source, &target, |_| true).unwrap_err();
        assert!(format!("{error:#}").contains(&source_file.display().to_string()));
        assert_io_diagnostic(error, &blocked_file);
    }

    #[cfg(windows)]
    #[test]
    fn locked_installation_file_preserves_windows_error_code() {
        use std::os::windows::fs::OpenOptionsExt;

        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("installed");
        fs::create_dir_all(target.join("resources")).unwrap();
        let locked_file = target.join("resources/readme.txt");
        fs::write(&locked_file, b"old resource").unwrap();
        let _locked = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&locked_file)
            .unwrap();

        let error = extract_zip_bytes_with_filter(
            include_bytes!("fixtures/legacy-payload-zip-0.6.6.zip"),
            &target,
            |path| path == Path::new("resources/readme.txt"),
        )
        .unwrap_err();

        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().raw_os_error(),
            Some(32)
        );
        assert_io_diagnostic(error, &locked_file);
    }

    #[test]
    fn reads_legacy_payload_from_file_and_embedded_bytes() {
        // Frozen output from zip 0.6.6: directories, Stored/Deflated entries,
        // and a UTF-8 filename. Keep these bytes independent of the current writer.
        let payload = include_bytes!("fixtures/legacy-payload-zip-0.6.6.zip");
        for embedded in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let target = root.path().join("installed");
            let filter = |path: &Path| path != Path::new("skip.txt");
            if embedded {
                extract_zip_bytes_with_filter(payload, &target, filter).unwrap();
            } else {
                let archive = root.path().join("payload.zip");
                fs::write(&archive, payload).unwrap();
                extract_zip_with_filter(&archive, &target, filter).unwrap();
            }
            assert_eq!(
                fs::read(target.join("frontend/dist/index.html")).unwrap(),
                b"<html>legacy installer payload</html>"
            );
            assert_eq!(
                fs::read(target.join("resources/readme.txt")).unwrap(),
                b"legacy resource"
            );
            assert_eq!(
                fs::read(target.join("resources/\u{7528}\u{6237}.txt")).unwrap(),
                b"legacy unicode path"
            );
            assert!(!target.join("skip.txt").exists());
        }
    }
}
