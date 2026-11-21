//! Parameter-aware read effects for common command-line filters.
use super::CommandReads;

pub(super) fn printf(args: &[String]) -> bool {
    let args = if args.first().is_some_and(|arg| arg == "--") {
        &args[1..]
    } else {
        args
    };
    let Some(format) = args.first() else {
        return false;
    };
    if format.starts_with('-') {
        return false;
    }
    let mut format = format.chars();
    while let Some(character) = format.next() {
        if character != '%' {
            continue;
        }
        if format.clone().next() == Some('%') {
            format.next();
            continue;
        }
        // The common output conversions only; %n can assign a shell variable.
        let mut conversion = None;
        for character in format.by_ref() {
            if character.is_ascii_alphabetic() {
                conversion = Some(character);
                break;
            }
            if !"#0- +'*.0123456789".contains(character) {
                return false;
            }
        }
        if !conversion.is_some_and(|c| "bqscdiouxXeEfFgGaA".contains(c)) {
            return false;
        }
    }
    true
}

pub(super) fn find(args: &[String]) -> (bool, CommandReads) {
    let mut paths = vec![];
    let mut index = usize::from(args.first().is_some_and(|arg| arg == "-P"));
    while let Some(arg) = args.get(index) {
        if arg.starts_with('-') || matches!(arg.as_str(), "!" | "(") {
            break;
        }
        paths.push((Some(index + 1), arg.clone()));
        index += 1;
    }
    if paths.is_empty() {
        paths.push((None, ".".into()));
    }
    while let Some(arg) = args.get(index) {
        index += 1;
        match arg.as_str() {
            "-name" | "-iname" | "-path" | "-ipath" => {
                if args.get(index).is_none() {
                    return (false, vec![]);
                }
                index += 1;
            }
            "-type" => {
                if !args.get(index).is_some_and(|kind| {
                    matches!(kind.as_str(), "f" | "d" | "l" | "b" | "c" | "p" | "s")
                }) {
                    return (false, vec![]);
                }
                index += 1;
            }
            "-maxdepth" | "-mindepth" => {
                if !args.get(index).is_some_and(|value| {
                    !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit())
                }) {
                    return (false, vec![]);
                }
                index += 1;
            }
            "(" | ")" | "!" | "-not" | "-a" | "-and" | "-o" | "-or" | "-true" | "-false"
            | "-empty" | "-print" | "-print0" | "-ls" | "-prune" | "-quit" => {}
            _ => return (false, vec![]),
        }
    }
    (true, paths)
}

pub(super) fn integer(value: &str) -> bool {
    let value = value.strip_prefix(['+', '-']).unwrap_or(value);
    !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit())
}

pub(super) fn head_tail(args: &[String]) -> (bool, CommandReads) {
    let mut paths = vec![];
    let mut literal = false;
    let mut operands = false;
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        index += 1;
        if !literal && arg == "--" && !operands {
            literal = true;
            continue;
        }
        if !literal && arg.starts_with('-') && arg != "-" {
            if operands {
                return (false, vec![]);
            }
            if matches!(arg.as_str(), "-n" | "-c" | "--lines" | "--bytes") {
                if !args.get(index).is_some_and(|value| integer(value)) {
                    return (false, vec![]);
                }
                index += 1;
            } else if let Some(value) = arg
                .strip_prefix("--lines=")
                .or_else(|| arg.strip_prefix("--bytes="))
            {
                if !integer(value) {
                    return (false, vec![]);
                }
            } else if let Some(value) = arg.strip_prefix("-n").or_else(|| arg.strip_prefix("-c")) {
                if !integer(value) {
                    return (false, vec![]);
                }
            } else if !matches!(
                arg.as_str(),
                "-q" | "-v" | "--quiet" | "--silent" | "--verbose"
            ) && !(arg.len() > 1 && arg[1..].bytes().all(|c| c.is_ascii_digit()))
            {
                return (false, vec![]);
            }
        } else {
            operands = true;
            if arg != "-" {
                paths.push((Some(index), arg.clone()));
            }
        }
    }
    (true, paths)
}

