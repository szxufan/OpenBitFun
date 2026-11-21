# Complete shell command analysis

This module extracts syntax facts for task file constraints and a separate conservative command-program projection for permission planning. It performs no IO,
reads no environment variables or scripts, launches no process, and returns no
permission decision. An empty operation list is not proof that a program is
read-only. Core applies session constraints and filesystem/path policy separately.

## Supported subset

The lexer retains quoting, escapes, expansion attributes, redirection kinds,
source spans and a queued here-doc body for each delimiter. The parser recognizes
simple commands, sequences, `&&`, `||`, pipelines and subshells. Static Bash FD
copy/close/move is separate from file open/append/read-write operations. Compact
redirects and Bash `>& filename`, `&>` and `&>>` are file operations. For `sh`,
Bash-only redirects are unsupported; zsh support is limited to this common subset.
The executable's own version still determines whether a syntax is valid at run
time (for example, Bash 3.2 does not implement `&>>`). No command is rewritten.

Quoted here-docs consumed by ordinary commands remain data. Bash/sh/zsh `-c`,
here-doc and here-string programs are recursively analyzed. Command/process/
backtick substitutions are conservatively unresolved, including substitutions
outside a literal answer body. Python/Node recognizers are retained from the
legacy guard but run only on extracted interpreter programs. Their literal
operations are useful facts, not completeness evidence; a recognized write also
retains an unresolved effect so another dynamic write cannot be washed out.
External scripts, unknown wrapper options, eval, xargs, patch and implicit Git
worktree changes remain unresolved. Ordinary build/test programs retain the
existing execution policy; their runtime side effects are outside this analyzer.

Absolute `cd` without parent traversal propagates success and failure directory
sets. Semicolon and `||` paths retain possible old directories. Subshell and
pipeline directories never escape into the parent. Relative/logical `cd`,
CDPATH/OLDPWD-sensitive changes, functions, loops and branch syntax are either
uncertain or unsupported. POSIX analysis requires a POSIX cwd; Windows shell path
translation is not inferred from a client-side drive path.

Spans are half-open UTF-8 byte ranges in the original command. Nested decoded
program facts refer to their containing argument/body span because quote removal
and `<<-` tab stripping can change offsets. Diagnostics contain bounded reasons
and escaped paths, never entire commands, here-doc bodies or environment values.

## Parser choice and bounds

A purpose-built lexer and recursive-descent parser keep the supported language
small and explicit. Tree-sitter Bash 0.25.1 was evaluated but not adopted: its
error recovery on multiple here-docs could expose literal body data as commands.
`shlex`/`shell-words` do not preserve the required syntax contexts. No new external
dependency is needed; the owner-local `shell-analysis` feature gates the parser,
while the interpreter recognizers reuse the baseline filename matcher's regex dependency.

Limits: 1 MiB source, 32 group/program/control nesting levels, 32,768 tokens,
4,096 file or descriptor facts, 8 possible directories, and 64 diagnostic facts.
Core additionally bounds path inspection to 32 KiB and 256 components per target.
Exceeded limits are explicit failures, never empty successful results. There is
no analysis or permission cache. Core retains bounded shell execution selections
between preflight and dispatch and rechecks constraints immediately before spawn.

## Integration and compatibility

Only complete ExecCommand uses this module. WriteStdin, Git's existing facade,
and direct-file tools retain their existing guards. No task data schema changes,
new force option, protection disable switch, or constraint relaxation is added.
Shell errors retain `failure_kind=edit_constraint_guard`, outer validation errors,
and `blocks_input_rewrite`, and add a bounded JSON details record to both metadata
and the existing model observation string. Thus old consumers still read the
message while the model receives the classification and `executed=false`.

Local path checks inspect the nearest existing ancestor, follow symlinks, and
then reuse tool path resolution. Remote checks use only the workspace filesystem
port. Until that port can resolve links authoritatively, symlinks and unavailable
metadata are unresolved; no local mirror supplies evidence. Existing remote cwd
containment remains unchanged. This does not remove symlink races, startup-file
side effects, alias/function overrides, cross-process FD state or the need for
runtime permissions, sandboxing and mount restrictions.

## Verification

Run the focused commands in the owner and Core guides. The 85-case fixture is
copied unchanged in semantic content; it is parser data, never a shell script.
Actual execution fixtures construct isolated temporary directories. The ignored
archive and normal-sample replay tests require explicit input/output paths and
call analyzers only. Do not execute archive commands or read their script inputs.
Tests inject an in-memory constraint snapshot only at the session-state lookup
boundary; parser, path checks, tool dispatch and Hook pipeline remain real code.

Rollback the complete analyzer feature and ExecCommand adapter changes together.
There is no data migration. Do not deploy a partial integration or disable the
existing guard as a fallback; the legacy parser's known defects would return.

## Command permission projection

