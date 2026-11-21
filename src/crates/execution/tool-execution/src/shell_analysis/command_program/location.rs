//! Non-executing directory flow, including possible failed changes and stack scope.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Location {
    pub cwd: Vec<String>,
    stack: Option<Vec<Vec<String>>>,
}
impl Location {
    pub(super) fn new(cwd: &str) -> Self {
        Self {
            cwd: vec![cwd.into()],
            stack: Some(vec![]),
        }
    }
    pub(super) fn union(&self, other: &Self, result: &mut CommandProgram) -> Self {
        let mut cwd = self.cwd.clone();
        for path in &other.cwd {
            if !cwd.contains(path) {
                cwd.push(path.clone());
            }
        }
        if cwd.len() > 8 {
            result.complete = false;
            cwd.truncate(8);
        }
        Self {
            cwd,
            stack: if self.stack == other.stack {
                self.stack.clone()
            } else {
                None
            },
        }
    }
    pub(super) fn paths(&self, target: &str, powershell: bool) -> Option<Vec<String>> {
        let mut paths = vec![];
        for cwd in &self.cwd {
            let path = absolute(cwd, target, powershell)?;
            if path.len() > 32 * 1024 || path.split('/').count() > 256 {
                return None;
            }
            // Check both physical and logical parent traversal. IO confirms symlinks later.
            for candidate in [path.clone(), lexical(&path)] {
                if !paths.contains(&candidate) {
                    paths.push(candidate);
                }
            }
        }
        (paths.len() <= 8).then_some(paths)
    }
}
#[derive(Clone)]
pub(super) struct LocationFlow {
    pub success: Location,
    pub failure: Location,
}
impl LocationFlow {
    pub(super) fn same(location: &Location) -> Self {
        Self {
            success: location.clone(),
            failure: location.clone(),
        }
    }
    pub(super) fn after(&self, result: &mut CommandProgram) -> Location {
        self.success.union(&self.failure, result)
    }
}
pub(super) fn is_location_command(name: &str, shell: &str) -> bool {
    if matches!(shell, "pwsh" | "powershell") {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "set-location"
                | "sl"
                | "cd"
                | "chdir"
                | "push-location"
                | "pushd"
                | "pop-location"
                | "popd"
        )
    } else {
        matches!(name, "cd" | "pushd" | "popd") || (shell == "zsh" && name == "chdir")
    }
}
pub(super) fn change(
    argv: &[String],
    shell: &str,
    location: &Location,
    result: &mut CommandProgram,
) -> Option<LocationFlow> {
    let name = argv.first()?;
    if !is_location_command(name, shell) {
        return None;
    }
    let powershell = matches!(shell, "pwsh" | "powershell");
    let name = if powershell {
        name.to_ascii_lowercase()
    } else {
        name.clone()
    };
    let push = matches!(name.as_str(), "pushd" | "push-location");
    let pop = matches!(name.as_str(), "popd" | "pop-location");
    let mut args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    if powershell {
        args.retain(|arg| !arg.eq_ignore_ascii_case("-passthru"));
        if args.first().is_some_and(|arg| {
            arg.eq_ignore_ascii_case("-path") || arg.eq_ignore_ascii_case("-literalpath")
        }) {
            args.remove(0);
        }
    } else {
        if args.first().is_some_and(|arg| matches!(*arg, "-L" | "-P")) {
            args.remove(0);
        }
        if args.first() == Some(&"--") {
            args.remove(0);
        }
    }
    let mut success = location.clone();
    let target = if pop && args.is_empty() {
        match success.stack.as_mut().and_then(Vec::pop) {
            Some(paths) => Some(paths),
            None => None,
        }
    } else if !pop && args.len() == 1 && !args[0].is_empty() && !args[0].starts_with(['-', '+']) {
        if !powershell && !args[0].starts_with(['/', '.']) {
            result.requires_default_cdpath = true;
        }
        if push {
            match success.stack.as_mut() {
                Some(stack) if stack.len() < MAX_DEPTH => stack.push(location.cwd.clone()),
                _ => result.complete = false,
            }
        }
        location.paths(args[0], powershell)
    } else {
        None
    };
    match target {
        Some(paths) => {
            if result.accesses.len() + paths.len() > MAX_FACTS
                || result
                    .accesses
                    .iter()
                    .map(|(_, path)| path.len())
                    .sum::<usize>()
                    + paths.iter().map(String::len).sum::<usize>()
                    > MAX_COMMAND_BYTES
            {
                result.complete = false;
            } else {
                result
                    .accesses
                    .extend(paths.iter().cloned().map(|path| ("read".into(), path)));
            }
            success.cwd = paths;
        }
        None => result.complete = false,
    }
    Some(LocationFlow {
        success,
        failure: location.clone(),
    })
}
fn absolute(cwd: &str, target: &str, powershell: bool) -> Option<String> {
    if !powershell {
        return Some(if target.starts_with('/') {
            target.into()
        } else {
            join(cwd, target)
        });
    }
    let cwd = cwd.replace('\\', "/");
    let target = target.replace('\\', "/");
    let drive = |path: &str| {
        path.as_bytes().get(1) == Some(&b':') && path.as_bytes()[0].is_ascii_alphabetic()
    };
    if target.contains(':') && !(drive(&target) && target.as_bytes().get(2) == Some(&b'/')) {
        return None;
    }
    if drive(&target) || target.starts_with("//") {
        return Some(target);
    }
    if target.starts_with('/') {
        if drive(&cwd) {
            return Some(format!("{}{target}", &cwd[..2]));
        }
        if cwd.starts_with("//") {
            return None;
        } // UNC root depends on its share.
        return Some(target);
    }
    Some(join(&cwd, &target))
}
fn lexical(path: &str) -> String {
    let mut parts = vec![];
    let floor = if path.starts_with("//") {
        2
    } else if path.as_bytes().get(1) == Some(&b':') {
        1
    } else {
        0
    };
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.len() > floor {
                    parts.pop();
                }
            }
            _ => parts.push(part),
        }
    }
    let prefix = if path.starts_with("//") {
        "//"
    } else if path.starts_with('/') {
        "/"
    } else {
        ""
    };
    let mut result = format!("{prefix}{}", parts.join("/"));
    if result.ends_with(':') {
        result.push('/');
    }
    result
}
