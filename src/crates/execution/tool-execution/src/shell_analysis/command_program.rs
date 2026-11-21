//! Conservative AST projection for permission matching, separate from constraint facts.
use super::*;
use openbitfun_agent_tools::command_permission::{CommandPermissionPlan, PermissionCommand};
use serde::Deserialize;
mod location;
mod read_commands;
use location::{Location, LocationFlow};
// Optional argv index identifies a whole file operand, separating it from
// pattern text, option values and implicit directory reads during glob handling.
type CommandReads = Vec<(Option<usize>, String)>;

#[derive(Debug, Clone, Default)]
pub struct CommandProgram {
    pub commands: Vec<PermissionCommand>,
    pub accesses: Vec<(String, String)>,
    pub glob_accesses: Vec<String>,
    pub complete: bool,
    pub reason: Option<String>,
    pub requires_default_cdpath: bool,
    pub requires_null_device: bool,
}
impl CommandProgram {
    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self {
            reason: Some(reason.into()),
            ..Self::default()
        }
    }
    pub fn plan(&self, kind: &str, path: &str, cwd: &str) -> CommandPermissionPlan {
        CommandPermissionPlan {
            shell_kind: kind.into(),
            shell_path: path.into(),
            workdir: cwd.into(),
            complete: self.complete && self.glob_accesses.is_empty(),
            commands: self.commands.clone(),
        }
    }
}

