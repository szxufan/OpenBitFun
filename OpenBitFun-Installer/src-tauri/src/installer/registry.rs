//! Windows Registry operations for the installer.
//!
//! Handles:
//! - Uninstall registry entries (Add/Remove Programs)
//! - Install location under `Software\{publisher}\{productName}` (matches Tauri NSIS `MANUPRODUCTKEY`)
//!
//! Must stay in sync with `src/apps/desktop/tauri.conf.json`: `bundle.publisher` + `productName`.
//! Main exe name must match `super::MAIN_APP_EXE` (same as Tauri NSIS).

use anyhow::{Context, Result};
use std::path::Path;
use winreg::enums::*;
use winreg::RegKey;

use super::MAIN_APP_EXE;

const APP_NAME: &str = "OpenBitFun";
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\OpenBitFun";

/// Matches Tauri NSIS `MANUFACTURER` (`bundle.publisher`).
const TAURI_MANUFACTURER: &str = "OpenBitFun Team";
/// Matches Tauri NSIS `PRODUCTNAME` (`productName`).
const TAURI_PRODUCT_NAME: &str = "OpenBitFun";

/// `HKCU\Software\{TAURI_MANUFACTURER}\{TAURI_PRODUCT_NAME}` — same as Tauri `MANUPRODUCTKEY`.
fn tauri_manufacturer_product_key() -> String {
    format!(r"Software\{}\{}", TAURI_MANUFACTURER, TAURI_PRODUCT_NAME)
}

fn quote_windows_path(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

fn normalize_registry_path(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(trimmed)
        .trim();
    if unquoted.is_empty() {
        None
    } else {
        Some(unquoted.to_string())
    }
}

/// Register the application in Add/Remove Programs.
pub(super) fn register_uninstall_entry(
    install_path: &Path,
    version: &str,
    uninstall_command: &str,
) -> Result<()> {
    let hkcu = root_for(install_path)?;
    register_uninstall_entry_at(&hkcu, install_path, version, uninstall_command)
}

fn register_uninstall_entry_at(
    root: &RegKey,
    install_path: &Path,
    version: &str,
    uninstall_command: &str,
) -> Result<()> {
    let keys = registration_keys(root, install_path)?;
    let (key, _) = root
        .create_subkey(&keys[0])
        .with_context(|| "Failed to create uninstall registry key")?;

    let exe_path = install_path.join(MAIN_APP_EXE);

    // Establish ownership before optional metadata so interrupted first-time
    // registration remains recognizable during journal recovery.
    key.set_value("InstallLocation", &quote_windows_path(install_path))?;
    let display_name = if keys[0] == UNINSTALL_KEY {
        APP_NAME.to_string()
    } else {
        format!("{APP_NAME} ({})", install_path.display())
    };
    key.set_value("DisplayName", &display_name)?;
    key.set_value("DisplayVersion", &version)?;
    key.set_value("Publisher", &TAURI_MANUFACTURER)?;
    key.set_value("MainBinaryName", &MAIN_APP_EXE)?;
    key.set_value("DisplayIcon", &quote_windows_path(&exe_path))?;
    key.set_value("UninstallString", &uninstall_command)?;
    key.set_value(
        "QuietUninstallString",
        &format!("{uninstall_command} --quiet"),
    )?;
    key.set_value("NoModify", &1u32)?;
    key.set_value("NoRepair", &1u32)?;

    log::info!("Registered uninstall entry at {}", keys[0]);
    Ok(())
}

/// Same as Tauri NSIS `WriteRegStr SHCTX "${MANUPRODUCTKEY}" "" $INSTDIR` — used for default install dir / upgrades.
pub(super) fn register_tauri_install_location(install_path: &Path) -> Result<()> {
    let hkcu = root_for(install_path)?;
    register_tauri_install_location_at(&hkcu, install_path)
}

fn register_tauri_install_location_at(root: &RegKey, install_path: &Path) -> Result<()> {
    let path = registration_keys(root, install_path)?[1].clone();
    let (key, _) = root
        .create_subkey(&path)
        .with_context(|| format!("Failed to create registry key {}", path))?;
    let dir = install_path.to_string_lossy();
    key.set_value("", &dir.as_ref())?;
    log::info!("Registered Tauri install location at {}", path);
    Ok(())
}

/// Read install dir written by Tauri NSIS or this installer (`MANUPRODUCTKEY` default value).
pub(super) fn read_tauri_install_location() -> Option<String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let path = tauri_manufacturer_product_key();
    let key = hkcu.open_subkey(&path).ok()?;
    let s: String = key.get_value("").ok()?;
    normalize_registry_path(&s)
}

/// Remove only keys that still point to the selected installation.
pub(super) fn remove_tauri_install_location(install_path: &Path) -> Result<()> {
    for root in [maintenance_user_root()?, RegKey::predef(HKEY_LOCAL_MACHINE)] {
        remove_registration_keys(&root, install_path, false)?;
    }
    Ok(())
}
pub(super) fn remove_uninstall_entry(install_path: &Path) -> Result<()> {
    for root in [maintenance_user_root()?, RegKey::predef(HKEY_LOCAL_MACHINE)] {
        remove_registration_keys(&root, install_path, true)?;
    }
    Ok(())
}

