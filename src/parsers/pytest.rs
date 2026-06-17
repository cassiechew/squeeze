//! Parser for `pytest` output (default verbose-ish format).

use crate::core::{CommandParser, CompressOptions, CompressedOutput, Strategy};
use crate::lossless::strip_ansi;
use crate::parsers::test_common::{TestFailure, TestReport, render};
use regex::Regex;
use std::sync::OnceLock;

pub struct PytestParser;

impl CommandParser for PytestParser {
    fn strategy(&self) -> Strategy {
        Strategy::Pytest
    }

    fn can_handle(&self, command: &str, output: &str) -> bool {
        if command_is_pytest(command) {
            return true;
        }
        output.contains("test session starts")
            || (output.contains(" PASSED") && output.contains(" FAILED"))
            || output.contains("=== FAILURES ===")
    }

    fn parse(&self, output: &str, opts: &CompressOptions) -> CompressedOutput {
        let clean = strip_ansi(output);
        let report = parse_report(&clean);
        render(
            &report,
            Strategy::Pytest,
            opts.stack_traces,
            opts.passing_tests,
        )
    }
}

fn command_is_pytest(command: &str) -> bool {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    for t in &tokens {
        if *t == "pytest" || *t == "py.test" {
            return true;
        }
    }
    // "python -m pytest"
    let joined = command.to_ascii_lowercase();
    joined.contains("-m pytest") || joined.contains("-mpytest")
}

fn parse_report(input: &str) -> TestReport {
    let mut report = TestReport::default();
    let mut in_failures = false;
    let mut in_short_summary = false;
    let mut current: Option<TestFailure> = None;

    static TEST_LINE: OnceLock<Regex> = OnceLock::new();
    let test_line = TEST_LINE.get_or_init(|| {
        // e.g. "tests/test_auth.py::test_login PASSED                      [ 1%]"
        Regex::new(r"^(\S+?::[^\s]+)\s+(PASSED|FAILED|ERROR|SKIPPED|XFAIL|XPASS)\b").unwrap()
    });
    static FAIL_SECTION_HEADER: OnceLock<Regex> = OnceLock::new();
    let fail_header = FAIL_SECTION_HEADER
        .get_or_init(|| Regex::new(r"^_+\s+(.+?)\s+_+$").unwrap());
    static SUMMARY_LINE: OnceLock<Regex> = OnceLock::new();
    let summary_line = SUMMARY_LINE
        .get_or_init(|| {
            Regex::new(r"=+\s*(?:(\d+)\s+failed)?[, ]*(?:(\d+)\s+passed)?[, ]*(?:(\d+)\s+skipped)?[, ]*(?:(\d+)\s+error)?[^=]*in\s+([^\s=]+)\s*=+").unwrap()
        });

    for raw in input.lines() {
        let line = raw.trim_end();

        if line.contains("=== FAILURES ===") || line.contains("= FAILURES =") {
            in_failures = true;
            in_short_summary = false;
            continue;
        }
        if line.contains("short test summary info") {
            if let Some(f) = current.take() {
                report.failures.push(f);
            }
            in_failures = false;
            in_short_summary = true;
            continue;
        }
        if line.starts_with("=") && line.contains(" in ") {
            // End-of-session summary.
            if let Some(f) = current.take() {
                report.failures.push(f);
            }
            in_failures = false;
            in_short_summary = false;
            if let Some(caps) = summary_line.captures(line) {
                if let Some(f) = caps.get(1) {
                    report.total_failed = f.as_str().parse().unwrap_or(0);
                }
                if let Some(p) = caps.get(2) {
                    report.total_passed = p.as_str().parse().unwrap_or(report.total_passed);
                }
                if let Some(s) = caps.get(3) {
                    report.total_skipped = s.as_str().parse().unwrap_or(0);
                }
                if let Some(d) = caps.get(5) {
                    report.duration = Some(d.as_str().to_string());
                }
            }
            continue;
        }

        // Test result lines during the main run.
        if !in_failures && !in_short_summary {
            if let Some(caps) = test_line.captures(line) {
                let full = caps.get(1).unwrap().as_str();
                let verdict = caps.get(2).unwrap().as_str();
                let file = full.split("::").next().unwrap_or(full).to_string();
                match verdict {
                    "PASSED" | "XPASS" => {
                        *report.passed_per_file.entry(file.clone()).or_insert(0) += 1;
                        *report.total_per_file.entry(file).or_insert(0) += 1;
                        report.total_passed += 1;
                    }
                    "FAILED" | "ERROR" => {
                        *report.total_per_file.entry(file).or_insert(0) += 1;
                    }
                    "SKIPPED" | "XFAIL" => {
                        report.total_skipped += 1;
                    }
                    _ => {}
                }
                continue;
            }
        }

        // Inside the FAILURES section.
        if in_failures {
            if let Some(caps) = fail_header.captures(line) {
                if let Some(f) = current.take() {
                    report.failures.push(f);
                }
                let test = caps.get(1).unwrap().as_str().trim().to_string();
                current = Some(TestFailure {
                    file: String::new(),
                    test,
                    messages: Vec::new(),
                    stack: Vec::new(),
                });
                continue;
            }
            if let Some(f) = current.as_mut() {
                // File:line trailing reference: "tests/test_pay.py:15: AssertionError"
                if let Some(pathlike) = line.split(':').next() {
                    if pathlike.ends_with(".py") && f.file.is_empty() {
                        f.file = pathlike.to_string();
                    }
                }
                if is_stack_like(line) {
                    f.stack.push(line.to_string());
                } else if !line.trim().is_empty() {
                    f.messages.push(line.to_string());
                }
            }
            continue;
        }

        // Short summary lines are useful but redundant with our own output.
    }
    if let Some(f) = current {
        report.failures.push(f);
    }
    // Backfill unknown files so the rendered output doesn't show empty names.
    for f in &mut report.failures {
        if f.file.is_empty() {
            f.file = "(unknown)".to_string();
        }
    }
    report
}