pub fn parse_permission_program(source: &str, shell: &str, cwd: &str) -> CommandProgram {
    if !matches!(shell, "bash" | "sh" | "zsh")
        || source.len() > MAX_COMMAND_BYTES
        || !cwd.starts_with('/')
    {
        return CommandProgram::unsupported(
            "shell dialect or working directory is unsupported for automatic command approval",
        );
    }
    let tokens = match lexer::lex(source) {
        Ok(tokens) => tokens,
        Err(_) => return CommandProgram::unsupported("invalid shell syntax"),
    };
    if tokens
        .iter()
        .any(|token| matches!(token, Token::Op(op, _) if op == "|&"))
    {
        return CommandProgram::unsupported("version-dependent pipe syntax requires confirmation");
    }
    let mut parser = Parser { tokens, pos: 0 };
    let ast = match parser.sequence(0) {
        Ok(ast) if parser.pos == parser.tokens.len() => ast,
        _ => return CommandProgram::unsupported("unsupported shell syntax"),
    };
    let mut result = CommandProgram {
        complete: true,
        ..Default::default()
    };
    collect(&ast, shell, &Location::new(cwd), &mut result);
    if result.commands.is_empty() {
        result.complete = false;
    }
    if !result.complete {
        result.reason = Some(
            "dynamic expansion, shell state, or unsupported command syntax requires confirmation"
                .into(),
        );
    }
    result
}
fn collect(
    ast: &Ast,
    shell: &str,
    location: &Location,
    result: &mut CommandProgram,
) -> LocationFlow {
    match ast {
        Ast::Sequence(list) => {
            let mut flow = LocationFlow::same(location);
            for node in list {
                let current = flow.after(result);
                flow = collect(node, shell, &current, result);
            }
            flow
        }
        Ast::Pipe(list) => {
            let mut after = location.clone();
            for node in list {
                let input = if matches!(shell, "pwsh" | "powershell") {
                    &after
                } else {
                    location
                };
                let flow = collect(node, shell, input, result);
                after = after.union(&flow.after(result), result);
            }
            // Some shell configurations retain the final pipeline's directory state.
            LocationFlow::same(&after)
        }
        Ast::And(a, b) => {
            let first = collect(a, shell, location, result);
            let second = collect(b, shell, &first.success, result);
            LocationFlow {
                success: second.success,
                failure: first.failure.union(&second.failure, result),
            }
        }
        Ast::Or(a, b) => {
            let first = collect(a, shell, location, result);
            let second = collect(b, shell, &first.failure, result);
            LocationFlow {
                success: first.success.union(&second.success, result),
                failure: second.failure,
            }
        }
        Ast::Group(node, redirects) => {
            collect(node, shell, location, result);
            if !redirects.is_empty() {
                result.complete = false;
            }
            LocationFlow::same(location)
        }
        Ast::Command(words, redirects) => {
            if words.is_empty()
                || words
                    .iter()
                    .any(|w| (w.dynamic && !simple_glob_word(w)) || w.executes || w.assignment)
            {
                result.complete = false;
                return LocationFlow::same(location);
            }
            if shell == "zsh"
                && words.iter().any(|word| {
                    !word.quoted
                        && (word.value.starts_with('=') || word.value.contains(['^', '#', '~']))
                })
            {
                result.complete = false;
            }
            let argv: Vec<String> = words.iter().map(|w| w.value.clone()).collect();
            let powershell = matches!(shell, "pwsh" | "powershell");
            let directory = location::change(&argv, shell, location, result);
            let normalized: Vec<String> = argv
                .iter()
                .enumerate()
                .map(|(i, arg)| {
                    if powershell && i == 0 {
                        arg.to_ascii_lowercase()
                    } else {
                        arg.clone()
                    }
                })
                .collect();
            if directory.is_none()
                && (dangerous_shell_program(&normalized)
                    || (powershell && powershell_changes_shell_state(&argv)))
            {
                result.complete = false;
            }
            let (mut read_only, reads) = if directory.is_some() {
                (true, vec![])
            } else {
                command_effects(&argv, powershell)
            };
            let glob_indices: Vec<_> = words
                .iter()
                .enumerate()
                .filter_map(|(index, word)| word.dynamic.then_some(index))
                .collect();
            if glob_indices.iter().any(|index| {
                !read_only || !reads.iter().any(|(operand, _)| *operand == Some(*index))
            }) {
                result.complete = false;
            }
            if argv[0] == "printf" && !read_only {
                result.complete = false;
            }
            for (index, path) in reads {
                if index.is_some_and(|index| glob_indices.contains(&index)) {
                    match location.paths(&path, powershell) {
                        Some(paths) if result.glob_accesses.len() + paths.len() <= 64 => {
                            result.glob_accesses.extend(paths)
                        }
                        _ => result.complete = false,
                    }
                } else {
                    record_access(result, location, "read", &path, powershell);
                }
            }
            for redirect in redirects {
                if redirect.word.dynamic
                    || redirect.word.executes
                    || redirect
                        .body
                        .as_ref()
                        .is_some_and(|body| body.executes || body.dynamic)
                {
                    result.complete = false;
                    continue;
                }
                match redirect.op.as_str() {
                    ">" | ">>" | ">|" | "<>" | "&>" | "&>>" => {
                        if redirect.op == "&>>" || (shell == "sh" && redirect.op.starts_with('&')) {
                            result.complete = false;
                        }
                        if !powershell && redirect.op != "<>" && redirect.word.value == "/dev/null"
                        {
                            result.requires_null_device = true;
                        } else {
                            read_only = false;
                            record_access(
                                result,
                                location,
                                "edit",
                                &redirect.word.value,
                                powershell,
                            );
                        }
                    }
                    "<" => {
                        record_access(result, location, "read", &redirect.word.value, powershell)
                    }
                    "<<" | "<<-" | "<<<" => {}
                    ">&" | "<&"
                        if matches!(redirect.word.value.as_str(), "-" | "0" | "1" | "2") => {}
                    _ => result.complete = false,
                }
            }
            result.commands.push(PermissionCommand { argv, read_only });
            directory.unwrap_or_else(|| LocationFlow::same(location))
        }
    }
}
fn record_access(
    result: &mut CommandProgram,
    location: &Location,
    action: &str,
    path: &str,
    powershell: bool,
) {
    match location.paths(path, powershell) {
        Some(paths)
            if result.accesses.len() + paths.len() <= MAX_FACTS
                && result
                    .accesses
                    .iter()
                    .map(|(_, path)| path.len())
                    .sum::<usize>()
                    + paths.iter().map(String::len).sum::<usize>()
                    <= MAX_COMMAND_BYTES =>
        {
            result
                .accesses
                .extend(paths.into_iter().map(|path| (action.into(), path)))
        }
        _ => result.complete = false,
    }
}

fn simple_glob_word(word: &Word) -> bool {
    !word.quoted && !word.executes && PermissionGlobMatcher::new(&word.value).is_some()
}

