//! Parser for Vitest text-reporter output.

use crate::core::{CommandParser, CompressOptions, CompressedOutput, Strategy};
use crate::lossless::strip_ansi;
use crate::parsers::test_common::{TestFailure, TestReport, render};

pub struct VitestParser;

impl CommandParser for VitestParser {
    fn strategy(&self) -> Strategy {
        Strategy::Vitest
    }

    fn can_handle(&self, command: &str, output: &str) -> bool {
        if command_is_vitest(command) {
            return true;
        }
        output.contains("Test Files") && output.contains("Tests ")
            || output.contains("RUN  v")
    }

    fn parse(&self, output: &str, opts: &CompressOptions) -> CompressedOutput {
        let clean = strip_ansi(output);
        let report = parse_report(&clean);
        render(
            &report,
            Strategy::Vitest,
            opts.stack_traces,
            opts.passing_tests,
        )
    }
}

fn command_is_vitest(command: &str) -> bool {
    command.split_whitespace().any(|t| t == "vitest")
}

fn parse_report(input: &str) -> TestReport {
    let mut report = TestReport::default();
    let mut current_file: Option<String> = None;
    let mut pending_failure: Option<TestFailure> = None;

    for raw in input.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();

        // File header: `✓ src/foo.test.ts (5)` or `❯ src/pay.test.ts (3) 1 failed`
        // Vitest also uses `FAIL  src/pay.test.ts > test name` inline sometimes.
        if let Some(rest) = strip_mark(trimmed, '✓') {
            if let Some((path, count)) = extract_file_header(rest) {
                current_file = Some(path.clone());
                let n = count.unwrap_or(0);
                if n > 0 {
                    *report.passed_per_file.entry(path.clone()).or_insert(0) += n;
                    *report.total_per_file.entry(path).or_insert(0) += n;
                    report.total_passed += n;
                }
                continue;
            }
            // Per-test line: just record as passing under the current file.
            if let Some(file) = &current_file {
                *report.passed_per_file.entry(file.clone()).or_insert(0) += 1;
                *report.total_per_file.entry(file.clone()).or_insert(0) += 1;
                report.total_passed += 1;
            }
            continue;
        }

        if let Some(rest) = strip_mark(trimmed, '❯') {
            if let Some((path, _count)) = extract_file_header(rest) {
                current_file = Some(path);
                continue;
            }
        }

        if let Some(rest) = strip_any_mark(trimmed, &['✗', '×', 'x']) {
            // Individual failing test.
            if let Some(f) = pending_failure.take() {
                report.failures.push(f);
            }
            let file = current_file.clone().unwrap_or_else(|| "(unknown)".to_string());
            pending_failure = Some(TestFailure {
                file: file.clone(),
                test: rest.split(" (").next().unwrap_or(rest).to_string(),
                messages: Vec::new(),
                stack: Vec::new(),
            });
            *report.total_per_file.entry(file).or_insert(0) += 1;
            report.total_failed += 1;
            continue;
        }

        // Summary lines end any pending failure block and update totals. These
        // MUST be checked before the pending_failure catch-all below, otherwise
        // the summary text gets absorbed into the preceding failure's
        // assertion messages.
        if let Some(rest) = trimmed.strip_prefix("Test Files") {
            if let Some(f) = pending_failure.take() {
                report.failures.push(f);
            }
            parse_totals_line(rest, &mut report, /*files=*/ true);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("Tests") {
            if let Some(f) = pending_failure.take() {
                report.failures.push(f);
            }
            parse_totals_line(rest, &mut report, /*files=*/ false);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("Duration") {
            if let Some(f) = pending_failure.take() {
                report.failures.push(f);
            }
            report.duration = Some(rest.trim().trim_start_matches(':').trim().to_string());
            continue;
        }

        if let Some(f) = pending_failure.as_mut() {
            // Arrow-prefixed error description.
            if let Some(msg) = trimmed.strip_prefix("→ ").or_else(|| trimmed.strip_prefix("=> ")) {
                f.messages.push(msg.to_string());
                continue;
            }
            if is_stack_frame(trimmed) {
                f.stack.push(trimmed.to_string());
                continue;
            }
            if !trimmed.is_empty() && !trimmed.starts_with("⎯") {
                f.messages.push(trimmed.to_string());
                continue;
            }
        }
    }
    if let Some(f) = pending_failure {
        report.failures.push(f);
    }
    report
}

fn strip_mark(s: &str, mark: char) -> Option<&str> {
    let mut chars = s.chars();
    let first = chars.next()?;
    if first == mark {
        let rest = chars.as_str();
        Some(rest.trim_start())
    } else {
        None
    }
}

fn strip_any_mark<'a>(s: &'a str, marks: &[char]) -> Option<&'a str> {
    let mut chars = s.chars();
    let first = chars.next()?;
    if marks.contains(&first) {
        Some(chars.as_str().trim_start())
    } else {
        None
    }
}

