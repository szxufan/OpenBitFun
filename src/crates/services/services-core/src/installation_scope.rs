//! Read-only scope facts shared by Windows maintenance callers. No elevation,
//! publication, registration mutation, or product capability selection lives here.
use std::{ffi::c_void, path::Path};
type Hkey = *mut c_void;
const HKCU: Hkey = -2147483647_isize as Hkey;
const HKLM: Hkey = -2147483646_isize as Hkey;
const HKU: Hkey = -2147483645_isize as Hkey;
const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";

#[link(name = "advapi32")]
extern "system" {
    fn RegOpenKeyExW(
        root: Hkey,
        name: *const u16,
        options: u32,
        access: u32,
        key: *mut Hkey,
    ) -> i32;
    fn RegCloseKey(key: Hkey) -> i32;
    fn RegEnumKeyExW(
        key: Hkey,
        index: u32,
        name: *mut u16,
        count: *mut u32,
        reserved: *mut u32,
        class: *mut u16,
        class_count: *mut u32,
        time: *mut c_void,
    ) -> i32;
    fn RegQueryValueExW(
        key: Hkey,
        name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        length: *mut u32,
    ) -> i32;
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
struct Key(Hkey);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
impl Key {
    fn open(root: Hkey, name: &str) -> Result<Option<Self>, String> {
        let mut key = std::ptr::null_mut();
        let code = unsafe { RegOpenKeyExW(root, wide(name).as_ptr(), 0, 0x20019, &mut key) };
        match code {
            0 => Ok(Some(Self(key))),
            2 => Ok(None),
            _ => Err(format!(
                "Cannot read installation registration {name} (Windows error {code})"
            )),
        }
    }
    fn location(&self) -> Result<Option<String>, String> {
        let name = wide("InstallLocation");
        let mut kind = 0;
        let mut size = 0;
        let code = unsafe {
            RegQueryValueExW(
                self.0,
                name.as_ptr(),
                std::ptr::null_mut(),
                &mut kind,
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if code == 2 {
            return Ok(None);
        }
        if code != 0 || !matches!(kind, 1 | 2) || size % 2 != 0 {
            return Err(
                "Installation registration has an unreadable location; registry data was preserved"
                    .into(),
            );
        }
        let mut value = vec![0u16; size as usize / 2];
        let code = unsafe {
            RegQueryValueExW(
                self.0,
                name.as_ptr(),
                std::ptr::null_mut(),
                &mut kind,
                value.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if code != 0 {
            return Err(format!(
                "Cannot read installation location (Windows error {code})"
            ));
        }
        Ok(Some(
            String::from_utf16_lossy(&value)
                .trim_end_matches('\0')
                .trim()
                .trim_matches('"')
                .to_string(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallationScope {
    pub current_user_registration: bool,
    pub machine_registration: bool,
    pub machine_install: bool,
}
impl InstallationScope {
    pub fn requires_machine_access(self) -> bool {
        self.machine_registration || self.machine_install
    }
}

fn normalized(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('/', "\\")
        .trim_start_matches(r"\\?\")
        .trim_end_matches('\\')
        .to_lowercase()
}
fn registered(root: Hkey, product: &str, directory: &Path) -> Result<bool, String> {
    let Some(parent) = Key::open(root, UNINSTALL)? else {
        return Ok(false);
    };
    for index in 0.. {
        let mut name = [0u16; 512];
        let mut count = name.len() as u32;
        let code = unsafe {
            RegEnumKeyExW(
                parent.0,
                index,
                name.as_mut_ptr(),
                &mut count,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if code == 259 {
            return Ok(false);
        }
        if code != 0 {
            return Err(format!(
                "Cannot enumerate installation registrations (Windows error {code})"
            ));
        }
        let name = String::from_utf16_lossy(&name[..count as usize]);
        let lower = name.to_ascii_lowercase();
        if !name.eq_ignore_ascii_case(product)
            && !lower.starts_with(&format!("{}-", product.to_ascii_lowercase()))
        {
            continue;
        }
        if let Some(key) = Key::open(parent.0, &name)? {
            if key
                .location()?
                .is_some_and(|location| normalized(Path::new(&location)) == normalized(directory))
            {
                return Ok(true);
            }
        }
    }
    unreachable!()
}
fn metadata_hint(directory: &Path) -> Result<Option<bool>, String> {
    for (file, pointer) in [
        (".openbitfun-installed.json", "/allUsers"),
        (
            ".openbitfun-maintenance/journal.json",
            "/integrations/registry/machine",
        ),
    ] {
        let bytes = match std::fs::read(directory.join(file)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "Cannot read installation scope; metadata was preserved: {error}"
                ))
            }
        };
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
            format!("Installation scope metadata is unreadable; retained for repair: {error}")
        })?;
        if let Some(schema) = value.get("schema") {
            if schema.as_u64() != Some(1) {
                return Err("Unsupported installation scope metadata; retained for repair".into());
            }
        }
        if let Some(value) = value.pointer(pointer) {
            return value
                .as_bool()
                .map(Some)
                .ok_or_else(|| "Invalid installation scope; metadata was preserved".into());
        }
    }
    Ok(None)
}
fn resolve(
    directory: &Path,
    product: &str,
    user: Hkey,
    machine: Hkey,
) -> Result<InstallationScope, String> {
    let current_user_registration = registered(user, product, directory)?;
    let machine_registration = registered(machine, product, directory)?;
    Ok(InstallationScope {
        current_user_registration,
        machine_registration,
        machine_install: metadata_hint(directory)?
            .unwrap_or(machine_registration && !current_user_registration),
    })
}

/// `owner_sid` keeps an elevated worker bound to the submitting user's hive.
pub fn windows_installation_scope(
    directory: &Path,
    product: &str,
    owner_sid: Option<&str>,
) -> Result<InstallationScope, String> {
    let user = if let Some(sid) = owner_sid {
        if !sid.starts_with("S-1-")
            || !sid
                .bytes()
                .all(|c| c.is_ascii_digit() || matches!(c, b'S' | b'-'))
        {
            return Err("Invalid installation owner SID".into());
        }
        Some(Key::open(HKU, sid)?.ok_or("Installation owner's registry hive is unavailable")?)
    } else {
        None
    };
    resolve(
        directory,
        product,
        user.as_ref().map_or(HKCU, |key| key.0),
        HKLM,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[link(name = "advapi32")]
    extern "system" {
        fn RegCreateKeyExW(
            root: Hkey,
            name: *const u16,
            reserved: u32,
            class: *mut u16,
            options: u32,
            access: u32,
            security: *const c_void,
            result: *mut Hkey,
            disposition: *mut u32,
        ) -> i32;
        fn RegSetValueExW(
            key: Hkey,
            name: *const u16,
            reserved: u32,
            kind: u32,
            bytes: *const u8,
            count: u32,
        ) -> i32;
        fn RegDeleteTreeW(root: Hkey, name: *const u16) -> i32;
    }
    fn create(root: Hkey, name: &str) -> Key {
        let mut key = std::ptr::null_mut();
        assert_eq!(
            unsafe {
                RegCreateKeyExW(
                    root,
                    wide(name).as_ptr(),
                    0,
                    std::ptr::null_mut(),
                    0,
                    0xf003f,
                    std::ptr::null(),
                    &mut key,
                    std::ptr::null_mut(),
                )
            },
            0
        );
        Key(key)
    }
    fn register(root: Hkey, name: &str, directory: &Path) {
        let key = create(root, &format!(r"{UNINSTALL}\{name}"));
        let value = wide(&format!("\"{}\"", directory.display()));
        assert_eq!(
            unsafe {
                RegSetValueExW(
                    key.0,
                    wide("InstallLocation").as_ptr(),
                    0,
                    1,
                    value.as_ptr().cast(),
                    (value.len() * 2) as u32,
                )
            },
            0
        );
    }
    #[test]
    fn legacy_machine_registration_dual_registration_and_inventory_share_one_rule() {
        let name = format!(
            r"Software\OpenBitFunScopeTests\{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let fixture = create(HKCU, &name);
        let user = create(fixture.0, "User");
        let machine = create(fixture.0, "Machine");
        let temp = tempfile::tempdir().unwrap();
        register(machine.0, "OpenBitFun", temp.path());
        let scope = resolve(temp.path(), "OpenBitFun", user.0, machine.0).unwrap();
        assert!(scope.machine_install && scope.requires_machine_access());
        register(user.0, "OpenBitFun-directory-id", temp.path());
        let scope = resolve(temp.path(), "OpenBitFun", user.0, machine.0).unwrap();
        assert!(
            scope.current_user_registration && scope.machine_registration && !scope.machine_install
        );
        assert!(scope.requires_machine_access());
        std::fs::write(
            temp.path().join(".openbitfun-installed.json"),
            br#"{"schema":1,"allUsers":true}"#,
        )
        .unwrap();
        assert!(
            resolve(temp.path(), "OpenBitFun", user.0, machine.0)
                .unwrap()
                .machine_install
        );
        std::fs::write(
            temp.path().join(".openbitfun-installed.json"),
            b"unreadable",
        )
        .unwrap();
        assert!(resolve(temp.path(), "OpenBitFun", user.0, machine.0).is_err());
        assert_eq!(
            std::fs::read(temp.path().join(".openbitfun-installed.json")).unwrap(),
            b"unreadable"
        );
        drop(machine);
        drop(user);
        drop(fixture);
        assert_eq!(unsafe { RegDeleteTreeW(HKCU, wide(&name).as_ptr()) }, 0);
    }
}