/// A bounded final-component glob. No shell is invoked to expand it.
pub struct PermissionGlobMatcher(regex::Regex);
impl PermissionGlobMatcher {
    pub fn new(pattern: &str) -> Option<Self> {
        if pattern.len() > 32 * 1024
            || pattern.contains("**")
            || pattern.contains([
                '[', ']', '{', '}', '$', '`', '~', '\\', '\0', '\n', '\r', '\'', '"',
            ])
        {
            return None;
        }
        let (parent, name) = pattern.rsplit_once('/').unwrap_or(("", pattern));
        if parent.contains(['*', '?']) || !name.contains(['*', '?']) {
            return None;
        }
        let expression = regex::escape(name).replace("\\*", ".*").replace("\\?", ".");
        regex::Regex::new(&format!("(?s)^{expression}$"))
            .ok()
            .map(Self)
    }
    pub fn matches(&self, name: &str) -> bool {
        self.0.is_match(name)
    }
}
fn dangerous_shell_program(argv: &[String]) -> bool {
    let Some(name) = argv.first() else {
        return true;
    };
    (name == "git" && argv.get(1).is_some_and(|arg| arg.starts_with('-')))
        || matches!(
            name.as_str(),
            "cd" | "pushd"
                | "popd"
                | "source"
                | "alias"
                | "unalias"
                | "autoload"
                | "set"
                | "setopt"
                | "unsetopt"
                | "export"
                | "unset"
                | "typeset"
                | "declare"
                | "readonly"
                | "local"
                | "hash"
                | "enable"
                | "builtin"
                | "trap"
                | "."
                | "eval"
                | "exec"
                | "env"
                | "command"
                | "nohup"
                | "timeout"
                | "sudo"
                | "bash"
                | "sh"
                | "zsh"
                | "fish"
                | "pwsh"
                | "powershell"
                | "cmd"
                | "if"
                | "then"
                | "else"
                | "fi"
                | "for"
                | "while"
                | "until"
                | "do"
                | "done"
                | "case"
                | "esac"
                | "function"
                | "{"
                | "}"
                | "!"
                | "[["
                | "(("
        )
        || name.contains('/')
        || name.contains('\\')
        || (name == "git"
            && argv.iter().any(|a| {
                a == "-c"
                    || a.starts_with("--config-env")
                    || a.starts_with("--exec-path")
                    || a.starts_with("--upload-pack")
                    || a.starts_with("--receive-pack")
            }))
}

/// Known command effects, never inferred from an empty constraint-operation list.
fn command_effects(argv: &[String], powershell: bool) -> (bool, CommandReads) {
    let Some(name) = argv.first() else {
        return (false, vec![]);
    };
    let args = &argv[1..];
    if powershell {
        let lower = name.to_ascii_lowercase();
        return match lower.as_str() {
            "get-location" if args.is_empty() => (true, vec![]),
            "write-output" if args.iter().all(|a| !a.starts_with('-')) => (true, vec![]),
            "get-content" | "get-childitem" | "get-item" | "test-path"
                if args.iter().all(|a| !a.starts_with('-')) =>
            {
                (
                    true,
                    args.iter()
                        .enumerate()
                        .map(|(index, path)| (Some(index + 1), path.clone()))
                        .collect(),
                )
            }
            "git" => command_effects(argv, false),
            _ => (false, vec![]),
        };
    }
    match name.as_str() {
        "true" | "false" if args.is_empty() => (true, vec![]),
        "pwd" if args.iter().all(|a| matches!(a.as_str(), "-L" | "-P")) => (true, vec![]),
        "echo" => (true, vec![]),
        "printf" => (read_commands::printf(args), vec![]),
        "find" => read_commands::find(args),
        "head" | "tail" => read_commands::head_tail(args),
        "grep" => read_commands::grep(args),
        "sort" => read_commands::sort(args),
        "cat" | "ls" | "wc" => {
            let mut paths = vec![];
            let mut literal = false;
            for (index, arg) in args.iter().enumerate() {
                if !literal && arg == "--" {
                    literal = true;
                    continue;
                }
                if !literal && arg.starts_with('-') {
                    let supported = match name.as_str() {
                        "cat" => {
                            matches!(arg.as_str(), "-n" | "-b" | "-s" | "-v" | "-A" | "-E" | "-T")
                        }
                        "ls" => matches!(
                            arg.as_str(),
                            "-l" | "-a"
                                | "-la"
                                | "-al"
                                | "-lh"
                                | "-lah"
                                | "-alh"
                                | "-h"
                                | "-R"
                                | "-d"
                                | "-1"
                        ),
                        "wc" => matches!(arg.as_str(), "-l" | "-w" | "-c" | "-m"),
                        _ => false,
                    };
                    if !supported {
                        return (false, vec![]);
                    }
                } else if arg != "-" {
                    paths.push((Some(index + 1), arg.clone()));
                }
            }
            if paths.is_empty() && name == "ls" {
                paths.push((None, ".".into()));
            }
            (true, paths)
        }
        "git" => {
            let Some(command) = args.first() else {
                return (false, vec![]);
            };
            let allowed = match command.as_str() {
                "log" => read_commands::git_log(&args[1..]),
                "status" => args[1..].iter().all(|a| {
                    matches!(
                        a.as_str(),
                        "--short"
                            | "-s"
                            | "--branch"
                            | "-b"
                            | "--porcelain"
                            | "--porcelain=v1"
                            | "--porcelain=v2"
                            | "--untracked-files=no"
                            | "--untracked-files=normal"
                            | "--untracked-files=all"
                    )
                }),
                "diff" => {
                    args.contains(&"--no-ext-diff".into())
                        && args.contains(&"--no-textconv".into())
                        && args[1..].iter().all(|a| {
                            matches!(
                                a.as_str(),
                                "--no-ext-diff"
                                    | "--no-textconv"
                                    | "--stat"
                                    | "--name-only"
                                    | "--name-status"
                                    | "--cached"
                                    | "--staged"
                                    | "--numstat"
                                    | "--shortstat"
                            )
                        })
                }
                "rev-parse" => args[1..].iter().all(|a| {
                    matches!(
                        a.as_str(),
                        "HEAD" | "--show-toplevel" | "--git-dir" | "--is-inside-work-tree"
                    )
                }),
                "branch" => args.len() == 1,
                _ => false,
            };
            (allowed, vec![])
        }
        _ => (false, vec![]),
    }
}

