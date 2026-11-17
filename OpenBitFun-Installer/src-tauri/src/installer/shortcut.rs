//! Windows shortcut (.lnk) creation for desktop and Start Menu.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::MAIN_APP_EXE;

const SHORTCUT_NAME: &str = "OpenBitFun.lnk";
const NESTED_START_MENU_DIR: &str = "OpenBitFun";

fn installation_shortcut_name(install_path: &Path) -> String {
    let label = super::commands::normalized_windows_install_path(install_path)
        .chars()
        .rev()
        .take(60)
        .collect::<String>()
        .chars()
        .rev()
        .map(|c| {
            if r#"\/:*?"<>|"#.contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect::<String>();
    format!(
        "OpenBitFun ({} - {}).lnk",
        label,
        &super::commands::windows_installation_id(install_path)[..12]
    )
}

fn shortcut_path(directory: &Path, install_path: &Path) -> Result<PathBuf> {
    let legacy = directory.join(SHORTCUT_NAME);
    if legacy.try_exists()?
        && super::commands::windows_path_eq_case_insensitive(
            &shortcut_target(&legacy)?,
            &install_path.join(MAIN_APP_EXE),
        )
    {
        return Ok(legacy);
    }
    Ok(directory.join(installation_shortcut_name(install_path)))
}

/// Create a desktop shortcut for OpenBitFun.
pub(super) fn create_desktop_shortcut(install_path: &Path) -> Result<()> {
    let desktop = desktop_dir(install_path)?;
    let shortcut_path = shortcut_path(&desktop, install_path)?;
    let exe_path = install_path.join(MAIN_APP_EXE);

    create_lnk(&shortcut_path, &exe_path, install_path)?;
    log::info!("Created desktop shortcut at {}", shortcut_path.display());
    Ok(())
}

/// Create a Start Menu shortcut for OpenBitFun.
pub(super) fn create_start_menu_shortcut(install_path: &Path) -> Result<()> {
    let start_menu = get_start_menu_dir(install_path)?;
    remove_nested_start_menu_shortcut(&start_menu, install_path)?;
    let shortcut_path = shortcut_path(&start_menu, install_path)?;
    let exe_path = install_path.join(MAIN_APP_EXE);

    create_lnk(&shortcut_path, &exe_path, install_path)?;
    log::info!("Created Start Menu shortcut at {}", shortcut_path.display());
    Ok(())
}

/// Remove desktop shortcut.
pub(super) fn remove_desktop_shortcut(install_path: &Path) -> Result<()> {
    let desktop = desktop_dir(install_path)?;
    remove_owned_shortcut(&desktop.join(SHORTCUT_NAME), install_path)?;
    remove_owned_shortcut(
        &desktop.join(installation_shortcut_name(install_path)),
        install_path,
    )
}

/// Remove Start Menu shortcut, including the nested folder layout.
pub(super) fn remove_start_menu_shortcut(install_path: &Path) -> Result<()> {
    let start_menu = get_start_menu_dir(install_path)?;
    let shortcut_path = start_menu.join(SHORTCUT_NAME);
    remove_owned_shortcut(&shortcut_path, install_path)?;
    remove_owned_shortcut(
        &start_menu.join(installation_shortcut_name(install_path)),
        install_path,
    )?;
    remove_nested_start_menu_shortcut(&start_menu, install_path)?;
    Ok(())
}

/// Get the current user's Start Menu Programs directory.
fn desktop_dir(install_path: &Path) -> Result<PathBuf> {
    if super::registry::is_machine_install(install_path).map_err(anyhow::Error::msg)? {
        Ok(PathBuf::from(std::env::var("PUBLIC")?).join("Desktop"))
    } else {
        dirs::desktop_dir().with_context(|| "Cannot find Desktop directory")
    }
}

fn get_start_menu_dir(install_path: &Path) -> Result<PathBuf> {
    let appdata = std::env::var(
        if super::registry::is_machine_install(install_path).map_err(anyhow::Error::msg)? {
            "PROGRAMDATA"
        } else {
            "APPDATA"
        },
    )
    .with_context(|| "Start Menu location is unavailable")?;
    Ok(PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs"))
}

fn remove_nested_start_menu_shortcut(start_menu: &Path, install_path: &Path) -> Result<()> {
    let nested = start_menu.join(NESTED_START_MENU_DIR);
    remove_owned_shortcut(&nested.join(SHORTCUT_NAME), install_path)?;
    // Preserve user-created shortcuts and subfolders.
    if nested.try_exists()? && std::fs::read_dir(&nested)?.next().is_none() {
        std::fs::remove_dir(nested)?;
    }
    Ok(())
}

fn shortcut_target(path: &Path) -> Result<PathBuf> {
    let output = super::commands::create_windows_silent_command(super::commands::system_powershell().map_err(anyhow::Error::msg)?)
        .args(["-NoProfile", "-NonInteractive", "-Command", "[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $s=New-Object -ComObject WScript.Shell; [Console]::Out.Write($s.CreateShortcut($env:OPENBITFUN_SHORTCUT_PATH).TargetPath)"])
        .env("OPENBITFUN_SHORTCUT_PATH", path).output()?;
    anyhow::ensure!(
        output.status.success(),
        "Cannot inspect shortcut {}",
        path.display()
    );
    Ok(PathBuf::from(String::from_utf8(output.stdout)?.trim()))
}

fn remove_owned_shortcut(path: &Path, install_path: &Path) -> Result<()> {
    if path.try_exists()?
        && super::commands::windows_path_eq_case_insensitive(
            &shortcut_target(path)?,
            &install_path.join(MAIN_APP_EXE),
        )
    {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Snapshot {
    files: Vec<Option<Vec<u8>>>,
    machine: bool,
    #[serde(default)]
    installation_files: Vec<Option<Vec<u8>>>,
}

pub(super) fn snapshot(install_path: &Path, machine: bool) -> Result<Snapshot> {
    let start = PathBuf::from(std::env::var(if machine {
        "PROGRAMDATA"
    } else {
        "APPDATA"
    })?)
    .join("Microsoft/Windows/Start Menu/Programs");
    let desktop = if machine {
        PathBuf::from(std::env::var("PUBLIC")?).join("Desktop")
    } else {
        dirs::desktop_dir().with_context(|| "Desktop is unavailable")?
    };
    let paths = [
        desktop.join(SHORTCUT_NAME),
        start.join(SHORTCUT_NAME),
        start.join(NESTED_START_MENU_DIR).join(SHORTCUT_NAME),
    ];
    let files = snapshot_files(&paths)?;
    let name = installation_shortcut_name(install_path);
    let installation_files = snapshot_files(&[desktop.join(&name), start.join(&name)])?;
    Ok(Snapshot {
        files,
        machine,
        installation_files,
    })
}

fn snapshot_files(paths: &[PathBuf]) -> Result<Vec<Option<Vec<u8>>>> {
    Ok(paths
        .iter()
        .map(|path| match std::fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        })
        .collect::<std::io::Result<Vec<_>>>()?)
}

pub(super) fn restore(install_path: &Path, snapshot: &Snapshot) -> Result<()> {
    anyhow::ensure!(
        snapshot.files.len() == 3,
        "Invalid shortcut recovery snapshot"
    );
    let start = if snapshot.machine {
        PathBuf::from(std::env::var("PROGRAMDATA")?).join("Microsoft/Windows/Start Menu/Programs")
    } else {
        PathBuf::from(std::env::var("APPDATA")?).join("Microsoft/Windows/Start Menu/Programs")
    };
    let desktop = if snapshot.machine {
        PathBuf::from(std::env::var("PUBLIC")?).join("Desktop")
    } else {
        dirs::desktop_dir().with_context(|| "Desktop is unavailable")?
    };
    let mut paths = vec![
        desktop.join(SHORTCUT_NAME),
        start.join(SHORTCUT_NAME),
        start.join(NESTED_START_MENU_DIR).join(SHORTCUT_NAME),
    ];
    anyhow::ensure!(
        snapshot.installation_files.is_empty() || snapshot.installation_files.len() == 2,
        "Invalid per-installation shortcut recovery snapshot"
    );
    if !snapshot.installation_files.is_empty() {
        let name = installation_shortcut_name(install_path);
        paths.extend([desktop.join(&name), start.join(&name)]);
    }
    restore_files(
        install_path,
        &paths,
        snapshot.files.iter().chain(&snapshot.installation_files),
    )
}

fn restore_files<'a>(
    install_path: &Path,
    paths: &[PathBuf],
    files: impl Iterator<Item = &'a Option<Vec<u8>>>,
) -> Result<()> {
    for (path, bytes) in paths.iter().zip(files) {
        if let Some(bytes) = bytes {
            if std::fs::read(path).is_ok_and(|current| current == *bytes) {
                continue;
            }
            if path.try_exists()? {
                anyhow::ensure!(
                    super::commands::windows_path_eq_case_insensitive(
                        &shortcut_target(path)?,
                        &install_path.join(MAIN_APP_EXE)
                    ),
                    "Shortcut changed during maintenance; recovery data retained: {}",
                    path.display()
                );
            }
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(path, bytes)?;
        } else {
            remove_owned_shortcut(path, install_path)?;
        }
    }
    Ok(())
}

/// Create a .lnk shortcut file using the mslnk crate.
fn create_lnk(shortcut_path: &Path, target: &Path, _working_dir: &Path) -> Result<()> {
    if shortcut_path.try_exists()? {
        anyhow::ensure!(
            super::commands::windows_path_eq_case_insensitive(
                &shortcut_target(shortcut_path)?,
                target
            ),
            "Shortcut belongs to another installation: {}",
            shortcut_path.display()
        );
    }
    let lnk = mslnk::ShellLink::new(target)
        .with_context(|| format!("Failed to create shell link for {}", target.display()))?;

    // Note: mslnk has limited API. For full control (icon, arguments, etc.),
    // consider using the windows crate with IShellLink COM interface.
    lnk.create_lnk(shortcut_path)
        .with_context(|| format!("Failed to write shortcut to {}", shortcut_path.display()))?;

    log::info!(
        "Created shortcut: {} -> {}",
        shortcut_path.display(),
        target.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_shortcuts_and_legacy_shortcuts_survive_other_installation_maintenance() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let links = temp.path().join("links");
        for path in [&first, &second, &links] {
            std::fs::create_dir(path).unwrap();
        }
        let ping = PathBuf::from(std::env::var_os("SYSTEMROOT").unwrap()).join("System32/ping.exe");
        for path in [&first, &second] {
            std::fs::copy(&ping, path.join(MAIN_APP_EXE)).unwrap();
        }
        let legacy = links.join(SHORTCUT_NAME);
        create_lnk(&legacy, &first.join(MAIN_APP_EXE), &first).unwrap();
        let additional = shortcut_path(&links, &second).unwrap();
        assert_ne!(legacy, additional);
        let before = snapshot_files(&[legacy.clone(), additional.clone()]).unwrap();
        create_lnk(&additional, &second.join(MAIN_APP_EXE), &second).unwrap();
        assert_eq!(shortcut_path(&links, &first).unwrap(), legacy);
        assert!(super::super::commands::windows_path_eq_case_insensitive(
            &shortcut_target(&legacy).unwrap(),
            &first.join(MAIN_APP_EXE)
        ));
        restore_files(
            &second,
            &[legacy.clone(), additional.clone()],
            before.iter(),
        )
        .unwrap();
        assert!(legacy.is_file());
        assert!(!additional.exists());
        create_lnk(&additional, &second.join(MAIN_APP_EXE), &second).unwrap();
        remove_owned_shortcut(&legacy, &second).unwrap();
        assert!(legacy.is_file());
        remove_owned_shortcut(&additional, &second).unwrap();
        assert!(legacy.is_file());
        assert!(!additional.exists());
    }

    #[test]
    fn legacy_shortcut_snapshots_roundtrip_without_new_required_fields() {
        let payload = serde_json::json!({"files": [null, [1, 2, 3], null], "machine": true});
        let snapshot: Snapshot = serde_json::from_value(payload.clone()).unwrap();
        assert!(snapshot.installation_files.is_empty());
        let roundtrip: Snapshot =
            serde_json::from_slice(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
        assert_eq!(roundtrip.files, snapshot.files);
        assert_eq!(roundtrip.machine, snapshot.machine);
        #[derive(serde::Deserialize, serde::Serialize)]
        struct LegacySnapshot {
            files: Vec<Option<Vec<u8>>>,
            machine: bool,
        }
        let old_reader: LegacySnapshot =
            serde_json::from_value(serde_json::to_value(&snapshot).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(old_reader).unwrap(), payload);
        assert_eq!(
            installation_shortcut_name(Path::new(r"C:\Apps\OpenBitFun")),
            installation_shortcut_name(Path::new("c:/apps/openbitfun/"))
        );
    }
}
