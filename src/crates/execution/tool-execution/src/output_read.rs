//! Line-based selection over retained exec output, shared by the read paths of
//! the exec tool family. Ports the proven protocol used by AI terminal shells:
//! at most one window (`head`, `tail`, or the `from_line`/`to_line` range)
//! combines with `filter` and `max_lines`; returned lines keep their original
//! line numbers so pagination can continue with `from_line = end_line + 1`.

use openbitfun_runtime_ports::ExecOutputStream;
use regex::Regex;
use serde_json::{json, Value};

pub const DEFAULT_READ_MAX_LINES: usize = 200;
pub const MAX_READ_LINES: usize = 2000;
pub const MAX_READ_LINE_LENGTH: usize = 1000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecOutputReadOptions {
    /// Regex source; only matching lines are kept. Line numbers stay original.
    pub filter: Option<String>,
    /// Which stream to read. `None` keeps the combined view.
    pub stream: Option<ExecOutputStream>,
    /// First N lines of the (filtered) view.
    pub head: Option<usize>,
    /// Last N lines of the (filtered) view.
    pub tail: Option<usize>,
    /// Start of a 1-based inclusive range on original line numbers.
    pub from_line: Option<usize>,
    /// End of a 1-based inclusive range on original line numbers.
    pub to_line: Option<usize>,
    /// Maximum number of lines returned per read.
    pub max_lines: Option<usize>,
}

impl ExecOutputReadOptions {
    /// True when any line-selection option is present; callers use this to
    /// choose line mode over plain incremental reads.
    pub fn has_line_selection(&self) -> bool {
        self.filter.is_some()
            || self.stream.is_some()
            || self.head.is_some()
            || self.tail.is_some()
            || self.from_line.is_some()
            || self.to_line.is_some()
            || self.max_lines.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutputReadResult {
    /// The selected lines, each prefixed with its original number as `N| text`.
    pub output: String,
    /// Total lines of the stream view, before filtering and windowing.
    pub total_lines: usize,
    /// Number of lines matching `filter`, when a filter was given.
    pub matched_lines: Option<usize>,
    /// Original line number of the first returned line; 0 when nothing matched.
    pub start_line: usize,
    /// Original line number of the last returned line; 0 when nothing matched.
    pub end_line: usize,
    /// True when more lines were cut off by `max_lines`; continue with
    /// `from_line = end_line + 1`.
    pub has_more: bool,
}

impl ExecOutputReadResult {
    pub fn to_value(&self) -> Value {
        json!({
            "output": self.output,
            "total_lines": self.total_lines,
            "matched_lines": self.matched_lines,
            "start_line": self.start_line,
            "end_line": self.end_line,
            "has_more": self.has_more,
        })
    }
}

/// An unsuccessful read. Returned as a value (not an error) so the model can
/// correct its arguments on the next call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutputReadError {
    pub message: String,
}

impl ExecOutputReadError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn to_value(&self) -> Value {
        json!({ "error": self.message })
    }
}

/// Splits output text into lines the way the read paths count them: CRLF is
/// normalized and a single trailing newline does not produce an extra empty
/// line.
pub fn split_output_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect();
    if lines.len() > 1 && lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// Truncates one line for display, eliding its middle when it exceeds
/// [`MAX_READ_LINE_LENGTH`] characters.
pub fn truncate_read_line(line: &str) -> String {
    let char_count = line.chars().count();
    if char_count <= MAX_READ_LINE_LENGTH {
        return line.to_string();
    }
    let half_length = (MAX_READ_LINE_LENGTH - 30) / 2;
    let omitted_count = char_count - half_length * 2;
    let head: String = line.chars().take(half_length).collect();
    let tail: String = line
        .chars()
        .skip(char_count.saturating_sub(half_length))
        .collect();
    format!("{head} ... [{omitted_count} chars omitted] ... {tail}")
}