fn remove_registration_keys(root: &RegKey, install_path: &Path, uninstall: bool) -> Result<()> {
    let (legacy, index, value) = if uninstall {
        (UNINSTALL_KEY.to_string(), 0, "InstallLocation")
    } else {
        (tauri_manufacturer_product_key(), 1, "")
    };
    for key in [legacy, installation_keys(install_path)[index].clone()] {
        delete_owned_key(root, install_path, &key, value)?;
    }
    Ok(())
}

fn delete_owned_key(root: &RegKey, install_path: &Path, path: &str, value: &str) -> Result<()> {
    match root.open_subkey(path) {
        Ok(key) => {
            let existing: String = key.get_value(value)?;
            if path_matches(&existing, install_path) {
                root.delete_subkey_all(path)?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
pub(super) fn remove_autostart_run_entry(install_path: &Path) -> Result<()> {
    let root = maintenance_user_root()?;
    let key = match root.open_subkey_with_flags(
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        KEY_READ | KEY_WRITE,
    ) {
        Ok(key) => key,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    match key.get_value::<String, _>(APP_NAME) {
        Ok(value) => {
            if command_targets(&value, install_path) {
                key.delete_value(APP_NAME)?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
pub(super) fn scope(
    path: &Path,
) -> std::result::Result<openbitfun_services_core::installation_scope::InstallationScope, String> {
    let owner = std::env::args()
        .skip(1)
        .take_while(|arg| !arg.eq_ignore_ascii_case("/ARGS"))
        .find_map(|arg| arg.strip_prefix("/OWNER_SID=").map(str::to_string));
    openbitfun_services_core::installation_scope::windows_installation_scope(
        path,
        APP_NAME,
        owner.as_deref(),
    )
}

pub(super) fn is_machine_install(path: &Path) -> std::result::Result<bool, String> {
    scope(path).map(|scope| scope.machine_install)
}

pub(super) fn validate_scope(path: &Path, all_users: bool) -> Result<()> {
    validate_scope_facts(scope(path).map_err(anyhow::Error::msg)?, all_users)
}

fn validate_scope_facts(
    scope: openbitfun_services_core::installation_scope::InstallationScope,
    all_users: bool,
) -> Result<()> {
    if all_users && scope.current_user_registration && !scope.machine_install {
        anyhow::bail!("This directory is a current-user installation; retain its scope or uninstall before changing scope");
    }
    Ok(())
}

pub(super) fn installed_version(path: &Path) -> Option<String> {
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
        .into_iter()
        .filter_map(|hive| read_installation_at(&RegKey::predef(hive), "registry", path))
        .find(|value| path_matches(&value.install_location, path))
        .and_then(|value| value.display_version)
}
fn root_for(path: &Path) -> Result<RegKey> {
    if is_machine_install(path).map_err(anyhow::Error::msg)? {
        Ok(RegKey::predef(HKEY_LOCAL_MACHINE))
    } else {
        maintenance_user_root()
    }
}
fn path_matches(value: &str, path: &Path) -> bool {
    normalize_registry_path(value)
        .is_some_and(|v| super::commands::windows_path_eq_case_insensitive(Path::new(&v), path))
}
fn command_targets(value: &str, path: &Path) -> bool {
    let value = value.trim();
    let program = if let Some(v) = value.strip_prefix('"') {
        v.split('"').next().unwrap_or("")
    } else {
        value.split_whitespace().next().unwrap_or("")
    };
    path_matches(program, &path.join(MAIN_APP_EXE))
}
fn installation_keys(path: &Path) -> [String; 2] {
    let id = super::commands::windows_installation_id(path);
    [
        format!("{UNINSTALL_KEY}-{id}"),
        format!("{}-{id}", tauri_manufacturer_product_key()),
    ]
}

// Keep updating legacy installations under their original keys. New directories
// get independent sibling keys, so uninstalling a legacy copy cannot erase them.
fn registration_keys(root: &RegKey, path: &Path) -> Result<[String; 2]> {
    let keys = installation_keys(path);
    let mut scoped_registration = false;
    for (name, value) in [(&keys[0], "InstallLocation"), (&keys[1], "")] {
        match root.open_subkey(name) {
            Ok(key) => {
                let existing: String = key.get_value(value)?;
                anyhow::ensure!(path_matches(&existing, path), "Installation registration belongs to another directory; registry data was preserved");
                scoped_registration = true;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if scoped_registration {
        return Ok(keys);
    }
    let legacy = [UNINSTALL_KEY.to_string(), tauri_manufacturer_product_key()];
    for (name, value) in [(&legacy[0], "InstallLocation"), (&legacy[1], "")] {
        match root.open_subkey(name) {
            Ok(key) => {
                let existing: String = key.get_value(value).context(
                    "Cannot establish existing installation ownership; registry data was preserved",
                )?;
                // Older desktop builds rewrote the manufacturer location even
                // in development. The owned uninstall record is authoritative;
                // a successful maintenance transaction repairs that stale hint.
                if path_matches(&existing, path) {
                    return Ok(legacy.clone());
                }
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(keys)
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Snapshot {
    machine: bool,
    keys: Vec<KeySnapshot>,
    #[serde(default)]
    installation_keys: Vec<KeySnapshot>,
    #[serde(default)]
    other_registration: Option<RegistrationSnapshot>,
    #[serde(default)]
    user_sid: Option<String>,
    #[serde(default)]
    user_values: Vec<(String, String, Option<(u32, Vec<u8>)>)>,
    #[serde(default)]
    context_keys: Vec<(String, Vec<(String, Vec<(String, u32, Vec<u8>)>)>)>,
}

type KeySnapshot = Option<Vec<(String, u32, Vec<u8>)>>;

#[derive(serde::Serialize, serde::Deserialize)]
struct RegistrationSnapshot {
    machine: bool,
    keys: Vec<KeySnapshot>,
    #[serde(default)]
    installation_keys: Vec<KeySnapshot>,
}

const CONTEXT_KEYS: [&str; 2] = [
    r"Software\Classes\Directory\Background\shell\OpenBitFun",
    r"Software\Classes\Directory\shell\OpenBitFun",
];

fn maintenance_user_sid() -> Result<String> {
    let sid = std::env::args()
        .skip(1)
        .take_while(|arg| !arg.eq_ignore_ascii_case("/ARGS"))
        .find_map(|arg| arg.strip_prefix("/OWNER_SID=").map(str::to_string))
        .map(Ok)
        .unwrap_or_else(super::elevation::owner_sid)
        .map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        sid.starts_with("S-1-")
            && sid
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'S' || b == b'-'),
        "Invalid maintenance user SID"
    );
    Ok(sid)
}

fn user_root(sid: &str) -> Result<RegKey> {
    Ok(RegKey::predef(HKEY_USERS).open_subkey_with_flags(sid, KEY_READ | KEY_WRITE)?)
}

fn maintenance_user_root() -> Result<RegKey> {
    user_root(&maintenance_user_sid()?)
}

fn snapshot_tree(
    key: &RegKey,
    relative: &str,
    values: &mut Vec<(String, Vec<(String, u32, Vec<u8>)>)>,
) -> Result<()> {
    values.push((
        relative.into(),
        key.enum_values()
            .map(|v| v.map(|(n, v)| (n, v.vtype as u32, v.bytes)))
            .collect::<std::io::Result<_>>()?,
    ));
    for child in key.enum_keys() {
        let child = child?;
        let path = if relative.is_empty() {
            child.clone()
        } else {
            format!(r"{relative}\{child}")
        };
        snapshot_tree(&key.open_subkey(&child)?, &path, values)?;
    }
    Ok(())
}

fn raw_value(kind: u32, bytes: &[u8]) -> Result<winreg::RegValue> {
    let vtype = match kind {
        0 => REG_NONE,
        1 => REG_SZ,
        2 => REG_EXPAND_SZ,
        3 => REG_BINARY,
        4 => REG_DWORD,
        5 => REG_DWORD_BIG_ENDIAN,
        6 => REG_LINK,
        7 => REG_MULTI_SZ,
        8 => REG_RESOURCE_LIST,
        9 => REG_FULL_RESOURCE_DESCRIPTOR,
        10 => REG_RESOURCE_REQUIREMENTS_LIST,
        11 => REG_QWORD,
        _ => anyhow::bail!("Unsupported registry recovery value type"),
    };
    Ok(winreg::RegValue {
        vtype,
        bytes: bytes.to_vec(),
    })
}

fn snapshot_registration_keys(root: &RegKey, names: &[String]) -> Result<Vec<KeySnapshot>> {
    let mut keys = Vec::new();
    for name in names {
        keys.push(match root.open_subkey(name) {
            Ok(key) => Some(
                key.enum_values()
                    .map(|item| item.map(|(name, value)| (name, value.vtype as u32, value.bytes)))
                    .collect::<std::io::Result<Vec<_>>>()?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        });
    }
    Ok(keys)
}

pub(super) fn snapshot(path: &Path, machine: bool) -> Result<Snapshot> {
    let install_path = path;
    let sid = maintenance_user_sid()?;
    let user = user_root(&sid)?;
    let root = if machine {
        RegKey::predef(HKEY_LOCAL_MACHINE)
    } else {
        user_root(&sid)?
    };
    registration_keys(&root, path)?;
    // Keep the legacy two-key payload intact for interrupted older transactions.
    let keys = snapshot_registration_keys(
        &root,
        &[UNINSTALL_KEY.to_string(), tauri_manufacturer_product_key()],
    )?;
    let installation_keys = snapshot_registration_keys(&root, &installation_keys(path))?;
    let scope = scope(path).map_err(anyhow::Error::msg)?;
    let other_registration = if (machine && scope.current_user_registration)
        || (!machine && scope.machine_registration)
    {
        let other = if machine {
            user_root(&sid)?
        } else {
            RegKey::predef(HKEY_LOCAL_MACHINE)
        };
        Some(RegistrationSnapshot {
            machine: !machine,
            keys: snapshot_registration_keys(
                &other,
                &[UNINSTALL_KEY.to_string(), tauri_manufacturer_product_key()],
            )?,
            installation_keys: snapshot_registration_keys(&other, &self::installation_keys(path))?,
        })
    } else {
        None
    };
    let mut user_values = Vec::new();
    for (path, name) in [
        ("Environment", "Path"),
        (r"Software\Microsoft\Windows\CurrentVersion\Run", APP_NAME),
    ] {
        let value = match user
            .open_subkey(path)
            .and_then(|key| key.get_raw_value(name))
        {
            Ok(value) => Some((value.vtype as u32, value.bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        if let Some((kind, bytes)) = &value {
            use winreg::types::FromRegValue;
            let text = String::from_reg_value(&raw_value(*kind, bytes)?)?;
            let owned = if name == "Path" {
                text.split(';').any(|part| path_matches(part, install_path))
            } else {
                command_targets(&text, install_path)
            };
            if owned {
                user_values.push((path.into(), name.into(), value));
            }
        }
    }
    let mut context_keys = Vec::new();
    for path in CONTEXT_KEYS {
        match user.open_subkey(path) {
            Ok(key) => {
                let command = key.open_subkey("command")?.get_value::<String, _>("")?;
                if !command_targets(&command, install_path) {
                    continue;
                }
                let mut tree = Vec::new();
                snapshot_tree(&key, "", &mut tree)?;
                context_keys.push((path.into(), tree));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(Snapshot {
        machine,
        keys,
        installation_keys,
        other_registration,
        user_sid: Some(sid),
        user_values,
        context_keys,
    })
}

fn restore_registration_keys(
    root: &RegKey,
    install_path: &Path,
    snapshot: &Snapshot,
) -> Result<()> {
    restore_registration_values(
        root,
        install_path,
        &snapshot.keys,
        &snapshot.installation_keys,
    )
}

fn restore_registration_values(
    root: &RegKey,
    install_path: &Path,
    keys: &[KeySnapshot],
    installation: &[KeySnapshot],
) -> Result<()> {
    anyhow::ensure!(keys.len() == 2, "Invalid registry recovery snapshot");
    anyhow::ensure!(
        installation.is_empty() || installation.len() == 2,
        "Invalid per-installation registry recovery snapshot"
    );
    let mut paths = vec![
        (UNINSTALL_KEY.to_string(), "InstallLocation"),
        (tauri_manufacturer_product_key(), ""),
    ];
    if !installation.is_empty() {
        let scoped = installation_keys(install_path);
        paths.extend([
            (scoped[0].clone(), "InstallLocation"),
            (scoped[1].clone(), ""),
        ]);
    }
    let originals = keys.iter().chain(installation).collect::<Vec<_>>();
    let mut preserve_foreign = vec![false; paths.len()];
    for (index, ((path, name), original)) in paths.iter().zip(&originals).enumerate() {
        if index < 2 && !installation.is_empty() {
            // A new directory does not own the shared legacy records. Do not
            // roll back another installation's metadata or resurrect its keys.
            use winreg::types::FromRegValue;
            let originally_owned = original
                .as_ref()
                .and_then(|values| values.iter().find(|(key, _, _)| key == name))
                .map(|(_, kind, bytes)| -> Result<bool> {
                    Ok(path_matches(
                        &String::from_reg_value(&raw_value(*kind, bytes)?)?,
                        install_path,
                    ))
                })
                .transpose()?
                .unwrap_or(false);
            let currently_owned = match root
                .open_subkey(path)
                .and_then(|key| key.get_value::<String, _>(*name))
            {
                Ok(value) => path_matches(&value, install_path),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(error.into()),
            };
            if !originally_owned && !currently_owned {
                preserve_foreign[index] = true;
                continue;
            }
        }
        match root.open_subkey(&path) {
            Ok(key) => match key.get_value::<String, _>(*name) {
                Ok(value) => anyhow::ensure!(
                    path_matches(&value, install_path)
                        || original.as_ref().is_some_and(|values| {
                            values.iter().find(|(key, _, _)| key == name).is_some_and(
                                |(_, kind, bytes)| {
                                    use winreg::types::FromRegValue;
                                    raw_value(*kind, bytes)
                                        .ok()
                                        .and_then(|value| String::from_reg_value(&value).ok())
                                        .is_some_and(|previous| previous == value)
                                },
                            )
                        }),
                    "Registration changed during maintenance; recovery data retained"
                ),
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && original.is_none()
                        && key.enum_values().next().is_none() => {}
                Err(error) => return Err(error.into()),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    for (index, ((path, _), values)) in paths.iter().zip(originals).enumerate() {
        if preserve_foreign[index] {
            continue;
        }
        if let Some(values) = values {
            let (key, _) = root.create_subkey(path)?;
            let names = key
                .enum_values()
                .map(|v| v.map(|(n, _)| n))
                .collect::<std::io::Result<Vec<_>>>()?;
            for name in names {
                key.delete_value(name)?;
            }
            for (name, kind, bytes) in values {
                let vtype = match kind {
                    0 => REG_NONE,
                    1 => REG_SZ,
                    2 => REG_EXPAND_SZ,
                    3 => REG_BINARY,
                    4 => REG_DWORD,
                    5 => REG_DWORD_BIG_ENDIAN,
                    6 => REG_LINK,
                    7 => REG_MULTI_SZ,
                    8 => REG_RESOURCE_LIST,
                    9 => REG_FULL_RESOURCE_DESCRIPTOR,
                    10 => REG_RESOURCE_REQUIREMENTS_LIST,
                    11 => REG_QWORD,
                    _ => anyhow::bail!("Unsupported registry recovery value type"),
                };
                key.set_raw_value(
                    name,
                    &winreg::RegValue {
                        bytes: bytes.clone(),
                        vtype,
                    },
                )?;
            }
        } else {
            match root.delete_subkey_all(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
    Ok(())
}

pub(super) fn restore(install_path: &Path, snapshot: &Snapshot) -> Result<()> {
    if let Some(sid) = &snapshot.user_sid {
        anyhow::ensure!(
            sid == &maintenance_user_sid()?,
            "Registry recovery belongs to another user"
        );
    }
    let root = if snapshot.machine {
        RegKey::predef(HKEY_LOCAL_MACHINE)
    } else if let Some(sid) = &snapshot.user_sid {
        user_root(sid)?
    } else {
        RegKey::predef(HKEY_CURRENT_USER)
    };
    restore_registration_keys(&root, install_path, snapshot)?;
    if let Some(other) = &snapshot.other_registration {
        anyhow::ensure!(
            other.machine != snapshot.machine,
            "Invalid duplicate-registration recovery scope"
        );
        let root =
            if other.machine {
                RegKey::predef(HKEY_LOCAL_MACHINE)
            } else {
                user_root(snapshot.user_sid.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("Duplicate registration recovery has no owner")
                })?)?
            };
        restore_registration_values(&root, install_path, &other.keys, &other.installation_keys)?;
    }
    if let Some(sid) = &snapshot.user_sid {
        anyhow::ensure!(
            sid == &maintenance_user_sid()?,
            "Registry recovery belongs to another user"
        );
        let user = user_root(sid)?;
        for (path, name, value) in &snapshot.user_values {
            anyhow::ensure!(
                (path == "Environment" && name == "Path")
                    || (path == r"Software\Microsoft\Windows\CurrentVersion\Run"
                        && name == APP_NAME),
                "Invalid user registry recovery path"
            );
            if let Some((kind, bytes)) = value {
                use winreg::types::FromRegValue;
                let (key, _) = user.create_subkey(path)?;
                let original = raw_value(*kind, bytes)?;
                let current = match key.get_raw_value(name) {
                    Ok(value) => Some(value),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error.into()),
                };
                if let Some(current) = current {
                    if current.bytes == original.bytes && current.vtype == original.vtype {
                        continue;
                    }
                    anyhow::ensure!(
                        name == "Path",
                        "Autostart changed during maintenance; recovery data retained"
                    );
                    let previous = String::from_reg_value(&original)?;
                    let present = String::from_reg_value(&current)?;
                    let merged = merge_owned_path(&previous, &present, install_path);
                    let bytes = merged
                        .encode_utf16()
                        .chain(std::iter::once(0))
                        .flat_map(u16::to_le_bytes)
                        .collect();
                    key.set_raw_value(
                        name,
                        &winreg::RegValue {
                            bytes,
                            vtype: current.vtype,
                        },
                    )?;
                } else {
                    key.set_raw_value(name, &original)?;
                }
            }
        }
        for (path, tree) in &snapshot.context_keys {
            anyhow::ensure!(
                CONTEXT_KEYS.contains(&path.as_str()),
                "Invalid context-menu recovery path"
            );
            if let Ok(key) = user.open_subkey(format!(r"{path}\command")) {
                let value: String = key.get_value("")?;
                anyhow::ensure!(
                    command_targets(&value, install_path),
                    "Context menu changed during maintenance; recovery data retained"
                );
            }
            for (relative, values) in tree {
                anyhow::ensure!(
                    !relative.split('\\').any(|part| part == ".." || part == ".")
                        && !relative.starts_with('\\'),
                    "Invalid context-menu recovery subkey"
                );
                let (key, _) = user.create_subkey(if relative.is_empty() {
                    path.clone()
                } else {
                    format!(r"{path}\{relative}")
                })?;
                for (name, kind, bytes) in values {
                    key.set_raw_value(name, &raw_value(*kind, bytes)?)?;
                }
            }
        }
    }
    Ok(())
}

fn merge_owned_path(previous: &str, present: &str, install_path: &Path) -> String {
    let mut merged = present.to_string();
    if !present
        .split(';')
        .any(|part| path_matches(part, install_path))
    {
        if let Some(owned) = previous
            .split(';')
            .find(|part| path_matches(part, install_path))
        {
            if !merged.is_empty() && !merged.ends_with(';') {
                merged.push(';');
            }
            merged.push_str(owned);
        }
    }
    merged
}

#[cfg(test)]
mod maintenance_tests {
    use super::*;

    #[test]
    fn duplicate_user_registration_does_not_change_an_existing_machine_scope() {
        use openbitfun_services_core::installation_scope::InstallationScope;
        let mut facts = InstallationScope {
            current_user_registration: true,
            machine_registration: true,
            machine_install: true,
        };
        assert!(validate_scope_facts(facts, true).is_ok());
        facts.machine_install = false;
        assert!(validate_scope_facts(facts, true).is_err());
        assert!(validate_scope_facts(facts, false).is_ok());
        facts.current_user_registration = false;
        assert!(validate_scope_facts(facts, true).is_ok());
    }

    struct RegistryFixture(String, RegKey);

    impl RegistryFixture {
        fn new() -> Self {
            let name = format!(
                r"Software\OpenBitFunMaintenanceTests\{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            let (root, _) = RegKey::predef(HKEY_CURRENT_USER)
                .create_subkey(&name)
                .unwrap();
            Self(name, root)
        }
    }

    impl Drop for RegistryFixture {
        fn drop(&mut self) {
            RegKey::predef(HKEY_CURRENT_USER)
                .delete_subkey_all(&self.0)
                .unwrap();
        }
    }

    fn registration_snapshot(root: &RegKey, path: &Path) -> Snapshot {
        Snapshot {
            machine: false,
            keys: snapshot_registration_keys(
                root,
                &[UNINSTALL_KEY.to_string(), tauri_manufacturer_product_key()],
            )
            .unwrap(),
            installation_keys: snapshot_registration_keys(root, &installation_keys(path)).unwrap(),
            other_registration: None,
            user_sid: None,
            user_values: Vec::new(),
            context_keys: Vec::new(),
        }
    }

    #[test]
    fn duplicate_hive_records_are_removed_and_restored_without_touching_other_directories() {
        let fixture = RegistryFixture::new();
        let (user, _) = fixture.1.create_subkey("User").unwrap();
        let (machine, _) = fixture.1.create_subkey("Machine").unwrap();
        let ours = Path::new(r"C:\Apps\OpenBitFun");
        let other = Path::new(r"D:\Other\OpenBitFun");
        for hive in [&user, &machine] {
            register_uninstall_entry_at(hive, ours, "1.0.11", "uninstall.exe").unwrap();
            register_tauri_install_location_at(hive, ours).unwrap();
            register_uninstall_entry_at(hive, other, "1.0.11", "other.exe").unwrap();
        }
        let original = registration_snapshot(&user, ours);
        let extra = RegistrationSnapshot {
            machine: true,
            keys: snapshot_registration_keys(
                &machine,
                &[UNINSTALL_KEY.to_string(), tauri_manufacturer_product_key()],
            )
            .unwrap(),
            installation_keys: snapshot_registration_keys(&machine, &installation_keys(ours))
                .unwrap(),
        };
        for hive in [&user, &machine] {
            remove_registration_keys(hive, ours, false).unwrap();
            remove_registration_keys(hive, ours, true).unwrap();
            assert!(read_installation_at(hive, "fixture", ours).is_none());
            assert!(read_installation_at(hive, "fixture", other).is_some());
        }
        restore_registration_keys(&user, ours, &original).unwrap();
        let extra: RegistrationSnapshot =
            serde_json::from_slice(&serde_json::to_vec(&extra).unwrap()).unwrap();
        restore_registration_values(&machine, ours, &extra.keys, &extra.installation_keys).unwrap();
        assert!(read_installation_at(&user, "fixture", ours).is_some());
        assert!(read_installation_at(&machine, "fixture", ours).is_some());
    }

    #[test]
    fn parallel_installations_register_update_rollback_and_uninstall_independently() {
        let fixture = RegistryFixture::new();
        let root = &fixture.1;
        let first = Path::new(r"C:\Apps\OpenBitFun");
        let second = Path::new(r"D:\Apps\OpenBitFun");
        for path in [first, second] {
            register_uninstall_entry_at(root, path, "1.0.11", "uninstall.exe").unwrap();
            register_tauri_install_location_at(root, path).unwrap();
        }
        assert_ne!(installation_keys(first), installation_keys(second));
        // A legacy runtime may rewrite this global hint. It must not move an
        // already registered independent installation back to the global keys.
        root.create_subkey(tauri_manufacturer_product_key())
            .unwrap()
            .0
            .set_value("", &second.to_string_lossy().as_ref())
            .unwrap();
        assert_eq!(
            registration_keys(root, second).unwrap(),
            installation_keys(second)
        );
        let snapshot = registration_snapshot(root, second);
        register_uninstall_entry_at(root, second, "1.0.12", "uninstall.exe").unwrap();
        assert_eq!(
            read_installation_at(root, "fixture", first)
                .unwrap()
                .display_version
                .as_deref(),
            Some("1.0.11")
        );
        assert_eq!(
            read_installation_at(root, "fixture", second)
                .unwrap()
                .display_version
                .as_deref(),
            Some("1.0.12")
        );
        restore_registration_keys(root, second, &snapshot).unwrap();
        assert_eq!(
            read_installation_at(root, "fixture", second)
                .unwrap()
                .display_version
                .as_deref(),
            Some("1.0.11")
        );
        for (key, value) in installation_keys(first).iter().zip(["InstallLocation", ""]) {
            delete_owned_key(root, first, key, value).unwrap();
        }
        assert!(read_installation_at(root, "fixture", first).is_none());
        assert_eq!(
            read_first_installation(root, "fixture")
                .unwrap()
                .install_location,
            second.to_string_lossy()
        );
    }

    #[test]
    fn legacy_registration_and_recovery_payloads_remain_compatible() {
        let fixture = RegistryFixture::new();
        let root = &fixture.1;
        let legacy = Path::new(r"C:\Legacy\OpenBitFun");
        let second = Path::new(r"D:\Other\OpenBitFun");
        let (key, _) = root.create_subkey(UNINSTALL_KEY).unwrap();
        key.set_value("InstallLocation", &quote_windows_path(legacy))
            .unwrap();
        key.set_value("DisplayVersion", &"1.0.10").unwrap();
        let (hint, _) = root
            .create_subkey(tauri_manufacturer_product_key())
            .unwrap();
        hint.set_value("", &r"C:\OldDevelopmentHost").unwrap();
        let original = registration_snapshot(root, legacy);
        // This is the shape written before per-directory registration existed.
        let old_payload = serde_json::json!({"machine": false, "keys": original.keys});
        let old: Snapshot = serde_json::from_value(old_payload.clone()).unwrap();
        assert!(old.installation_keys.is_empty());
        register_uninstall_entry_at(root, legacy, "1.0.11", "uninstall.exe").unwrap();
        register_tauri_install_location_at(root, legacy).unwrap();
        assert_eq!(registration_keys(root, legacy).unwrap()[0], UNINSTALL_KEY);
        assert_eq!(
            hint.get_value::<String, _>("").unwrap(),
            legacy.to_string_lossy()
        );
        restore_registration_keys(root, legacy, &old).unwrap();
        assert_eq!(
            key.get_value::<String, _>("DisplayVersion").unwrap(),
            "1.0.10"
        );
        assert_eq!(
            hint.get_value::<String, _>("").unwrap(),
            r"C:\OldDevelopmentHost"
        );
        let before_new = registration_snapshot(root, second);
        register_uninstall_entry_at(root, second, "1.0.11", "uninstall.exe").unwrap();
        register_tauri_install_location_at(root, second).unwrap();
        assert_eq!(
            key.get_value::<String, _>("DisplayVersion").unwrap(),
            "1.0.10"
        );
        register_uninstall_entry_at(root, legacy, "1.0.12", "uninstall.exe").unwrap();
        restore_registration_keys(root, second, &before_new).unwrap();
        assert!(read_installation_at(root, "fixture", second).is_none());
        assert_eq!(
            read_installation_at(root, "fixture", legacy)
                .unwrap()
                .display_version
                .as_deref(),
            Some("1.0.12")
        );

        // Older readers still see exactly the original two legacy key snapshots.
        #[derive(serde::Deserialize, serde::Serialize)]
        struct LegacySnapshot {
            machine: bool,
            keys: Vec<KeySnapshot>,
        }
        let old_reader: LegacySnapshot =
            serde_json::from_value(serde_json::to_value(&original).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(old_reader).unwrap(), old_payload);
        let roundtrip: Snapshot =
            serde_json::from_slice(&serde_json::to_vec(&old).unwrap()).unwrap();
        restore_registration_keys(root, legacy, &roundtrip).unwrap();
    }

    #[test]
    fn directory_identity_is_stable_across_windows_path_spellings() {
        assert_eq!(
            installation_keys(Path::new(r"C:\Apps\OpenBitFun")),
            installation_keys(Path::new("c:/apps/openbitfun/"))
        );
    }

    #[test]
    fn path_recovery_preserves_concurrent_changes_and_is_idempotent() {
        let root = Path::new(r"C:\OpenBitFun");
        let recovered = merge_owned_path(r"C:\old;C:\OpenBitFun", r"C:\new;C:\other", root);
        assert_eq!(recovered, r"C:\new;C:\other;C:\OpenBitFun");
        assert_eq!(
            merge_owned_path(r"C:\old;C:\OpenBitFun", &recovered, root),
            recovered
        );
    }
    #[test]
    fn registry_removal_requires_exact_installation_ownership() {
        let name = format!(
            r"Software\OpenBitFunMaintenanceTests\{}",
            std::process::id()
        );
        let parent = RegKey::predef(HKEY_CURRENT_USER);
        let (fixture, _) = parent.create_subkey(&name).unwrap();
        let (key, _) = fixture.create_subkey("Install").unwrap();
        key.set_value("InstallLocation", &r"C:\other").unwrap();
        drop(key);
        delete_owned_key(
            &fixture,
            Path::new(r"C:\ours"),
            "Install",
            "InstallLocation",
        )
        .unwrap();
        assert!(fixture.open_subkey("Install").is_ok());
        delete_owned_key(
            &fixture,
            Path::new(r"C:\other"),
            "Install",
            "InstallLocation",
        )
        .unwrap();
        assert!(fixture.open_subkey("Install").is_err());
        drop(fixture);
        parent.delete_subkey_all(&name).unwrap();
    }
}

/// Data read from `Uninstall\OpenBitFun` (Tauri NSIS / this installer).
#[derive(Debug, Clone)]
pub(super) struct UninstallRegistryData {
    pub install_location: String,
    pub display_version: Option<String>,
    pub uninstall_string: Option<String>,
    pub hive: &'static str,
}

fn read_uninstall_key(
    root: &RegKey,
    name: &str,
    hive_name: &'static str,
) -> Option<UninstallRegistryData> {
    let key = root.open_subkey(name).ok()?;
    let install_location: String = key.get_value("InstallLocation").ok()?;
    let display_version: Option<String> = key.get_value("DisplayVersion").ok();
    let uninstall_string: Option<String> = key.get_value("UninstallString").ok();
    Some(UninstallRegistryData {
        install_location: normalize_registry_path(&install_location)?,
        display_version,
        uninstall_string,
        hive: hive_name,
    })
}

fn read_installation_at(
    root: &RegKey,
    hive: &'static str,
    path: &Path,
) -> Option<UninstallRegistryData> {
    [
        UNINSTALL_KEY.to_string(),
        installation_keys(path)[0].clone(),
    ]
    .iter()
    .filter_map(|name| read_uninstall_key(root, name, hive))
    .find(|value| path_matches(&value.install_location, path))
}

fn read_first_installation(root: &RegKey, hive: &'static str) -> Option<UninstallRegistryData> {
    if let Some(legacy) = read_uninstall_key(root, UNINSTALL_KEY, hive) {
        return Some(legacy);
    }
    let (parent, product) = UNINSTALL_KEY.rsplit_once('\\')?;
    let key = root.open_subkey(parent).ok()?;
    let mut names = key.enum_keys().collect::<std::io::Result<Vec<_>>>().ok()?;
    names.sort();
    names
        .into_iter()
        .filter(|name| name.starts_with(&format!("{product}-")))
        .find_map(|name| {
            let name = format!(r"{parent}\{name}");
            let data = read_uninstall_key(root, &name, hive)?;
            (installation_keys(Path::new(&data.install_location))[0] == name).then_some(data)
        })
}

/// Detect existing install like NSIS `ReadRegStr` on `UNINSTKEY` (HKCU then HKLM).
pub(super) fn read_existing_install_from_uninstall_registry() -> Option<UninstallRegistryData> {
    read_first_installation(&RegKey::predef(HKEY_CURRENT_USER), "hkcu")
        .or_else(|| read_first_installation(&RegKey::predef(HKEY_LOCAL_MACHINE), "hklm"))
}

/// Remove OpenBitFun context menu entries (no longer registered on install).
pub(super) fn remove_context_menu(install_path: &Path) -> Result<()> {
    let hkcu = maintenance_user_root()?;
    for path in CONTEXT_KEYS {
        match hkcu.open_subkey(format!("{path}\\command")) {
            Ok(key) => {
                let value = key.get_value::<String, _>("")?;
                if command_targets(&value, install_path) {
                    hkcu.delete_subkey_all(path)?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// Remove the install path from the user's PATH environment variable.
pub(super) fn remove_from_path(install_path: &Path) -> Result<()> {
    let hkcu = maintenance_user_root()?;
    let env_key = match hkcu.open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE) {
        Ok(key) => key,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };

    let raw = match env_key.get_raw_value("Path") {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let current_path: String = env_key.get_value("Path")?;
    let install_dir = install_path.to_string_lossy();

    let new_path: String = current_path
        .split(';')
        .filter(|p| !path_matches(p.trim(), Path::new(install_dir.as_ref())))
        .collect::<Vec<_>>()
        .join(";");

    if new_path != current_path {
        let bytes = new_path
            .encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        env_key.set_raw_value(
            "Path",
            &winreg::RegValue {
                bytes,
                vtype: raw.vtype,
            },
        )?;
    }
    Ok(())
}