`parse_permission_program` walks the existing AST without changing the constraint
analyzer or its acceptance fixtures. It exposes literal argv nodes and file
accesses. It never treats an empty mutation list as read-only evidence. Known
read-only commands use an explicit, parameter-aware effect classifier; unknown
programs require a rule or confirmation. Unresolved dynamic expansion, environment wrappers,
unresolved directory state, executable interpreter input, version-dependent syntax, and
unsupported dialects cannot gain wildcard approval from partial parsing.
PowerShell uses the execution host's selected native parser through the fixed,
profile-free Core helper; only its bounded AST projection enters this module.
Bash/sh/zsh use the supported common subset. Git Bash with a Windows cwd and
other dialects currently remain on the explicit approval path.

Common read filters include parameter-aware `head`/`tail` counts, `grep` patterns
and pattern-file inputs, stdout-only `sort` modes, and non-patch `git log`
format/count options. Pattern text and numeric/format arguments are not file
targets; explicit input and pattern files retain their file permissions. Unknown
options, recursive grep, sort output/temp/compression options, and log output or
patch options remain outside the automatic subset. Internal program configuration
and temporary-file behavior remain within the trusted environment below.
POSIX `echo` is an output-only command regardless of leading dashes in its text;
its expansions and redirects are still analyzed independently.
Pure-output `printf` formats and find's listing/filter expressions are also
recognized. `printf -v`/`%n`, find execution/deletion/file-output actions and link
following flags remain outside the automatic subset.

Unquoted final-component `*`/`?` globs in known file operand positions are resolved
through the execution workspace filesystem. Command names, patterns, option values,
directory changes, mixed quoting, recursive/multi-directory globs and command
substitutions do not use this path. No shell runs to obtain expansions. Pending
globs cannot become complete permission plans until Core resolves every target.
Command rules match the literal AST argument words; expanded paths are checked
independently as file resources.
Expansion is bounded to 64 patterns, 1,024 directory entries per pattern and
4,096 accesses within the existing three-second path inspection deadline. Each
match retains normal sensitive-file, symlink and external-directory checks.
Dotfiles are checked conservatively, and option-like/unrepresentable filenames
or unavailable/truncated metadata require confirmation. Remote expansion never
uses a controller-local mirror. Shell profiles/options and filesystem changes
between inspection and execution retain the trust/race limits described below.

Only an exact POSIX `/dev/null` output redirect can discard output without an
edit/external-directory intent. Core first confirms a non-symlink character
device on the execution host (local metadata or a fixed, bounded remote `test`
probe). Failed/unavailable evidence leaves the program requiring confirmation.
PowerShell file redirects and all other `/dev` paths retain normal file policy.

The runtime evaluates `process.execute` rules per command, with
`process.execute.read_only` as a subordinate default that explicit execution
rules can restrict. Rule patterns are shell-independent argv patterns: quotes
group arguments, `*`/`?` match within one argument, and a final standalone `*`
matches remaining arguments. They do not match across shell operators.
Literal full-program and deliberate universal allows remain explicit authority.
Legacy `bash` rules participate in the same matching; old remembered Bash grants
stay exact. New remembered approvals bind the shell path/kind, cwd, source and
TTY mode. File reads, writes and external directories are checked independently.
Path evidence reuses the constraint guard's physical/no-follow inspection; missing
remote metadata or unresolved remote links leave the command asking for approval
without inspecting a controller-local mirror.

Analysis uses the same frozen shell selection as dispatch, including after Hook
rewrites. Selections are invocation-owned and released after success, rejection
or cancellation. No permission decision is cached. Shell profiles, aliases,
functions, executable lookup and trusted program configuration retain their
existing host semantics: this is approval classification, not an OS sandbox.

Literal directory changes are classified as read-only: POSIX `cd`, zsh `chdir`,
`pushd`/`popd`, and PowerShell `Set-Location`/`sl`/`cd`/`chdir` plus
`Push-Location`/`Pop-Location` and their `pushd`/`popd` aliases. The projection
tracks successful and failed changes across `&&`, `||`, sequences, pipelines
and subshells, including a known directory stack. Subsequent file accesses and
redirects use all possible directories. Both physical and logical parent
traversal candidates are inspected, keeping symlink resolution in Core.
Directory targets still require their normal file/external-directory permission.
Implicit home/previous-directory changes, unknown stacks and PowerShell provider
switches remain unresolved. POSIX bare relative targets rely on the trusted
shell's default directory search: a nonempty host `CDPATH`, or an unavailable
remote search environment, requires confirmation; `./relative` and absolute
paths avoid that ambiguity. Profiles and custom aliases retain the trust boundary
described above. Directory flow is bounded to eight possible paths, 32 stack
frames and 32 KiB/256 components per path.

Native PowerShell opt-in verification (works with a trusted Windows PowerShell
or PowerShell Core executable):

```bash
OPENBITFUN_TEST_POWERSHELL=/absolute/path/to/pwsh cargo test --locked -p openbitfun-core --no-default-features --features agent-runtime,git --lib command_permission_native_powershell_ast -- --ignored
```