/// Applies [`ExecOutputReadOptions`] to `text` and returns the selected lines
/// or a structured error for invalid combinations (more than one window,
/// non-positive counts, an inverted range, or an invalid regex). Pure.
pub fn select_output_lines(
    text: &str,
    options: &ExecOutputReadOptions,
) -> Result<ExecOutputReadResult, ExecOutputReadError> {
    let ExecOutputReadOptions {
        filter,
        head,
        tail,
        from_line,
        to_line,
        max_lines,
        ..
    } = options;

    let has_range = from_line.is_some() || to_line.is_some();
    let window_count =
        usize::from(head.is_some()) + usize::from(tail.is_some()) + usize::from(has_range);
    if window_count > 1 {
        return Err(ExecOutputReadError::new(
            "Specify at most one window: head, tail, or from_line/to_line.",
        ));
    }
    for (name, value) in [
        ("head", *head),
        ("tail", *tail),
        ("from_line", *from_line),
        ("to_line", *to_line),
        ("max_lines", *max_lines),
    ] {
        if let Some(value) = value {
            if value < 1 {
                return Err(ExecOutputReadError::new(format!(
                    "\"{name}\" must be a positive integer (got {value})."
                )));
            }
        }
    }
    if let (Some(from), Some(to)) = (from_line, to_line) {
        if to < &from {
            return Err(ExecOutputReadError::new(format!(
                "\"to_line\" ({to}) must not be smaller than \"from_line\" ({from})."
            )));
        }
    }

    let regex = match filter {
        Some(source) => match Regex::new(source) {
            Ok(regex) => Some(regex),
            Err(error) => {
                return Err(ExecOutputReadError::new(format!(
                    "Invalid \"filter\" regex '{source}': {error}"
                )));
            }
        },
        None => None,
    };

    let all_lines = split_output_lines(text);
    let mut entries: Vec<(usize, &str)> = all_lines
        .iter()
        .enumerate()
        .map(|(index, line)| (index + 1, line.as_str()))
        .collect();
    if let Some(regex) = &regex {
        entries.retain(|(_, line)| regex.is_match(line));
    }
    let matched_lines = regex.as_ref().map(|_| entries.len());

    let window: Vec<(usize, &str)> = if has_range {
        // Range selection keeps original line numbers even when filtered.
        let from = from_line.unwrap_or(1);
        let to = to_line.unwrap_or(usize::MAX);
        entries
            .iter()
            .copied()
            .filter(|(n, _)| *n >= from && *n <= to)
            .collect()
    } else if let Some(head) = head {
        entries.iter().copied().take(*head).collect()
    } else if let Some(tail) = tail {
        entries[entries.len().saturating_sub(*tail)..].to_vec()
    } else {
        entries.clone()
    };

    let effective_max_lines = max_lines
        .unwrap_or(DEFAULT_READ_MAX_LINES)
        .clamp(1, MAX_READ_LINES);
    let has_more = window.len() > effective_max_lines;
    let selected = &window[..window.len().min(effective_max_lines)];

    Ok(ExecOutputReadResult {
        output: selected
            .iter()
            .map(|(n, line)| format!("{n}| {}", truncate_read_line(line)))
            .collect::<Vec<String>>()
            .join("\n"),
        total_lines: all_lines.len(),
        matched_lines,
        start_line: selected.first().map(|(n, _)| *n).unwrap_or(0),
        end_line: selected.last().map(|(n, _)| *n).unwrap_or(0),
        has_more,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> ExecOutputReadOptions {
        ExecOutputReadOptions::default()
    }

    fn sample() -> String {
        (1..=10)
            .map(|n| format!("line-{n}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn split_normalizes_crlf_and_drops_single_trailing_newline() {
        assert_eq!(split_output_lines(""), Vec::<String>::new());
        assert_eq!(split_output_lines("a\r\nb\r\n"), vec!["a", "b"]);
        assert_eq!(split_output_lines("a\n\n"), vec!["a", ""]);
    }

    #[test]
    fn select_returns_numbered_lines_with_pagination_metadata() {
        let result = select_output_lines(&sample(), &options()).expect("selection should succeed");

        assert_eq!(result.total_lines, 10);
        assert_eq!(result.start_line, 1);
        assert_eq!(result.end_line, 10);
        assert!(!result.has_more);
        assert!(result.output.starts_with("1| line-1"));
        assert!(result.output.ends_with("10| line-10"));
        assert_eq!(result.matched_lines, None);
    }

    #[test]
    fn select_respects_max_lines_and_reports_has_more() {
        let result = select_output_lines(
            &sample(),
            &ExecOutputReadOptions {
                max_lines: Some(3),
                ..options()
            },
        )
        .expect("selection should succeed");

        assert_eq!(result.end_line, 3);
        assert!(result.has_more);
        assert_eq!(result.output, "1| line-1\n2| line-2\n3| line-3".to_string());
    }

    #[test]
    fn select_range_keeps_original_numbers_for_pagination() {
        let result = select_output_lines(
            &sample(),
            &ExecOutputReadOptions {
                from_line: Some(4),
                to_line: Some(6),
                ..options()
            },
        )
        .expect("selection should succeed");

        assert_eq!(result.start_line, 4);
        assert_eq!(result.end_line, 6);
        assert_eq!(result.output, "4| line-4\n5| line-5\n6| line-6");
    }

    #[test]
    fn select_filter_keeps_original_line_numbers() {
        let result = select_output_lines(
            &sample(),
            &ExecOutputReadOptions {
                filter: Some("line-[15]$".to_string()),
                ..options()
            },
        )
        .expect("selection should succeed");

        assert_eq!(result.matched_lines, Some(2));
        assert_eq!(result.output, "1| line-1\n5| line-5");
    }

    #[test]
    fn select_head_and_tail_are_mutually_exclusive_with_range() {
        let error = select_output_lines(
            &sample(),
            &ExecOutputReadOptions {
                head: Some(2),
                tail: Some(2),
                ..options()
            },
        )
        .expect_err("two windows must fail");

        assert!(error.message.contains("at most one window"));

        let error = select_output_lines(
            &sample(),
            &ExecOutputReadOptions {
                head: Some(2),
                from_line: Some(2),
                ..options()
            },
        )
        .expect_err("window plus range must fail");

        assert!(error.message.contains("at most one window"));
    }

    #[test]
    fn select_rejects_non_positive_and_inverted_ranges() {
        let error = select_output_lines(
            &sample(),
            &ExecOutputReadOptions {
                head: Some(0),
                ..options()
            },
        )
        .expect_err("zero head must fail");
        assert!(error.message.contains("positive integer"));

        let error = select_output_lines(
            &sample(),
            &ExecOutputReadOptions {
                from_line: Some(5),
                to_line: Some(2),
                ..options()
            },
        )
        .expect_err("inverted range must fail");
        assert!(error.message.contains("must not be smaller"));
    }

    #[test]
    fn select_reports_invalid_regex_without_panicking() {
        let error = select_output_lines(
            &sample(),
            &ExecOutputReadOptions {
                filter: Some("(unclosed".to_string()),
                ..options()
            },
        )
        .expect_err("invalid regex must fail");

        assert!(error.message.contains("Invalid \"filter\" regex"));
    }

    #[test]
    fn select_tail_takes_last_n_of_filtered_view() {
        let result = select_output_lines(
            &sample(),
            &ExecOutputReadOptions {
                tail: Some(2),
                ..options()
            },
        )
        .expect("selection should succeed");

        assert_eq!(result.output, "9| line-9\n10| line-10");
    }

    #[test]
    fn truncate_read_line_elides_the_middle_beyond_the_budget() {
        let long = "a".repeat(MAX_READ_LINE_LENGTH + 500);
        let truncated = truncate_read_line(&long);

        assert!(truncated.contains("chars omitted] ..."));
        assert!(truncated.chars().count() < long.chars().count());

        let exact = "b".repeat(MAX_READ_LINE_LENGTH);
        assert_eq!(truncate_read_line(&exact), exact);
    }

    #[test]
    fn max_lines_defaults_and_clamps() {
        let text = (1..=300)
            .map(|n| format!("row-{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let defaulted = select_output_lines(&text, &options()).expect("default selection");
        assert_eq!(defaulted.end_line, DEFAULT_READ_MAX_LINES);
        assert!(defaulted.has_more);

        let oversized_text = (1..=3000)
            .map(|n| format!("row-{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let oversized = select_output_lines(
            &oversized_text,
            &ExecOutputReadOptions {
                max_lines: Some(MAX_READ_LINES + 5),
                ..options()
            },
        )
        .expect("clamped selection");
        assert_eq!(oversized.end_line, MAX_READ_LINES);
        assert!(oversized.has_more);
    }
}
