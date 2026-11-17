//! Desktop-owned maintenance for bytes already verified by the official updater.
//!
//! Discovery, download and signature verification stay in tauri-plugin-updater.
//! These adapters own OS publication, authorization and the installer handoff.
use std::path::Path;
use tauri::AppHandle;

/// Used only after the cached bytes pass checksum and signature verification.
/// Missing legacy metadata is inferred from the actual package, never guessed
/// from the running package type alone.
pub(crate) fn validate_cached_package(bytes: &[u8], kind: &str) -> Result<(), String> {
    use std::io::Read;
    let windows = |payload: &[u8]| {
        let msi = payload.starts_with(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]);
        let exe = payload.starts_with(b"MZ")
            && [
                b"OPENBITFUN_MAINTENANCE_V1".as_slice(),
                b"NullsoftInst".as_slice(),
            ]
            .iter()
            .any(|marker| payload.windows(marker.len()).any(|part| part == *marker));
        (kind != "msi" && exe) || (kind != "nsis" && msi)
    };
    let valid = match kind {
        "nsis" | "msi" | "unbundled" if kind != "unbundled" || cfg!(windows) => {
            if bytes.starts_with(b"PK\x03\x04") {
                let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
                    .map_err(|error| error.to_string())?;
                let mut found = None;
                for index in 0..archive.len() {
                    let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
                    if !entry.is_dir()
                        && std::path::Path::new(entry.name())
                            .extension()
                            .is_some_and(|ext| {
                                ext.eq_ignore_ascii_case("exe") || ext.eq_ignore_ascii_case("msi")
                            })
                    {
                        if found.is_some() {
                            return Err("Legacy update archive contains multiple installers; cache was retained".into());
                        }
                        let mut payload = Vec::new();
                        entry
                            .read_to_end(&mut payload)
                            .map_err(|error| error.to_string())?;
                        found = Some(windows(&payload));
                    }
                }
                found.unwrap_or(false)
            } else {
                windows(bytes)
            }
        }
        "deb" => bytes.starts_with(b"!<arch>\n"),
        "rpm" => bytes.starts_with(&[0xed, 0xab, 0xee, 0xdb]),
        "appimage" if !bytes.starts_with(&[0x1f, 0x8b]) => {
            bytes.starts_with(b"\x7fELF") && matches!(bytes.get(8..11), Some(b"AI\x01" | b"AI\x02"))
        }
        "app" | "appimage" => {
            let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
            let mut apps = std::collections::HashSet::new();
            let mut info = false;
            let mut executable = false;
            let mut appimages = 0;
            for entry in archive.entries().map_err(|error| error.to_string())? {
                let mut entry = entry.map_err(|error| error.to_string())?;
                let path = entry
                    .path()
                    .map_err(|error| error.to_string())?
                    .into_owned();
                if path.components().any(|part| {
                    matches!(
                        part,
                        std::path::Component::ParentDir
                            | std::path::Component::RootDir
                            | std::path::Component::Prefix(_)
                    )
                }) {
                    return Err("Invalid legacy update archive path; cache was retained".into());
                }
                let path = path
                    .components()
                    .filter(|part| matches!(part, std::path::Component::Normal(_)))
                    .collect::<std::path::PathBuf>();
                if kind == "appimage"
                    && path.extension().is_some_and(|ext| ext == "AppImage")
                    && entry.header().entry_type().is_file()
                {
                    let mut header = [0u8; 11];
                    entry
                        .read_exact(&mut header)
                        .map_err(|error| error.to_string())?;
                    if !header.starts_with(b"\x7fELF")
                        || !matches!(&header[8..], b"AI\x01" | b"AI\x02")
                    {
                        return Err("Invalid cached AppImage; cache was retained".into());
                    }
                    appimages += 1;
                }
                if kind == "app" {
                    let name = path
                        .components()
                        .next()
                        .map(|part| part.as_os_str().to_owned());
                    if let Some(name) = name.filter(|name| {
                        std::path::Path::new(name)
                            .extension()
                            .is_some_and(|ext| ext == "app")
                    }) {
                        apps.insert(name.clone());
                        let relative = path
                            .strip_prefix(std::path::Path::new(&name))
                            .map_err(|error| error.to_string())?;
                        info |= relative == std::path::Path::new("Contents/Info.plist")
                            && entry.header().entry_type().is_file();
                        executable |= relative.starts_with("Contents/MacOS")
                            && entry.header().entry_type().is_file();
                    }
                }
            }
            if kind == "appimage" {
                appimages == 1
            } else {
                apps.len() == 1 && info && executable
            }
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err("Legacy cached package format cannot be verified for this installation; the cache was retained".into())
    }
}