fn powershell_changes_shell_state(argv: &[String]) -> bool {
    let Some(name) = argv.first() else {
        return true;
    };
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        "set-location"
            | "push-location"
            | "pop-location"
            | "cd"
            | "chdir"
            | "sl"
            | "pushd"
            | "popd"
            | "set-alias"
            | "new-alias"
            | "sal"
            | "nal"
            | "import-module"
            | "remove-module"
            | "ipmo"
            | "rmo"
            | "invoke-expression"
            | "iex"
            | "set-variable"
            | "new-variable"
            | "remove-variable"
            | "clear-variable"
            | "sv"
            | "nv"
            | "rv"
            | "clv"
    ) || argv.iter().skip(1).any(|arg| {
        let lower = arg.to_ascii_lowercase();
        ["env:", "function:", "alias:", "variable:"]
            .iter()
            .any(|prefix| lower.starts_with(prefix))
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PowerShellFacts {
    complete: bool,
    commands: Vec<Vec<String>>,
    accesses: Vec<(String, String)>,
    #[serde(default)]
    redirects: Vec<Vec<String>>,
    #[serde(default)]
    tree: Option<NativeNode>,
}
#[derive(Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum NativeNode {
    Sequence(Vec<NativeNode>),
    Pipe(Vec<NativeNode>),
    And(Vec<NativeNode>),
    Or(Vec<NativeNode>),
    Command(usize),
}
fn native_ast(
    node: NativeNode,
    commands: &mut [Option<Ast>],
    depth: usize,
    count: &mut usize,
) -> Option<Ast> {
    *count += 1;
    if depth > MAX_DEPTH || *count > MAX_NODES {
        return None;
    }
    Some(match node {
        NativeNode::Command(index) => commands.get_mut(index)?.take()?,
        NativeNode::Sequence(nodes) | NativeNode::Pipe(nodes) if nodes.len() == 1 => {
            native_ast(nodes.into_iter().next()?, commands, depth + 1, count)?
        }
        NativeNode::Sequence(nodes) => Ast::Sequence(
            nodes
                .into_iter()
                .map(|n| native_ast(n, commands, depth + 1, count))
                .collect::<Option<_>>()?,
        ),
        NativeNode::Pipe(nodes) => Ast::Pipe(
            nodes
                .into_iter()
                .map(|n| native_ast(n, commands, depth + 1, count))
                .collect::<Option<_>>()?,
        ),
        NativeNode::And(mut nodes) if nodes.len() == 2 => {
            let b = native_ast(nodes.pop()?, commands, depth + 1, count)?;
            let a = native_ast(nodes.pop()?, commands, depth + 1, count)?;
            Ast::And(Box::new(a), Box::new(b))
        }
        NativeNode::Or(mut nodes) if nodes.len() == 2 => {
            let b = native_ast(nodes.pop()?, commands, depth + 1, count)?;
            let a = native_ast(nodes.pop()?, commands, depth + 1, count)?;
            Ast::Or(Box::new(a), Box::new(b))
        }
        _ => return None,
    })
}
fn native_word(value: String) -> Word {
    Word {
        dynamic: value.contains(['*', '?', '[', ']']),
        value,
        executes: false,
        quoted: true,
        assignment: false,
        span: Span { start: 0, end: 0 },
    }
}
/// Converts only the bounded native parser projection, never executable source.
pub fn powershell_permission_program(json: &str, cwd: &str) -> CommandProgram {
    if json.len() > MAX_COMMAND_BYTES {
        return CommandProgram::unsupported("PowerShell AST output limit");
    }
    let Ok(facts) = serde_json::from_str::<PowerShellFacts>(json) else {
        return CommandProgram::unsupported("PowerShell AST unavailable");
    };
    if facts.commands.len() > MAX_FACTS
        || facts.accesses.len() > MAX_FACTS
        || facts.redirects.len() > MAX_FACTS
    {
        return CommandProgram::unsupported("PowerShell AST node limit");
    }
    let has_location = facts.commands.iter().any(|argv| {
        argv.first()
            .is_some_and(|name| location::is_location_command(name, "powershell"))
    });
    if has_location && facts.tree.is_none() {
        return CommandProgram::unsupported("PowerShell directory flow unavailable");
    }
    let mut commands: Vec<Option<Ast>> = facts
        .commands
        .into_iter()
        .enumerate()
        .map(|(index, argv)| {
            let redirects = facts
                .redirects
                .get(index)
                .into_iter()
                .flatten()
                .cloned()
                .map(|target| Redirect {
                    op: ">".into(),
                    fd: None,
                    word: native_word(target),
                    body: None,
                    span: Span { start: 0, end: 0 },
                })
                .collect();
            Some(Ast::Command(
                argv.into_iter().map(native_word).collect(),
                redirects,
            ))
        })
        .collect();
    let ast = match facts.tree {
        Some(tree) => {
            let Some(ast) = native_ast(tree, &mut commands, 0, &mut 0) else {
                return CommandProgram::unsupported("PowerShell AST flow unavailable");
            };
            if commands.iter().any(Option::is_some) {
                return CommandProgram::unsupported("PowerShell AST flow is incomplete");
            }
            ast
        }
        None => Ast::Sequence(commands.into_iter().flatten().collect()),
    };
    let mut result = CommandProgram {
        complete: facts.complete,
        ..Default::default()
    };
    collect(&ast, "powershell", &Location::new(cwd), &mut result);
    // Legacy fixtures without per-command redirect facts remain conservative.
    for (action, path) in facts.accesses {
        record_access(&mut result, &Location::new(cwd), &action, &path, true);
        if action != "read" {
            for command in &mut result.commands {
                command.read_only = false;
            }
        }
    }
    if result.commands.is_empty() {
        result.complete = false;
    }
    if !result.complete {
        result.reason =
            Some("unsupported or dynamic PowerShell syntax requires confirmation".into());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_program_printf_and_find_have_bounded_read_effects() {
        for source in [
            "printf '\\n--- git ---\\n'",
            "printf '%s\\n' 'hello'",
            "printf '%%n'",
            "find . -maxdepth 2 -type f -not -path './.git/*'",
            "find . -type f -name '*.js' -o -name '*.html' | sort",
        ] {
            let p = parse_permission_program(source, "bash", "/repo");
            assert!(
                p.complete && p.commands.iter().all(|c| c.read_only),
                "{source}: {p:?}"
            );
        }
        for source in [
            "printf -v x %s value",
            "printf '%n' x",
            "find . -delete",
            "find . -exec rm '{}' \\;",
            "find . -fprint output",
            "find -L . -type f",
            "find . -name *.js",
            "wc -l $(find . -type f)",
        ] {
            let p = parse_permission_program(source, "bash", "/repo");
            assert!(
                !p.complete || p.commands.iter().any(|c| !c.read_only),
                "{source}: {p:?}"
            );
        }
    }

    #[test]
    fn command_program_globs_require_file_operand_roles_and_host_resolution() {
        for shell in ["bash", "sh", "zsh"] {
            let p = parse_permission_program(
                "grep -h -E \"name: '|subtitle: '\" games/*.js",
                shell,
                "/repo",
            );
            assert!(p.complete && p.commands[0].read_only, "{p:?}");
            assert_eq!(p.glob_accesses, ["/repo/games/*.js"]);
            assert!(!p.plan(shell, "/bin/sh", "/repo").complete);
            let literal = parse_permission_program("grep -E name games/\\*.js", shell, "/repo");
            assert!(literal.complete);
            assert!(literal.glob_accesses.is_empty());
            for source in [
                "grep *.js file.txt",
                "grep games/*.js 'games/*.js'",
                "grep -e *.js file.txt",
                "grep $PATTERN games/*.js",
                "cat **/*.js",
                "cat games/*/*.js",
                "cat games/{a,b}.js",
                "echo $(find . -type f)",
            ] {
                assert!(
                    !parse_permission_program(source, shell, "/repo").complete,
                    "{source}"
                );
            }
        }
    }
    #[test]
    fn command_program_common_read_pipelines_and_null_output() {
        for shell in ["bash", "sh", "zsh"] {
            for source in [
                "cat README.md 2>/dev/null | head -100",
                "ls -la games; echo ---; wc -l app.js styles.css; echo ---; git log --oneline -10 2>/dev/null",
                "head -c 600 styles.css; echo; echo '---'; grep -o 'snake-[a-z-]\\*' styles.css | sort -u | head; echo '--- git'; git rev-parse --is-inside-work-tree 2>&1",
                "echo -ne 'text'; echo '--unknown'; echo '--- git'",
                "echo text >/dev/null; cat < /dev/null",
            ] {
                let p = parse_permission_program(source, shell, "/repo");
                assert!(p.complete && p.commands.iter().all(|c| c.read_only), "{shell}: {source}: {p:?}");
                assert!(!p.accesses.iter().any(|(action, _)| action == "edit"), "{source}: {p:?}");
            }
            let p =
                parse_permission_program("cat README.md 2>/dev/null | head -100", shell, "/repo");
            assert!(p.requires_null_device);
            assert!(!p.accesses.iter().any(|(_, path)| path.starts_with("/dev")));
            for source in [
                "echo text > output",
                "echo text >/dev/tty",
                "echo text >/dev/null-copy",
                "echo text >./dev/null",
                "cat <>/dev/null",
            ] {
                let p = parse_permission_program(source, shell, "/repo");
                assert!(!p.requires_null_device, "{source}");
                assert!(p.commands.iter().any(|c| !c.read_only), "{source}");
            }
            assert!(
                !parse_permission_program("echo $(touch marker) >/dev/null", shell, "/repo")
                    .complete
            );
        }
    }

    #[test]
    fn powershell_file_redirect_does_not_use_posix_null_device_semantics() {
        let facts = serde_json::json!({"complete":true,"commands":[["Write-Output","hello"]],"accesses":[],"redirects":[["/dev/null"]]});
        let p = powershell_permission_program(&facts.to_string(), "/repo");
        assert!(!p.requires_null_device);
        assert!(!p.commands[0].read_only);
        assert!(p.accesses.contains(&("edit".into(), "/dev/null".into())));
    }

    #[test]
    fn command_program_read_options_keep_values_separate_from_file_targets() {
        for (source, expected) in [
            ("head -c 600 styles.css", vec!["/repo/styles.css"]),
            ("tail -n +10 app.js", vec!["/repo/app.js"]),
            ("head --bytes=600 styles.css", vec!["/repo/styles.css"]),
            ("grep -o '.env' styles.css", vec!["/repo/styles.css"]),
            (
                "grep -in -e foo -f .env styles.css",
                vec!["/repo/.env", "/repo/styles.css"],
            ),
            (
                "grep --regexp=foo --file=.env styles.css",
                vec!["/repo/.env", "/repo/styles.css"],
            ),
            ("grep -- -pattern styles.css", vec!["/repo/styles.css"]),
            ("sort -t : -k 1,1 -u styles.css", vec!["/repo/styles.css"]),
        ] {
            let p = parse_permission_program(source, "bash", "/repo");
            assert!(
                p.complete && p.commands.iter().all(|c| c.read_only),
                "{source}: {p:?}"
            );
            assert_eq!(
                p.accesses
                    .iter()
                    .map(|(_, path)| path.as_str())
                    .collect::<Vec<_>>(),
                expected,
                "{source}"
            );
        }
        for source in [
            "sort -o output styles.css",
            "sort --output=output styles.css",
            "sort --compress-program=evil",
            "sort -T outside",
            "sort --random-source=.env",
            "sort --files0-from=.env",
            "grep -r pattern games",
            "grep --unknown pattern styles.css",
            "grep -f",
            "head -c",
            "head --unknown styles.css",
            "git log --output=output",
            "git log --ext-diff -p",
            "git log -p",
        ] {
            let p = parse_permission_program(source, "bash", "/repo");
            assert!(
                p.commands.iter().any(|c| !c.read_only) || !p.complete,
                "{source}"
            );
        }
        for source in [
            "git log --oneline -10",
            "git log --oneline -n 10",
            "git log --max-count=10",
            "git log -n10",
        ] {
            let p = parse_permission_program(source, "bash", "/repo");
            assert!(
                p.complete && p.commands.iter().all(|c| c.read_only),
                "{source}"
            );
        }
    }

    #[test]
    fn directory_changes_track_success_failure_and_subshell_scope() {
        for shell in ["bash", "sh", "zsh"] {
            for source in [
                "cd ./sub",
                "cd -P ./sub && cat file",
                "pushd ./sub && popd && cat file",
            ] {
                let p = parse_permission_program(source, shell, "/repo");
                assert!(
                    p.complete && p.commands.iter().all(|c| c.read_only),
                    "{shell}: {source}: {p:?}"
                );
            }
            for (source, expected, absent) in [
                ("cd ./sub && cat file", "/repo/sub/file", "/repo/file"),
                ("cd ./sub || cat file", "/repo/file", "/repo/sub/file"),
                ("(cd ./sub); cat file", "/repo/file", "/repo/sub/file"),
                (
                    "pushd ./sub && popd && cat file",
                    "/repo/file",
                    "/repo/sub/file",
                ),
            ] {
                let p = parse_permission_program(source, shell, "/repo");
                assert!(p.complete, "{source}: {p:?}");
                assert!(
                    p.accesses.contains(&("read".into(), expected.into())),
                    "{source}: {p:?}"
                );
                assert!(
                    !p.accesses.contains(&("read".into(), absent.into())),
                    "{source}: {p:?}"
                );
            }
            let p = parse_permission_program("cd ./sub; cat file", shell, "/repo");
            assert!(p.accesses.contains(&("read".into(), "/repo/file".into())));
            assert!(p
                .accesses
                .contains(&("read".into(), "/repo/sub/file".into())));
            let p = parse_permission_program("cd ./link && cd .. && cat secret", shell, "/repo");
            assert!(p.complete);
            assert!(p.accesses.contains(&("read".into(), "/repo/secret".into())));
            assert!(p
                .accesses
                .iter()
                .any(|(_, path)| path.ends_with("/link/../secret")));
            for source in [
                "cd",
                "cd -",
                "popd",
                "cd $TARGET",
                "pushd +1",
                "cd ./sub; popd",
            ] {
                assert!(
                    !parse_permission_program(source, shell, "/repo").complete,
                    "{source}"
                );
            }
        }
        assert!(parse_permission_program("chdir ./sub && pwd", "zsh", "/repo").complete);
        assert!(parse_permission_program("cd sub", "bash", "/repo").requires_default_cdpath);
        assert!(!parse_permission_program("cd ./sub", "bash", "/repo").requires_default_cdpath);
    }

    #[test]
    fn native_directory_flow_tracks_windows_paths_and_rejects_provider_switches() {
        for name in [
            "Set-Location",
            "sl",
            "CD",
            "chdir",
            "Push-Location",
            "pushd",
        ] {
            let facts = serde_json::json!({"complete":true,"commands":[[name,"-LiteralPath","sub"],["Get-Content","file"]],"accesses":[],"tree":{"kind":"and","value":[{"kind":"command","value":0},{"kind":"command","value":1}]}});
            let p = powershell_permission_program(&facts.to_string(), "C:\\repo");
            assert!(
                p.complete && p.commands.iter().all(|c| c.read_only),
                "{name}: {p:?}"
            );
            assert!(
                p.accesses
                    .contains(&("read".into(), "C:/repo/sub/file".into())),
                "{p:?}"
            );
        }
        for target in ["Env:", "Registry::HKEY_LOCAL_MACHINE", "C:relative", "-"] {
            let facts = serde_json::json!({"complete":true,"commands":[["Set-Location",target]],"accesses":[],"tree":{"kind":"command","value":0}});
            assert!(
                !powershell_permission_program(&facts.to_string(), "C:\\repo").complete,
                "{target}"
            );
        }
    }
    #[test]
    fn command_program_preserves_commands_arguments_and_redirect_effects() {
        for shell in ["bash", "sh", "zsh"] {
            let p = parse_permission_program(
                "git clone 'url with spaces' && cat README.md | wc -l",
                shell,
                "/repo",
            );
            assert!(p.complete, "{shell}: {p:?}");
            assert_eq!(p.commands.len(), 3);
            assert_eq!(p.commands[0].argv, ["git", "clone", "url with spaces"]);
            assert!(!p.commands[0].read_only);
            assert!(p.commands[1..].iter().all(|c| c.read_only));
            assert_eq!(p.accesses, [("read".into(), "/repo/README.md".into())]);
        }
        let p = parse_permission_program("cat < input > output 2>&1", "bash", "/repo");
        assert!(p.complete);
        assert!(!p.commands[0].read_only);
        assert_eq!(
            p.accesses,
            [
                ("read".into(), "/repo/input".into()),
                ("edit".into(), "/repo/output".into())
            ]
        );
        let p =
            parse_permission_program("cat <<'EOF'\n$(rm evil); fake > path\nEOF", "bash", "/repo");
        assert!(p.complete);
        assert_eq!(p.commands.len(), 1);
        assert!(p.accesses.is_empty());
    }
    #[test]
    fn command_program_does_not_treat_unknown_syntax_as_read_only() {
        for command in [
            "cat $(rm victim)",
            "cat `rm victim`",
            "cat <(rm victim)",
            "cat $FILE",
            "cat {.env,.env.local}",
            "cat ~/secret",
            "A=b cat file",
            "env cat file",
            "bash -c 'rm x'",
            "git -c core.sshCommand=evil status",
            "git clone --upload-pack=evil repo",
            "cd - && ls",
            "set -P; ls",
            "setopt GLOB_SUBST; ls",
            "trap 'rm victim' EXIT; pwd",
            "cat > $TARGET",
            "cat <<EOF\n$(rm x)\nEOF",
            "echo x &",
            "for x in a; do echo x; done",
        ] {
            assert!(
                !parse_permission_program(command, "bash", "/repo").complete,
                "{command}"
            );
        }
        for shell in ["pwsh", "powershell", "fish", "cmd", "ksh"] {
            assert!(!parse_permission_program("git status", shell, "/repo").complete);
        }
        assert!(!parse_permission_program("git status", "bash", "C:\\repo").complete);
        assert!(!parse_permission_program("echo '", "bash", "/repo").complete);
    }
    #[test]
    fn command_program_read_only_requires_known_program_and_options() {
        for command in [
            "pwd",
            "git status --short",
            "ls -la",
            "cat README.md",
            "git diff --no-ext-diff --no-textconv --stat",
        ] {
            let p = parse_permission_program(command, "bash", "/repo");
            assert!(
                p.complete && p.commands.iter().all(|c| c.read_only),
                "{command}: {p:?}"
            );
        }
        for command in [
            "git clone repo",
            "git diff",
            "git log --output=file",
            "rg --pre=evil text",
            "ls --unknown",
            "cat file > out",
            "npm test",
            "python script.py",
        ] {
            let p = parse_permission_program(command, "bash", "/repo");
            assert!(
                p.commands.iter().any(|c| !c.read_only) || !p.complete,
                "{command}"
            );
        }
    }
    #[test]
    fn native_powershell_projection_remains_conservative() {
        let p = powershell_permission_program(
            r#"{"complete":true,"commands":[["Get-Content","C:\\repo\\file.txt"],["Set-Content","copy.txt"]],"accesses":[]}"#,
            "/repo",
        );
        assert!(p.complete);
        assert!(p.commands[0].read_only);
        assert!(!p.commands[1].read_only);
        assert_eq!(p.accesses, [("read".into(), "C:/repo/file.txt".into())]);
        assert!(
            !powershell_permission_program(
                r#"{"complete":false,"commands":[["Get-Content"]],"accesses":[]}"#,
                "/repo"
            )
            .complete
        );
        assert!(
            !powershell_permission_program(
                r#"{"complete":true,"commands":[["Get-Content","*.txt"]],"accesses":[]}"#,
                "/repo"
            )
            .complete
        );
        assert!(!powershell_permission_program("not-json", "/repo").complete);
        for name in [
            "Set-Location",
            "CD",
            "Set-Alias",
            "Import-Module",
            "Set-Variable",
        ] {
            let facts = serde_json::json!({"complete":true,"commands":[[name,"target"],["Get-Content","README.md"]],"accesses":[]});
            assert!(
                !powershell_permission_program(&facts.to_string(), "/repo").complete,
                "{name}"
            );
        }
        let facts = serde_json::json!({"complete":true,"commands":[["Set-Content","Function:git","body"]],"accesses":[]});
        assert!(!powershell_permission_program(&facts.to_string(), "/repo").complete);
    }
}