pub(super) fn grep(args: &[String]) -> (bool, CommandReads) {
    let mut paths = vec![];
    let mut pattern = false;
    let mut literal = false;
    let mut operands = false;
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        index += 1;
        if !literal && !operands && arg == "--" {
            literal = true;
            continue;
        }
        if !literal && arg.starts_with('-') && arg != "-" {
            if operands {
                return (false, vec![]);
            }
            if let Some(long) = arg.strip_prefix("--") {
                let (option, attached) = long
                    .split_once('=')
                    .map_or((long, None), |(name, value)| (name, Some(value)));
                match option {
                    "regexp" | "file" | "max-count" | "after-context" | "before-context"
                    | "context" => {
                        let Some(value) = attached.or_else(|| {
                            let value = args.get(index);
                            index += 1;
                            value.map(String::as_str)
                        }) else {
                            return (false, vec![]);
                        };
                        match option {
                            "regexp" => pattern = true,
                            "file" => {
                                pattern = true;
                                paths.push((None, value.into()));
                            }
                            _ if !integer(value) => return (false, vec![]),
                            _ => {}
                        }
                    }
                    "extended-regexp"
                    | "fixed-strings"
                    | "basic-regexp"
                    | "perl-regexp"
                    | "ignore-case"
                    | "invert-match"
                    | "line-number"
                    | "count"
                    | "only-matching"
                    | "files-with-matches"
                    | "files-without-match"
                    | "quiet"
                    | "silent"
                    | "no-messages"
                    | "no-filename"
                    | "with-filename"
                    | "word-regexp"
                    | "line-regexp"
                    | "text"
                    | "null"
                    | "null-data"
                        if attached.is_none() => {}
                    _ => return (false, vec![]),
                }
            } else {
                let mut options = arg[1..].char_indices().peekable();
                while let Some((offset, option)) = options.next() {
                    if "efmABC".contains(option) {
                        let rest = &arg[1 + offset + option.len_utf8()..];
                        let value = if rest.is_empty() {
                            let Some(value) = args.get(index) else {
                                return (false, vec![]);
                            };
                            index += 1;
                            value.as_str()
                        } else {
                            rest
                        };
                        match option {
                            'e' => pattern = true,
                            'f' => {
                                pattern = true;
                                paths.push((None, value.into()));
                            }
                            _ if !integer(value) => return (false, vec![]),
                            _ => {}
                        }
                        break;
                    }
                    if !"EFGPivnclLqshHwxoaIbUZz".contains(option) {
                        return (false, vec![]);
                    }
                }
            }
        } else {
            operands = true;
            if !pattern {
                pattern = true;
            } else if arg != "-" {
                paths.push((Some(index), arg.clone()));
            }
        }
    }
    (pattern, paths)
}

pub(super) fn sort(args: &[String]) -> (bool, CommandReads) {
    let mut paths = vec![];
    let mut literal = false;
    let mut operands = false;
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        index += 1;
        if !literal && !operands && arg == "--" {
            literal = true;
            continue;
        }
        if !literal && arg.starts_with('-') && arg != "-" {
            if operands {
                return (false, vec![]);
            }
            if let Some(long) = arg.strip_prefix("--") {
                let (option, attached) = long
                    .split_once('=')
                    .map_or((long, None), |(name, value)| (name, Some(value)));
                match option {
                    "key" | "field-separator" => {
                        if attached.is_none() {
                            if args.get(index).is_none() {
                                return (false, vec![]);
                            }
                            index += 1;
                        }
                    }
                    "ignore-leading-blanks"
                    | "dictionary-order"
                    | "ignore-case"
                    | "general-numeric-sort"
                    | "human-numeric-sort"
                    | "ignore-nonprinting"
                    | "month-sort"
                    | "numeric-sort"
                    | "reverse"
                    | "random-sort"
                    | "stable"
                    | "unique"
                    | "version-sort"
                    | "zero-terminated"
                    | "check"
                        if attached.is_none() => {}
                    _ => return (false, vec![]),
                }
            } else {
                for (offset, option) in arg[1..].char_indices() {
                    if matches!(option, 'k' | 't') {
                        if arg.len() == 1 + offset + option.len_utf8() {
                            if args.get(index).is_none() {
                                return (false, vec![]);
                            }
                            index += 1;
                        }
                        break;
                    }
                    if !"bdfghiMnrRsuVzcC".contains(option) {
                        return (false, vec![]);
                    }
                }
            }
        } else {
            operands = true;
            if arg != "-" {
                paths.push((Some(index), arg.clone()));
            }
        }
    }
    (true, paths)
}

pub(super) fn git_log(args: &[String]) -> bool {
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        index += 1;
        if matches!(
            arg.as_str(),
            "--oneline"
                | "--graph"
                | "--decorate"
                | "--no-decorate"
                | "--all"
                | "--branches"
                | "--tags"
                | "--remotes"
                | "--date-order"
                | "--topo-order"
                | "--reverse"
                | "--no-patch"
                | "--no-ext-diff"
                | "--no-textconv"
        ) {
            continue;
        }
        if matches!(arg.as_str(), "-n" | "--max-count") {
            if !args.get(index).is_some_and(|value| integer(value)) {
                return false;
            }
            index += 1;
            continue;
        }
        if let Some(value) = arg
            .strip_prefix("--max-count=")
            .or_else(|| arg.strip_prefix("-n"))
        {
            if !integer(value) {
                return false;
            }
            continue;
        }
        if let Some(value) = arg.strip_prefix('-') {
            if integer(value) {
                continue;
            }
        }
        return false;
    }
    true
}