#[cfg(test)]
mod cache_format_tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn legacy_windows_archives_accept_one_installer_and_reject_ambiguity() {
        let archive = |count| {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            for index in 0..count {
                zip.start_file(
                    format!("installer{index}.exe"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
                zip.write_all(b"MZfixture-NullsoftInst").unwrap();
            }
            zip.finish().unwrap().into_inner()
        };
        assert!(validate_cached_package(&archive(1), "nsis").is_ok());
        assert!(validate_cached_package(&archive(1), "msi").is_err());
        assert!(validate_cached_package(&archive(2), "nsis").is_err());
    }

    #[test]
    fn legacy_unix_archives_accept_dot_paths_and_reject_multiple_bundles() {
        let archive = |names: &[(&str, &[u8])]| {
            let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            let mut tar = tar::Builder::new(encoder);
            for (name, contents) in names {
                let mut header = tar::Header::new_gnu();
                header.set_size(contents.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                tar.append_data(&mut header, name, *contents).unwrap();
            }
            tar.into_inner().unwrap().finish().unwrap()
        };
        let files = [
            ("./OpenBitFun.app/Contents/Info.plist", b"plist".as_slice()),
            (
                "./OpenBitFun.app/Contents/MacOS/OpenBitFun",
                b"executable".as_slice(),
            ),
        ];
        assert!(validate_cached_package(&archive(&files), "app").is_ok());
        assert!(validate_cached_package(&archive(&files[..1]), "app").is_err());
        let mut multiple = files.to_vec();
        multiple.push(("Other.app/Contents/Info.plist", b"plist"));
        assert!(validate_cached_package(&archive(&multiple), "app").is_err());
        let image = b"\x7fELF\0\0\0\0AI\x02fixture";
        assert!(
            validate_cached_package(&archive(&[("./OpenBitFun.AppImage", image)]), "appimage")
                .is_ok()
        );
        assert!(validate_cached_package(
            &archive(&[("a.AppImage", image), ("b.AppImage", image)]),
            "appimage"
        )
        .is_err());
    }

    #[test]
    fn legacy_packages_require_a_recognizable_matching_format() {
        let native = b"MZfixture-OPENBITFUN_MAINTENANCE_V1";
        assert!(validate_cached_package(native, "nsis").is_ok());
        assert!(validate_cached_package(b"MZunknown executable", "nsis").is_err());
        assert!(validate_cached_package(native, "msi").is_err());
        assert!(validate_cached_package(b"!<arch>\nfixture", "deb").is_ok());
        assert!(validate_cached_package(b"!<arch>\nfixture", "rpm").is_err());
        assert!(validate_cached_package(b"\x7fELF\0\0\0\0AI\x02fixture", "appimage").is_ok());
        assert!(validate_cached_package(b"\x7fELFfixture", "appimage").is_err());
    }
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "macos", target_os = "linux", test))]
mod publication;
#[cfg(windows)]
mod windows;

pub(crate) fn install(
    app: &AppHandle,
    bytes: &[u8],
    version: &str,
    kind: &str,
    receipt: &Path,
) -> Result<(), String> {
    let _ = (app, version, kind, receipt);
    #[cfg(windows)]
    return windows::install(app, bytes, version, receipt).map_err(|error| error.to_string());
    #[cfg(target_os = "macos")]
    return macos::install(bytes).map_err(|error| error.to_string());
    #[cfg(target_os = "linux")]
    return linux::install(app, bytes, kind).map_err(|error| error.to_string());
    #[allow(unreachable_code, unused_variables)]
    Err("Application maintenance is unsupported on this platform".into())
}
