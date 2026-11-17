//! Native and legacy NSIS command-line compatibility at the installer boundary.
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Operation {
    Install,
    Update,
    Uninstall,
}

#[derive(Debug)]
pub(super) struct Request {
    pub operation: Operation,
    pub native: bool,
    pub quiet: bool,
    pub passive: bool,
    pub restart: bool,
    pub target: Option<PathBuf>,
    pub flags: Vec<String>,
    pub app_args: Vec<String>,
    pub legacy_uninstall: bool,
}

impl Request {
    pub(super) fn elevated_worker_args(&self, target: &Path, owner: &str) -> Vec<String> {
        let mut flags = self
            .flags
            .iter()
            .filter(|arg| !arg.eq_ignore_ascii_case("/R"))
            .cloned()
            .collect::<Vec<_>>();
        flags.push(format!("/OPENBITFUN_TARGET={}", target.display()));
        if self.value("/OWNER_SID=").is_none() {
            flags.push(format!("/OWNER_SID={owner}"));
        }
        flags
    }
    pub(super) fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|arg| arg.eq_ignore_ascii_case(flag))
    }
    pub(super) fn value(&self, prefix: &str) -> Option<&str> {
        self.flags.iter().find_map(|arg| {
            arg.get(..prefix.len())
                .filter(|head| head.eq_ignore_ascii_case(prefix))
                .map(|_| &arg[prefix.len()..])
        })
    }
    pub(super) fn non_interactive(&self) -> bool {
        self.quiet || self.passive || self.value("/READY_EVENT=").is_some()
    }
}

fn legacy_target(flags: &[String], prefix: &str, raw: Option<&str>) -> Option<PathBuf> {
    let argument = flags.iter().find(|arg| {
        arg.get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    })?;
    // NSIS /D= and _?= consume the unquoted final command-line tail, including
    // spaces. Structured native arguments continue to use the OS argv parser.
    if let Some(raw) = raw {
        let lowered = raw.to_ascii_lowercase();
        if let Some(index) = lowered
            .match_indices(&prefix.to_ascii_lowercase())
            .map(|(i, _)| i)
            .find(|&i| i == 0 || raw.as_bytes()[i - 1].is_ascii_whitespace())
        {
            let tail = raw[index + prefix.len()..].trim();
            if !tail.is_empty() {
                return Some(PathBuf::from(tail.trim_matches('"')));
            }
        }
    }
    Some(PathBuf::from(&argument[prefix.len()..]))
}

pub(super) fn parse(
    executable: &Path,
    args: &[String],
    raw: Option<&str>,
) -> Result<Request, String> {
    let boundary = args
        .iter()
        .position(|arg| arg.eq_ignore_ascii_case("/ARGS"))
        .unwrap_or(args.len());
    let flags = args[..boundary].to_vec();
    let has = |flag: &str| flags.iter().any(|arg| arg.eq_ignore_ascii_case(flag));
    let uninstall = executable
        .file_stem()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("uninstall"))
        || has("--uninstall")
        || has("/NATIVE_UNINSTALL");
    let quiet = has("--quiet") || has("/S");
    let passive = has("/P");
    let update = has("/UPDATE");
    let operation = if uninstall {
        Operation::Uninstall
    } else if update {
        Operation::Update
    } else {
        Operation::Install
    };
    let explicit = flags
        .iter()
        .position(|arg| arg == "--uninstall")
        .map(|index| {
            flags
                .get(index + 1)
                .filter(|arg| !arg.is_empty())
                .map(PathBuf::from)
                .ok_or("Uninstall requires an explicit directory")
        })
        .transpose()?;
    let native_target = flags
        .iter()
        .find_map(|arg| arg.strip_prefix("/OPENBITFUN_TARGET=").map(PathBuf::from));
    let target = explicit
        .or(native_target)
        .or_else(|| legacy_target(&flags, if uninstall { "_?=" } else { "/D=" }, raw));
    Ok(Request {
        operation,
        native: quiet || passive || update || has("/NATIVE_INSTALL") || has("/NATIVE_UNINSTALL"),
        quiet,
        passive,
        restart: !uninstall && has("/R"),
        target,
        app_args: args.get(boundary + 1..).unwrap_or_default().to_vec(),
        legacy_uninstall: uninstall
            && !has("--quiet")
            && !has("/NATIVE_UNINSTALL")
            && (quiet || passive || update || flags.iter().any(|arg| arg.starts_with("_?="))),
        flags,
    })
}

