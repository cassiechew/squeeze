//! Parser for `go test` output in both text and NDJSON (`go test -json`)
//! formats. Detection is permissive: even mixed streams (stderr build
//! errors interleaved with NDJSON) are handled by routing JSON lines
//! through the NDJSON parser and ignoring the rest.
//!
//! NDJSON event types understood: `run`, `output`, `pass`, `fail`, `skip`,
//! `bench`, `bail`, `pause`, `cont`, `start`. Package-level events
//! (without a `Test` field) surface as synthetic `(package)` entries so
//! compile errors or early-abort panics are preserved in the report
//! instead of being silently swallowed.

use crate::core::{CommandParser, CompressOptions, CompressedOutput, Strategy};
use crate::lossless::strip_ansi;
use crate::parsers::test_common::{TestFailure, TestReport, render};
use serde_json::Value;
use std::collections::HashMap;

pub struct GoTestParser;

impl CommandParser for GoTestParser {
    fn strategy(&self) -> Strategy {
        Strategy::GoTest
    }

    fn can_handle(&self, command: &str, output: &str) -> bool {
        if command_is_go_test(command) {
            return true;
        }
        output.contains("--- PASS: ")
            || output.contains("--- FAIL: ")
            || (output.contains("\"Action\"") && output.contains("\"Test\""))
    }

    fn parse(&self, output: &str, opts: &CompressOptions) -> CompressedOutput {
        let clean = strip_ansi(output);
        let report = if is_ndjson(&clean) {
            parse_ndjson(&clean)
        } else {
            parse_text(&clean)
        };
        render(
            &report,
            Strategy::GoTest,
            opts.stack_traces,
            opts.passing_tests,
        )
    }
}

fn command_is_go_test(command: &str) -> bool {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    let mut seen_go = false;
    for t in tokens {
        if t == "go" {
            seen_go = true;
            continue;
        }
        if seen_go {
            if t.starts_with('-') {
                continue;
            }
            return t == "test";
        }
    }
    false
}

/// Detect NDJSON shape. Tolerates interleaved non-JSON lines (stderr build
/// errors, warnings, compile messages) - if we see at least 2 JSON event
/// lines and they make up at least 30% of non-empty lines in the first 40,
/// we route through the NDJSON parser.
fn is_ndjson(output: &str) -> bool {
    let mut json_lines = 0usize;
    let mut non_empty = 0usize;
    for l in output.lines().take(40) {
        let t = l.trim();
        if t.is_empty() {
            continue;
        }
        non_empty += 1;
        if t.starts_with('{') && t.ends_with('}') && t.contains("\"Action\"") {
            json_lines += 1;
        }
    }
    json_lines >= 2 && json_lines * 10 >= non_empty * 3
}