fn is_stack_like(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("at ") || (t.contains(".py:") && t.starts_with("File \""))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HAPPY_PATH: &str = "\
============================= test session starts ==============================
collected 3 items

tests/test_auth.py::test_login PASSED                                    [ 33%]
tests/test_auth.py::test_logout PASSED                                   [ 66%]
tests/test_api.py::test_ping PASSED                                      [100%]

============================== 3 passed in 0.42s ===============================
";

    const FAILING: &str = "\
============================= test session starts ==============================
collected 3 items

tests/test_auth.py::test_login PASSED                                    [ 33%]
tests/test_pay.py::test_charge FAILED                                    [ 66%]
tests/test_api.py::test_ping PASSED                                      [100%]

=================================== FAILURES ===================================
_______________________________ test_charge ____________________________________

    def test_charge():
>       assert pay(100) == 100
E       assert 99 == 100

tests/test_pay.py:15: AssertionError
=========================== short test summary info ============================
FAILED tests/test_pay.py::test_charge - assert 99 == 100
========================= 1 failed, 2 passed in 1.23s ==========================
";

    #[test]
    fn happy_path_all_passed() {
        let p = PytestParser;
        let out = p.parse(HAPPY_PATH, &CompressOptions::default());
        assert!(out.content.starts_with("3 passed"));
        assert!(out.content.contains("tests/test_auth.py (2)"));
        assert!(out.content.contains("tests/test_api.py (1)"));
    }

    #[test]
    fn failing_shows_assertion() {
        let p = PytestParser;
        let out = p.parse(FAILING, &CompressOptions::default());
        assert!(out.content.starts_with("1 failed, 2 passed"));
        assert!(out.content.contains("test_charge"));
        assert!(out.content.contains("assert 99 == 100"));
        assert!(out.content.contains("tests/test_pay.py"));
    }

    #[test]
    fn empty_input_is_safe() {
        let p = PytestParser;
        let out = p.parse("", &CompressOptions::default());
        assert!(out.content.starts_with("0 tests"));
    }

    #[test]
    fn detects_command() {
        let p = PytestParser;
        assert!(p.can_handle("pytest", ""));
        assert!(p.can_handle("pytest -xvs", ""));
        assert!(p.can_handle("python -m pytest tests/", ""));
    }

    #[test]
    fn lossless_assertion_preserved() {
        let p = PytestParser;
        let out = p.parse(FAILING, &CompressOptions::default());
        for s in ["test_charge", "assert 99 == 100", "tests/test_pay.py"] {
            assert!(out.content.contains(s), "missing: {s}");
        }
    }
}