pub(super) fn current() -> Result<Request, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    #[cfg(windows)]
    let raw = {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCommandLineW() -> *const u16;
        }
        let pointer = unsafe { GetCommandLineW() };
        let mut length = 0;
        unsafe {
            while *pointer.add(length) != 0 {
                length += 1;
            }
        }
        Some(String::from_utf16_lossy(unsafe {
            std::slice::from_raw_parts(pointer, length)
        }))
    };
    #[cfg(not(windows))]
    let raw: Option<String> = None;
    parse(&executable, &args, raw.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn elevation_retains_handoff_and_owner_but_leaves_restart_with_the_original_token() {
        let parsed = request(
            "installer.exe",
            &[
                "/UPDATE",
                "/R",
                "/WAITPID=42",
                "/READY_EVENT=event",
                "/SUPERVISOR_PID=24",
                "/RESULT_ID=abc",
                "/ARGS",
                "workspace with spaces",
            ],
            None,
        );
        let flags =
            parsed.elevated_worker_args(Path::new(r"C:\Program Files\OpenBitFun"), "S-1-5-21-1");
        let worker = parse(Path::new("installer.exe"), &flags, None).unwrap();
        assert_eq!(worker.operation, Operation::Update);
        assert_eq!(worker.value("/WAITPID="), Some("42"));
        assert_eq!(worker.value("/READY_EVENT="), Some("event"));
        assert_eq!(worker.value("/SUPERVISOR_PID="), Some("24"));
        assert_eq!(worker.value("/RESULT_ID="), Some("abc"));
        assert_eq!(worker.value("/OWNER_SID="), Some("S-1-5-21-1"));
        assert!(!worker.restart && worker.app_args.is_empty());
        assert!(parsed.restart);
        assert_eq!(parsed.app_args, ["workspace with spaces"]);
    }
    fn request(exe: &str, args: &[&str], raw: Option<&str>) -> Request {
        parse(
            Path::new(exe),
            &args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>(),
            raw,
        )
        .unwrap()
    }
    #[test]
    fn legacy_update_uninstaller_never_becomes_an_installer() {
        for args in [
            vec!["/UPDATE", "/P", "_?=C:\\Apps\\OpenBitFun"],
            vec!["--uninstall", "C:\\Apps\\OpenBitFun", "/UPDATE", "/P"],
        ] {
            let parsed = request("uninstall.exe", &args, None);
            assert_eq!(parsed.operation, Operation::Uninstall);
            assert!(parsed.native && parsed.passive && parsed.legacy_uninstall);
            assert!(!parsed.restart);
        }
    }
    #[test]
    fn legacy_directory_tails_and_restart_arguments_are_preserved() {
        let parsed = request(
            "installer.exe",
            &["/UPDATE", "/D=C:\\Program", "Files\\OpenBitFun"],
            Some(r"installer.exe /UPDATE /D=C:\Program Files\OpenBitFun"),
        );
        assert_eq!(
            parsed.target.unwrap(),
            Path::new(r"C:\Program Files\OpenBitFun")
        );
        let parsed = request(
            "uninstall.exe",
            &["/S", "_?=C:\\Program", "Files\\OpenBitFun"],
            Some(r"uninstall.exe /S _?=C:\Program Files\OpenBitFun"),
        );
        assert!(parsed.native && parsed.quiet && parsed.legacy_uninstall);
        assert_eq!(
            parsed.target.unwrap(),
            Path::new(r"C:\Program Files\OpenBitFun")
        );
        let parsed = request(
            "installer.exe",
            &[
                "/UPDATE",
                "/R",
                "/ARGS",
                "workspace with spaces",
                "--flag",
                "/UPDATE",
                "/OPENBITFUN_TARGET=not-a-target",
                "--maintenance-protocol",
                "/OWNER_SID=not-an-owner",
            ],
            None,
        );
        assert!(parsed.restart);
        assert!(parsed.target.is_none());
        assert!(!parsed.has("--maintenance-protocol"));
        assert!(parsed.value("/OWNER_SID=").is_none());
        assert_eq!(
            parsed.app_args,
            [
                "workspace with spaces",
                "--flag",
                "/UPDATE",
                "/OPENBITFUN_TARGET=not-a-target",
                "--maintenance-protocol",
                "/OWNER_SID=not-an-owner"
            ]
        );
    }
    #[test]
    fn native_and_interactive_uninstall_contracts_are_retained() {
        assert!(!request("uninstall.exe", &[], None).native);
        assert!(!request("maintenance.exe", &["--uninstall", "C:\\Apps"], None).native);
        let parsed = request(
            "maintenance.exe",
            &["--uninstall", "C:\\Apps", "--quiet"],
            None,
        );
        assert!(parsed.native && parsed.quiet && !parsed.legacy_uninstall);
        let parsed = request(
            "installer.exe",
            &[
                "/UPDATE",
                "/READY_EVENT=event",
                "/OPENBITFUN_TARGET=C:\\Apps",
            ],
            None,
        );
        assert_eq!(parsed.operation, Operation::Update);
        assert_eq!(parsed.target.unwrap(), Path::new(r"C:\Apps"));
    }
}