fn extract_file_header(rest: &str) -> Option<(String, Option<usize>)> {
    // Shape: `path/to/file (N)` or `path/to/file (N) extra`
    // File must look like a path with an extension.
    let path_end = rest.find(" (")?;
    let path = &rest[..path_end];
    if !looks_like_file(path) {
        return None;
    }
    let after = &rest[path_end + 2..];
    let count_end = after.find(')')?;
    let count_str = &after[..count_end];
    let count: Option<usize> = count_str.parse().ok();
    Some((path.to_string(), count))
}

fn looks_like_file(path: &str) -> bool {
    path.contains('.') && (path.contains('/') || path.contains('\\'))
}

fn parse_totals_line(rest: &str, report: &mut TestReport, files: bool) {
    // e.g. "  1 failed | 11 passed (12)" or ":  1 failed | 2 passed"
    let text = rest.trim_start_matches(':').trim();
    let mut failed: Option<usize> = None;
    let mut passed: Option<usize> = None;
    let mut skipped: Option<usize> = None;
    for part in text.split('|') {
        let p = part.trim();
        if let Some(n) = pair(p, "failed") {
            failed = Some(n);
        } else if let Some(n) = pair(p, "passed") {
            passed = Some(n);
        } else if let Some(n) = pair(p, "skipped") {
            skipped = Some(n);
        }
    }
    // Only overwrite report totals for the Tests line - Test Files is per-file.
    if !files {
        if let Some(f) = failed {
            report.total_failed = f;
        }
        if let Some(p) = passed {
            report.total_passed = p;
        }
        if let Some(s) = skipped {
            report.total_skipped = s;
        }
    }
}

fn pair(s: &str, keyword: &str) -> Option<usize> {
    let idx = s.find(keyword)?;
    let prefix = s[..idx].trim().trim_end_matches(',');
    prefix.parse().ok()
}

fn is_stack_frame(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("at ") || t.starts_with("❯ ") && t.contains(".ts:") || t.contains(":") && t.contains(".js:")
}

#[cfg(test)]
mod tests {
    use super::*;

    const HAPPY_PATH: &str = "\
 ✓ src/auth.test.ts (5)
 ✓ src/api.test.ts (4)

Test Files  2 passed (2)
     Tests  9 passed (9)
  Duration  420ms
";

    const FAILING: &str = "\
 ✓ src/auth.test.ts (5)
 ❯ src/pay.test.ts (3)
   ✓ authorize
   ✗ charge credit card
     → expected 100 to be 99
   ✓ refund

Test Files  1 failed | 1 passed (2)
     Tests  1 failed | 8 passed (9)
  Duration  1.2s
";

    #[test]
    fn happy_path() {
        let p = VitestParser;
        let out = p.parse(HAPPY_PATH, &CompressOptions::default());
        assert!(out.content.starts_with("9 passed"));
        assert!(out.content.contains("src/auth.test.ts (5)"));
        assert!(out.content.contains("src/api.test.ts (4)"));
    }

    #[test]
    fn failing_case() {
        let p = VitestParser;
        let out = p.parse(FAILING, &CompressOptions::default());
        assert!(
            out.content.contains("1 failed, 8 passed"),
            "expected summary '1 failed, 8 passed' in:\n{}",
            out.content
        );
        assert!(out.content.contains("charge credit card"));
        assert!(out.content.contains("expected 100 to be 99"));
    }

    #[test]
    fn empty_input_safe() {
        let p = VitestParser;
        let out = p.parse("", &CompressOptions::default());
        assert!(out.content.starts_with("0 tests"));
    }

    #[test]
    fn detects_command() {
        let p = VitestParser;
        assert!(p.can_handle("vitest", ""));
        assert!(p.can_handle("npx vitest run", ""));
    }

    #[test]
    fn lossless_failure_message_preserved() {
        let p = VitestParser;
        let out = p.parse(FAILING, &CompressOptions::default());
        assert!(out.content.contains("expected 100 to be 99"));
        assert!(out.content.contains("charge credit card"));
    }
}
