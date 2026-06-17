//! Lossless text operations. Each of these strips content that carries no
//! semantic value for an agent: ANSI colour codes, terminal control chars,
//! progress-bar redraws, consecutive blank lines, consecutive identical
//! lines, and git UI hint lines.
//!
//! These are composable. The full pipeline is [`lossless_pipeline`].

use regex::Regex;
use std::sync::OnceLock;

/// Strip ANSI CSI escape sequences (colour, cursor movement, erase) and
/// OSC sequences (often used for hyperlinks or window titles).
///
/// Also strips a few standalone control chars that commonly appear in TTY
/// output: BEL (`\x07`), vertical tab (`\x0b`), form feed (`\x0c`).
pub fn strip_ansi(input: &str) -> String {
    static CSI: OnceLock<Regex> = OnceLock::new();
    static OSC: OnceLock<Regex> = OnceLock::new();
    let csi = CSI.get_or_init(|| Regex::new(r"\x1b\[[\x30-\x3f]*[\x20-\x2f]*[\x40-\x7e]").unwrap());
    // OSC: ESC ] ... BEL  or  ESC ] ... ESC \
    let osc = OSC.get_or_init(|| Regex::new(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)").unwrap());

    let s = osc.replace_all(input, "");
    let s = csi.replace_all(&s, "");
    s.chars()
        .filter(|c| !matches!(*c, '\x07' | '\x0b' | '\x0c'))
        .collect()
}

/// Collapse progress-bar redraws. Everything before the last `\r` on a line
/// has been overwritten, so only the final segment is meaningful.
pub fn collapse_carriage_returns(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for line in input.split_inclusive('\n') {
        // Strip trailing newline for processing, re-add after.
        let (body, newline) = if let Some(stripped) = line.strip_suffix('\n') {
            (stripped, "\n")
        } else {
            (line, "")
        };
        if body.contains('\r') {
            // Take the final segment after the last '\r'.
            let last = body.rsplit('\r').next().unwrap_or("");
            if !last.is_empty() {
                out.push_str(last);
            }
        } else {
            out.push_str(body);
        }
        out.push_str(newline);
    }
    out
}

/// Collapse runs of blank/whitespace-only lines to a single blank line.
pub fn collapse_blank_lines(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }
    let ends_nl = input.ends_with('\n');
    let mut lines: Vec<&str> = input.split('\n').collect();
    if ends_nl {
        // split('\n') on "a\n" returns ["a", ""]; drop the trailing empty so
        // we don't treat it as an extra blank line.
        lines.pop();
    }
    let mut out = String::with_capacity(input.len());
    let mut prev_blank = false;
    for line in lines {
        let is_blank = line.trim().is_empty();
        if is_blank && prev_blank {
            continue;
        }
        out.push_str(line);
        out.push('\n');
        prev_blank = is_blank;
    }
    if !ends_nl && out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Deduplicate consecutive identical lines, replacing runs with
/// `(repeated N times)` markers.
pub fn dedup_consecutive_lines(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }
    let ends_nl = input.ends_with('\n');
    let mut lines: Vec<&str> = input.split('\n').collect();
    if ends_nl {
        lines.pop();
    }
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < lines.len() {
        let current = lines[i];
        let mut count = 1;
        while i + count < lines.len() && lines[i + count] == current {
            count += 1;
        }
        if current.trim().is_empty() {
            // Blank lines are the job of collapse_blank_lines, not dedup.
            // Emit each blank once so the two ops compose cleanly.
            for _ in 0..count {
                out.push('\n');
            }
        } else {
            out.push_str(current);
            out.push('\n');
            if count > 1 {
                out.push_str(&format!("(repeated {count} times)\n"));
            }
        }
        i += count;
    }
    if !ends_nl && out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Strip git UI hint lines such as `  (use "git add ..." )`.
pub fn strip_git_hints(input: &str) -> String {
    static HINT: OnceLock<Regex> = OnceLock::new();
    let hint = HINT.get_or_init(|| Regex::new(r#"^\s*\(use "git [^"]*"[^)]*\)\s*$"#).unwrap());
    input
        .split('\n')
        .filter(|l| !hint.is_match(l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Apply every lossless op in the canonical order.
pub fn lossless_pipeline(input: &str) -> String {
    let s = strip_ansi(input);
    let s = collapse_carriage_returns(&s);
    let s = strip_git_hints(&s);
    let s = dedup_consecutive_lines(&s);
    collapse_blank_lines(&s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn strip_ansi_removes_colour_codes() {
        let input = "\x1b[31mred\x1b[0m and \x1b[1;32mbold green\x1b[0m";
        assert_eq!(strip_ansi(input), "red and bold green");
    }

    #[test]
    fn strip_ansi_removes_cursor_movement() {
        let input = "line1\x1b[2Kline2\x1b[1Aline3";
        assert_eq!(strip_ansi(input), "line1line2line3");
    }

    #[test]
    fn strip_ansi_removes_bel() {
        assert_eq!(strip_ansi("beep\x07boop"), "beepboop");
    }

    #[test]
    fn strip_ansi_removes_osc_hyperlink() {
        let input = "\x1b]8;;https://example.com\x07link\x1b]8;;\x07 text";
        assert_eq!(strip_ansi(input), "link text");
    }

    #[test]
    fn strip_ansi_on_clean_input_is_identity() {
        let clean = "just some text\nwith newlines\n";
        assert_eq!(strip_ansi(clean), clean);
    }

    #[test]
    fn collapse_cr_keeps_final_segment() {
        let input = "10%\r30%\r100% done\n";
        assert_eq!(collapse_carriage_returns(input), "100% done\n");
    }

    #[test]
    fn collapse_cr_preserves_normal_lines() {
        let input = "line a\nline b\n";
        assert_eq!(collapse_carriage_returns(input), "line a\nline b\n");
    }

    #[test]
    fn collapse_blank_lines_to_one() {
        let input = "a\n\n\n\nb\n";
        assert_eq!(collapse_blank_lines(input), "a\n\nb\n");
    }

    #[test]
    fn collapse_blank_lines_preserves_single() {
        let input = "a\n\nb\n";
        assert_eq!(collapse_blank_lines(input), "a\n\nb\n");
    }

    #[test]
    fn dedup_collapses_runs() {
        let input = "hello\nhello\nhello\nworld\n";
        assert_eq!(
            dedup_consecutive_lines(input),
            "hello\n(repeated 3 times)\nworld\n"
        );
    }

    #[test]
    fn dedup_leaves_nonrepeats_alone() {
        let input = "a\nb\nc\n";
        assert_eq!(dedup_consecutive_lines(input), "a\nb\nc\n");
    }

    #[test]
    fn dedup_does_not_count_blank_lines() {
        let input = "\n\n\n";
        assert_eq!(dedup_consecutive_lines(input), "\n\n\n");
    }

    #[test]
    fn strip_git_hints_removes_use_lines() {
        let input = "On branch main\n  (use \"git add <file>...\" to update)\nmodified: foo\n";
        assert_eq!(strip_git_hints(input), "On branch main\nmodified: foo\n");
    }

    #[test]
    fn pipeline_composes_ops() {
        let input = "\x1b[31mloading...\r\x1b[0mdone\ndone\n\n\nfinal\n";
        let out = lossless_pipeline(input);
        assert!(out.contains("done"));
        assert!(!out.contains("\x1b"));
        assert!(!out.contains("loading..."));
    }
}
