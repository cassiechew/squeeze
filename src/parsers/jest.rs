//! Parser for Jest text output.

use crate::core::{CommandParser, CompressOptions, CompressedOutput, Strategy};
use crate::lossless::strip_ansi;
use crate::parsers::test_common::{TestFailure, TestReport, render};

pub struct JestParser;

impl CommandParser for JestParser {
    fn strategy(&self) -> Strategy {
        Strategy::Jest
    }

    fn can_handle(&self, command: &str, output: &str) -> bool {
        if command_is_jest(command) {
            return true;
        }
        // Jest's distinctive markers. Restrict so Vitest doesn't match too.
        output.contains("Test Suites:") && output.contains("Tests:")
    }

    fn parse(&self, output: &str, opts: &CompressOptions) -> CompressedOutput {
        let clean = strip_ansi(output);
        let report = parse_report(&clean);
        render(
            &report,
            Strategy::Jest,
            opts.stack_traces,
            opts.passing_tests,
        )
    }
}

fn command_is_jest(command: &str) -> bool {
    command.split_whitespace().any(|t| t == "jest")
}

fn parse_report(input: &str) -> TestReport {
    let mut report = TestReport::default();
    let mut current_file: Option<String> = None;
    let mut current_failure: Option<TestFailure> = None;
    let mut in_failure_detail = false;

    for raw in input.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();

        // `PASS src/foo.test.ts` or `FAIL src/pay.test.ts`.
        if let Some(rest) = trimmed.strip_prefix("PASS ") {
            current_file = Some(rest.trim().to_string());
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("FAIL ") {
            current_file = Some(rest.trim().to_string());
            continue;
        }

        // Bullet-prefixed failure section header: `● describe > test name`
        if let Some(rest) = trimmed.strip_prefix("● ") {
            if let Some(f) = current_failure.take() {
                report.failures.push(f);
            }
            let file = current_file.clone().unwrap_or_else(|| "(unknown)".to_string());
            current_failure = Some(TestFailure {
                file,
                test: rest.to_string(),
                messages: Vec::new(),
                stack: Vec::new(),
            });
            in_failure_detail = true;
            continue;
        }

        // Indented ✓ and ✗ lines for individual tests within a file header.
        if let Some(rest) = strip_mark(trimmed, '✓') {
            if let Some(file) = &current_file {
                *report.passed_per_file.entry(file.clone()).or_insert(0) += 1;
                *report.total_per_file.entry(file.clone()).or_insert(0) += 1;
                report.total_passed += 1;
            }
            // Record name is irrelevant because passing names are collapsed.
            let _ = rest;
            continue;
        }
        if let Some(rest) = strip_any_mark(trimmed, &['✗', '×']) {
            // Individual test failure line inside a file block.
            if let Some(f) = current_failure.take() {
                report.failures.push(f);
            }
            let file = current_file.clone().unwrap_or_else(|| "(unknown)".to_string());
            *report.total_per_file.entry(file.clone()).or_insert(0) += 1;
            report.total_failed += 1;
            current_failure = Some(TestFailure {
                file,
                test: rest.split(" (").next().unwrap_or(rest).to_string(),
                messages: Vec::new(),
                stack: Vec::new(),
            });
            in_failure_detail = false;
            continue;
        }

        // Summary line.
        if let Some(rest) = trimmed.strip_prefix("Tests:") {
            parse_totals_line(rest, &mut report);
            in_failure_detail = false;
            if let Some(f) = current_failure.take() {
                report.failures.push(f);
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("Time:") {
            report.duration = Some(rest.trim().to_string());
            continue;
        }
        if trimmed.starts_with("Test Suites:") || trimmed.starts_with("Ran all test suites") {
            continue;
        }

        // Inside a failure detail block, collect assertion messages and stack.
        if in_failure_detail {
            if let Some(f) = current_failure.as_mut() {
                if trimmed.is_empty() {
                    continue;
                }
                if is_stack_frame(trimmed) {
                    f.stack.push(trimmed.to_string());
                } else {
                    f.messages.push(trimmed.to_string());
                }
            }
        }
    }
    if let Some(f) = current_failure {
        report.failures.push(f);
    }
    report
}

fn strip_mark(s: &str, mark: char) -> Option<&str> {
    let mut chars = s.chars();
    let first = chars.next()?;
    if first == mark {
        Some(chars.as_str().trim_start())
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

fn parse_totals_line(rest: &str, report: &mut TestReport) {
    // `Tests:       1 failed, 11 passed, 12 total`
    let text = rest.trim_start_matches(':').trim();
    for part in text.split(',') {
        let p = part.trim();
        if let Some(n) = pair(p, "failed") {
            report.total_failed = n;
        } else if let Some(n) = pair(p, "passed") {
            report.total_passed = n;
        } else if let Some(n) = pair(p, "skipped") {
            report.total_skipped = n;
        }
    }
}

fn pair(s: &str, keyword: &str) -> Option<usize> {
    let idx = s.find(keyword)?;
    let prefix = s[..idx].trim();
    prefix.parse().ok()
}

fn is_stack_frame(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("at ")
        || (t.starts_with('/') && t.contains(':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HAPPY_PATH: &str = "\
 PASS  src/auth.test.ts
 PASS  src/api.test.ts

Test Suites: 2 passed, 2 total
Tests:       9 passed, 9 total
Snapshots:   0 total
Time:        0.5 s
";

    const FAILING: &str = "\
 PASS  src/auth.test.ts
 FAIL  src/pay.test.ts
  payment
    ✓ should authorize (5ms)
    ✗ should charge (2ms)

  ● payment > should charge

    expect(received).toBe(expected)

    Expected: 100
    Received: 99

      at Object.<anonymous> (src/pay.test.ts:15:19)

Test Suites: 1 failed, 1 passed, 2 total
Tests:       1 failed, 1 passed, 2 total
Snapshots:   0 total
Time:        1.1 s
";

    #[test]
    fn happy_path() {
        let p = JestParser;
        let out = p.parse(HAPPY_PATH, &CompressOptions::default());
        assert!(out.content.starts_with("9 passed"));
    }

    #[test]
    fn failing_case() {
        let p = JestParser;
        let out = p.parse(FAILING, &CompressOptions::default());
        assert!(out.content.starts_with("1 failed, 1 passed"));
        assert!(out.content.contains("should charge"));
        assert!(out.content.contains("Expected: 100"));
        assert!(out.content.contains("Received: 99"));
    }

    #[test]
    fn empty_input_safe() {
        let p = JestParser;
        let out = p.parse("", &CompressOptions::default());
        assert!(out.content.starts_with("0 tests"));
    }

    #[test]
    fn detects_command() {
        let p = JestParser;
        assert!(p.can_handle("jest", ""));
        assert!(p.can_handle("npx jest --coverage", ""));
    }

    #[test]
    fn lossless_assertion_preserved() {
        let p = JestParser;
        let out = p.parse(FAILING, &CompressOptions::default());
        for s in ["should charge", "Expected: 100", "Received: 99"] {
            assert!(out.content.contains(s), "missing: {s}");
        }
    }
}