fn parse_text(input: &str) -> TestReport {
    let mut report = TestReport::default();
    // Messages accumulate between `=== RUN Test` and `--- (PASS|FAIL|SKIP): Test`
    // because go emits assertion output BEFORE the verdict line.
    let mut running_messages: Vec<String> = Vec::new();
    let mut running_stack: Vec<String> = Vec::new();
    // Tests that have finished but whose package footer hasn't appeared yet.
    let mut pending_passed: usize = 0;
    let mut pending_failures: Vec<TestFailure> = Vec::new();

    for raw in input.lines() {
        let line = raw.trim_end();

        if line.starts_with("=== RUN ")
            || line.starts_with("=== PAUSE ")
            || line.starts_with("=== CONT ")
            || line.starts_with("=== NAME ")
        {
            // Starting a new test - drop any lingering scratch from the previous.
            if line.starts_with("=== RUN ") {
                running_messages.clear();
                running_stack.clear();
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix("--- FAIL: ") {
            if let Some((name, _dur)) = split_test_result(rest) {
                pending_failures.push(TestFailure {
                    file: String::new(), // filled in at the package footer
                    test: name.to_string(),
                    messages: std::mem::take(&mut running_messages),
                    stack: std::mem::take(&mut running_stack),
                });
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("--- PASS: ") {
            if split_test_result(rest).is_some() {
                pending_passed += 1;
            }
            running_messages.clear();
            running_stack.clear();
            continue;
        }
        if let Some(rest) = line.strip_prefix("--- SKIP: ") {
            if split_test_result(rest).is_some() {
                report.total_skipped += 1;
            }
            running_messages.clear();
            running_stack.clear();
            continue;
        }

        if let Some((verdict, pkg, dur)) = parse_package_footer(line) {
            let failure_count = pending_failures.len();
            for mut f in pending_failures.drain(..) {
                f.file = pkg.clone();
                report.failures.push(f);
            }
            if pending_passed > 0 {
                *report.passed_per_file.entry(pkg.clone()).or_insert(0) += pending_passed;
            }
            let total_for_pkg = pending_passed + failure_count;
            if total_for_pkg > 0 {
                *report.total_per_file.entry(pkg.clone()).or_insert(0) += total_for_pkg;
            }
            report.total_passed += pending_passed;
            report.total_failed += failure_count;
            pending_passed = 0;
            if verdict == "ok" || verdict == "FAIL" {
                if report.duration.is_none() {
                    report.duration = Some(dur.to_string());
                }
            }
            continue;
        }

        if line == "FAIL" || line == "PASS" || line == "ok" {
            continue;
        }

        // Otherwise we're in the middle of a running test's output.
        let t = line.trim_start();
        if t.is_empty() {
            continue;
        }
        if is_stack_frame_go(t) {
            running_stack.push(t.to_string());
        } else {
            running_messages.push(t.to_string());
        }
    }

    // Flush any tests that never saw a package footer.
    let orphan_failure_count = pending_failures.len();
    for mut f in pending_failures {
        if f.file.is_empty() {
            f.file = "(unknown)".to_string();
        }
        report.failures.push(f);
    }
    if pending_passed > 0 {
        *report
            .passed_per_file
            .entry("(unknown)".to_string())
            .or_insert(0) += pending_passed;
    }
    let orphan_total = pending_passed + orphan_failure_count;
    if orphan_total > 0 {
        *report
            .total_per_file
            .entry("(unknown)".to_string())
            .or_insert(0) += orphan_total;
    }
    report.total_passed += pending_passed;
    report.total_failed += orphan_failure_count;
    report
}

fn split_test_result(rest: &str) -> Option<(&str, &str)> {
    let paren = rest.rfind(" (")?;
    let name = &rest[..paren];
    let dur = rest[paren + 2..].trim_end_matches(')');
    Some((name, dur))
}

fn parse_package_footer(line: &str) -> Option<(&str, String, &str)> {
    // Possible prefixes separated by whitespace.
    let verdict_fail = line.starts_with("FAIL\t") || line.starts_with("FAIL ");
    let verdict_ok = line.starts_with("ok  \t") || line.starts_with("ok\t") || line.starts_with("ok  ");
    if !(verdict_fail || verdict_ok) {
        return None;
    }
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }
    let verdict = if parts[0] == "FAIL" { "FAIL" } else { "ok" };
    let pkg = parts[1].to_string();
    let dur = parts[2];
    Some((verdict, pkg, dur))
}

fn is_stack_frame_go(line: &str) -> bool {
    // goroutine 1 [running]:
    // main.(*T).Foo(...)
    //   /path/to/file.go:42 +0x12
    line.starts_with("goroutine ")
        || line.contains(".go:") && line.starts_with('/')
        || line.ends_with("(...)")
}

// --- NDJSON handling ---------------------------------------------------------

/// Parse `go test -json` output via serde_json. Malformed lines and
/// non-JSON interleaved stderr are tolerated and skipped. Package-level
/// failures (compile errors, main-goroutine panics) surface as synthetic
/// `(package)` test entries so the information isn't silently dropped.
fn parse_ndjson(input: &str) -> TestReport {
    let mut report = TestReport::default();
    let mut output_by_test: HashMap<String, Vec<String>> = HashMap::new();
    let mut package_output: HashMap<String, Vec<String>> = HashMap::new();
    let mut package_duration: Option<f64> = None;

    for raw in input.lines() {
        let line = raw.trim();
        if !line.starts_with('{') || !line.ends_with('}') {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            // Malformed JSON line - skip, don't abort the run.
            continue;
        };
        let action = v
            .get("Action")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if action.is_empty() {
            continue;
        }
        let package = v
            .get("Package")
            .and_then(Value::as_str)
            .unwrap_or("(unknown)")
            .to_string();
        let test = v.get("Test").and_then(Value::as_str).map(String::from);
        let output_text = v
            .get("Output")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let elapsed = v.get("Elapsed").and_then(Value::as_f64);

        match action.as_str() {
            "output" => {
                if let Some(t) = &test {
                    output_by_test
                        .entry(format!("{package}::{t}"))
                        .or_default()
                        .push(output_text);
                } else {
                    package_output
                        .entry(package.clone())
                        .or_default()
                        .push(output_text);
                }
            }
            "pass" => {
                if let Some(t) = test {
                    output_by_test.remove(&format!("{package}::{t}"));
                    report.total_passed += 1;
                    *report.passed_per_file.entry(package.clone()).or_insert(0) += 1;
                    *report.total_per_file.entry(package).or_insert(0) += 1;
                } else {
                    // Package-level pass - discard buffered output, record duration.
                    package_output.remove(&package);
                    if let Some(e) = elapsed {
                        if package_duration.is_none() {
                            package_duration = Some(e);
                        }
                    }
                }
            }
            "fail" => {
                if let Some(t) = test {
                    let key = format!("{package}::{t}");
                    let raw_msgs = output_by_test.remove(&key).unwrap_or_default();
                    let messages = clean_output_messages(&raw_msgs);
                    report.failures.push(TestFailure {
                        file: package.clone(),
                        test: t,
                        messages,
                        stack: Vec::new(),
                    });
                    report.total_failed += 1;
                    *report.total_per_file.entry(package).or_insert(0) += 1;
                } else {
                    // Package-level failure. Could be a compile error, a
                    // panic outside any test, or a test binary bailing
                    // before individual test events reached us. Surface it
                    // as a synthetic failure so the agent sees the output.
                    let raw_msgs = package_output.remove(&package).unwrap_or_default();
                    let messages = clean_output_messages(&raw_msgs);
                    if !messages.is_empty() {
                        report.failures.push(TestFailure {
                            file: package.clone(),
                            test: "(package)".to_string(),
                            messages,
                            stack: Vec::new(),
                        });
                        report.total_failed += 1;
                        *report.total_per_file.entry(package).or_insert(0) += 1;
                    }
                }
            }
            "skip" => {
                if let Some(t) = &test {
                    output_by_test.remove(&format!("{package}::{t}"));
                    report.total_skipped += 1;
                } else {
                    package_output.remove(&package);
                }
            }
            "bench" => {
                // Benchmark completion event - count as passing for accounting.
                if let Some(t) = test {
                    output_by_test.remove(&format!("{package}::{t}"));
                    report.total_passed += 1;
                    *report.passed_per_file.entry(package.clone()).or_insert(0) += 1;
                    *report.total_per_file.entry(package).or_insert(0) += 1;
                }
            }
            "bail" => {
                // Test binary terminated early. Attribute any buffered
                // output so we don't lose context.
                let (test_label, raw_msgs) = if let Some(t) = test {
                    let key = format!("{package}::{t}");
                    (t, output_by_test.remove(&key).unwrap_or_default())
                } else {
                    (
                        "(bail)".to_string(),
                        package_output.remove(&package).unwrap_or_default(),
                    )
                };
                report.failures.push(TestFailure {
                    file: package.clone(),
                    test: test_label,
                    messages: clean_output_messages(&raw_msgs),
                    stack: Vec::new(),
                });
                report.total_failed += 1;
                *report.total_per_file.entry(package).or_insert(0) += 1;
            }
            // No report-level effect: run, start, pause, cont.
            _ => {}
        }
    }

    if let Some(d) = package_duration {
        report.duration = Some(format!("{d:.3}s"));
    }
    report
}

/// Strip boilerplate that `go test -json` echoes back inside `Output`
/// events (e.g. the `=== RUN`, `--- FAIL`, `FAIL\t...` lines go tool
/// test2json emits alongside structured events) and drop blank lines.
fn clean_output_messages(raw: &[String]) -> Vec<String> {
    raw.iter()
        .flat_map(|s| s.lines().map(String::from).collect::<Vec<_>>())
        .filter(|l| {
            let t = l.trim();
            if t.is_empty() {
                return false;
            }
            !(t.starts_with("=== RUN")
                || t.starts_with("=== PAUSE")
                || t.starts_with("=== CONT")
                || t.starts_with("=== NAME")
                || t.starts_with("--- PASS")
                || t.starts_with("--- FAIL")
                || t.starts_with("--- SKIP")
                || t == "PASS"
                || t == "FAIL"
                || t.starts_with("FAIL\t")
                || t.starts_with("ok  \t")
                || t.starts_with("ok\t"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HAPPY_PATH: &str = "\
=== RUN   TestFoo
--- PASS: TestFoo (0.00s)
=== RUN   TestBar
--- PASS: TestBar (0.00s)
PASS
ok  \texample.com/pkg\t0.010s
";

    const FAILING_INPUT: &str = "\
=== RUN   TestFoo
--- PASS: TestFoo (0.00s)
=== RUN   TestBar
    bar_test.go:42: expected 5, got 10
--- FAIL: TestBar (0.00s)
=== RUN   TestBaz
--- PASS: TestBaz (0.00s)
FAIL
FAIL\texample.com/pkg\t0.010s
";

    #[test]
    fn happy_path_passes_tracked() {
        let p = GoTestParser;
        let out = p.parse(HAPPY_PATH, &CompressOptions::default());
        assert!(out.content.starts_with("2 passed"));
        assert!(out.content.contains("example.com/pkg (2)"));
    }

    #[test]
    fn failing_case_groups_by_package() {
        let p = GoTestParser;
        let out = p.parse(FAILING_INPUT, &CompressOptions::default());
        assert!(out.content.starts_with("1 failed, 2 passed"));
        assert!(out.content.contains("example.com/pkg"));
        assert!(out.content.contains("TestBar"));
        assert!(out.content.contains("expected 5, got 10"));
    }

    #[test]
    fn ndjson_detection_and_parse() {
        let p = GoTestParser;
        let input = r#"{"Time":"t","Action":"run","Package":"p","Test":"TestFoo"}
{"Time":"t","Action":"output","Package":"p","Test":"TestFoo","Output":"--- PASS: TestFoo (0.00s)\n"}
{"Time":"t","Action":"pass","Package":"p","Test":"TestFoo","Elapsed":0}
{"Time":"t","Action":"run","Package":"p","Test":"TestBar"}
{"Time":"t","Action":"output","Package":"p","Test":"TestBar","Output":"    bar_test.go:42: boom\n"}
{"Time":"t","Action":"fail","Package":"p","Test":"TestBar","Elapsed":0}
"#;
        let out = p.parse(input, &CompressOptions::default());
        assert!(out.content.starts_with("1 failed, 1 passed"));
        assert!(out.content.contains("TestBar"));
        assert!(out.content.contains("bar_test.go:42: boom"));
    }

    #[test]
    fn empty_input_is_safe() {
        let p = GoTestParser;
        let out = p.parse("", &CompressOptions::default());
        assert!(out.content.starts_with("0 tests"));
    }

    #[test]
    fn detects_command() {
        let p = GoTestParser;
        assert!(p.can_handle("go test ./...", ""));
        assert!(p.can_handle("go test -v -race", ""));
        assert!(!p.can_handle("go build", ""));
    }

    #[test]
    fn lossless_assertion_preserved() {
        let p = GoTestParser;
        let out = p.parse(FAILING_INPUT, &CompressOptions::default());
        assert!(out.content.contains("expected 5, got 10"));
        assert!(out.content.contains("TestBar"));
    }

    // --- NDJSON robustness tests --------------------------------------------

    #[test]
    fn ndjson_subtests_grouped_under_parent_package() {
        let p = GoTestParser;
        // When a subtest fails, go emits a `fail` event for the subtest AND
        // a `fail` event for the parent. Both are surfaced; that's faithful
        // to the source but means 2 failures for what is logically one.
        let input = r#"{"Action":"run","Package":"p","Test":"TestParent"}
{"Action":"run","Package":"p","Test":"TestParent/case_a"}
{"Action":"output","Package":"p","Test":"TestParent/case_a","Output":"    sub_test.go:10: nope\n"}
{"Action":"fail","Package":"p","Test":"TestParent/case_a","Elapsed":0.01}
{"Action":"run","Package":"p","Test":"TestParent/case_b"}
{"Action":"pass","Package":"p","Test":"TestParent/case_b","Elapsed":0.01}
{"Action":"fail","Package":"p","Test":"TestParent","Elapsed":0.02}
{"Action":"fail","Package":"p","Elapsed":0.05}
"#;
        let out = p.parse(input, &CompressOptions::default());
        assert!(out.content.starts_with("2 failed, 1 passed"), "actual: {}", out.content);
        assert!(out.content.contains("TestParent/case_a"));
        assert!(out.content.contains("TestParent"));
        assert!(out.content.contains("sub_test.go:10: nope"));
        // Subtest-level pass is collapsed into the per-package count.
        assert!(out.content.contains("p (1/3)"), "expected mixed count; actual: {}", out.content);
    }

    #[test]
    fn ndjson_package_level_failure_surfaces_as_synthetic_entry() {
        let p = GoTestParser;
        // Compile error: no Test events, just package output + fail.
        // Uses r##"..."## delimiter because the payload contains "# which
        // would otherwise close a single-hash raw string.
        let input = r##"{"Action":"output","Package":"p","Output":"# example.com/p\n"}
{"Action":"output","Package":"p","Output":"./foo.go:5:6: undefined: Bar\n"}
{"Action":"fail","Package":"p","Elapsed":0}
"##;
        let out = p.parse(input, &CompressOptions::default());
        assert!(out.content.contains("1 failed"));
        assert!(out.content.contains("(package)"));
        assert!(out.content.contains("undefined: Bar"));
    }

    #[test]
    fn ndjson_tolerates_malformed_and_interleaved_lines() {
        let p = GoTestParser;
        let input = r#"go: downloading example.com/foo v1.2.3
{"Action":"run","Package":"p","Test":"TestOK"}
{this is not valid json}
{"Action":"pass","Package":"p","Test":"TestOK","Elapsed":0}
build warning: something
{"Action":"pass","Package":"p","Elapsed":0.01}
"#;
        let out = p.parse(input, &CompressOptions::default());
        assert!(out.content.starts_with("1 passed"), "actual: {}", out.content);
    }

    #[test]
    fn ndjson_handles_unicode_in_test_names_and_messages() {
        let p = GoTestParser;
        let input = r#"{"Action":"run","Package":"p","Test":"TestΩmega"}
{"Action":"output","Package":"p","Test":"TestΩmega","Output":"got: é expected: é\n"}
{"Action":"fail","Package":"p","Test":"TestΩmega","Elapsed":0}
{"Action":"fail","Package":"p","Elapsed":0}
"#;
        let out = p.parse(input, &CompressOptions::default());
        assert!(out.content.contains("TestΩmega"), "actual: {}", out.content);
        // é should be decoded to é via serde_json.
        assert!(out.content.contains("got: é expected: é"));
    }

    #[test]
    fn ndjson_handles_benchmark_events() {
        let p = GoTestParser;
        let input = r#"{"Action":"run","Package":"p","Test":"BenchmarkFoo"}
{"Action":"output","Package":"p","Test":"BenchmarkFoo","Output":"BenchmarkFoo-8   1000  1234 ns/op\n"}
{"Action":"bench","Package":"p","Test":"BenchmarkFoo"}
{"Action":"pass","Package":"p","Elapsed":1.5}
"#;
        let out = p.parse(input, &CompressOptions::default());
        assert!(out.content.starts_with("1 passed"), "actual: {}", out.content);
    }

    #[test]
    fn ndjson_empty_input_safe() {
        let p = GoTestParser;
        let out = p.parse("", &CompressOptions::default());
        assert!(out.content.starts_with("0 tests"));
    }

    #[test]
    fn ndjson_only_run_events_without_verdict_yields_zero_report() {
        let p = GoTestParser;
        // Stream cut off mid-run: no pass/fail events arrived.
        let input = r#"{"Action":"run","Package":"p","Test":"TestFoo"}
{"Action":"output","Package":"p","Test":"TestFoo","Output":"running...\n"}
"#;
        let out = p.parse(input, &CompressOptions::default());
        assert!(out.content.starts_with("0 tests"));
    }

    #[test]
    fn ndjson_detection_ignores_spurious_brace_lines() {
        // A single `{` line shouldn't flip detection on.
        assert!(!is_ndjson("{\n--- PASS: X (0.0s)\nok pkg 0.01s\n"));
        // But proper NDJSON should detect.
        let proper = r#"{"Action":"run","Package":"p","Test":"T"}
{"Action":"pass","Package":"p","Test":"T","Elapsed":0}
"#;
        assert!(is_ndjson(proper));
    }

    #[test]
    fn ndjson_elapsed_duration_recorded() {
        let p = GoTestParser;
        let input = r#"{"Action":"run","Package":"p","Test":"T"}
{"Action":"pass","Package":"p","Test":"T","Elapsed":0.01}
{"Action":"pass","Package":"p","Elapsed":1.234}
"#;
        let out = p.parse(input, &CompressOptions::default());
        // Summary line format is "N passed (duration)".
        assert!(out.content.contains("1.234s"), "actual: {}", out.content);
    }

    #[test]
    fn ndjson_skip_event_counts_as_skipped() {
        let p = GoTestParser;
        let input = r#"{"Action":"run","Package":"p","Test":"TestA"}
{"Action":"pass","Package":"p","Test":"TestA","Elapsed":0}
{"Action":"run","Package":"p","Test":"TestB"}
{"Action":"skip","Package":"p","Test":"TestB","Elapsed":0}
{"Action":"pass","Package":"p","Elapsed":0}
"#;
        let out = p.parse(input, &CompressOptions::default());
        assert!(out.content.contains("1 skipped"), "actual: {}", out.content);
        assert!(out.content.contains("1 passed"));
    }
}
