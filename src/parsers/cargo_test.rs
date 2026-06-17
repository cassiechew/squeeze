//! Parser for `cargo test` output.
//!
//! Detection: presence of `running N tests` and `test result:`, or the
//! command contains `cargo` + `test`.

use crate::core::{CommandParser, CompressOptions, CompressedOutput, Strategy};
use crate::lossless::strip_ansi;
use crate::parsers::test_common::{TestFailure, TestReport, derive_group_key, render};

pub struct CargoTestParser;

impl CommandParser for CargoTestParser {
    fn strategy(&self) -> Strategy {
        Strategy::CargoTest
    }

    fn can_handle(&self, command: &str, output: &str) -> bool {
        if command_is_cargo_test(command) {
            return true;
        }
        output.contains("running ") && output.contains("test result:")
    }

    fn parse(&self, output: &str, opts: &CompressOptions) -> CompressedOutput {
        let clean = strip_ansi(output);
        let report = parse_report(&clean);
        render(
            &report,
            Strategy::CargoTest,
            opts.stack_traces,
            opts.passing_tests,
        )
    }
}

fn command_is_cargo_test(command: &str) -> bool {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    let mut seen_cargo = false;
    for t in tokens {
        if t == "cargo" {
            seen_cargo = true;
            continue;
        }
        if seen_cargo {
            if t.starts_with('-') {
                continue;
            }
            return t == "test" || t == "nextest";
        }
    }
    false
}

fn parse_report(input: &str) -> TestReport {
    let mut report = TestReport::default();
    let mut in_failures_output = false;
    let mut current_failure_name: Option<String> = None;
    let mut current_msgs: Vec<String> = Vec::new();
    let mut current_stack: Vec<String> = Vec::new();

    for raw in input.lines() {
        let line = raw.trim_end();

        // Result lines during the main run.
        if let Some(rest) = line.strip_prefix("test ") {
            // Examples:
            //  test foo::bar ... ok
            //  test foo::bar ... FAILED
            //  test foo::bar ... ignored
            if let Some((name, status)) = split_test_line(rest) {
                let group = derive_group_key(&name);
                match status {
                    Status::Ok => {
                        *report.passed_per_file.entry(group.clone()).or_insert(0) += 1;
                        *report.total_per_file.entry(group).or_insert(0) += 1;
                        report.total_passed += 1;
                    }
                    Status::Failed => {
                        *report.total_per_file.entry(group).or_insert(0) += 1;
                        report.total_failed += 1;
                        // The failure body is emitted later in a dedicated block.
                    }
                    Status::Ignored => {
                        report.total_skipped += 1;
                    }
                }
                continue;
            }
        }

        // Failure output block headers: `---- foo::bar stdout ----`.
        if let Some(rest) = line.strip_prefix("---- ") {
            if let Some(name) = rest.strip_suffix(" stdout ----") {
                flush_failure(
                    &mut report,
                    &mut current_failure_name,
                    &mut current_msgs,
                    &mut current_stack,
                );
                current_failure_name = Some(name.to_string());
                in_failures_output = true;
                continue;
            }
        }

        if line == "failures:" || line.starts_with("test result:") {
            // End of per-failure block or main summary.
            flush_failure(
                &mut report,
                &mut current_failure_name,
                &mut current_msgs,
                &mut current_stack,
            );
            in_failures_output = false;

            if let Some(dur) = line.strip_prefix("test result:") {
                parse_summary_extras(&mut report, dur);
            }
            continue;
        }

        if line.starts_with("running ") || line.is_empty() {
            continue;
        }

        if in_failures_output && current_failure_name.is_some() {
            // Heuristic: lines starting with digits + ": " are stack frames
            // (RUST_BACKTRACE). `note: run with` is also noise.
            if line.starts_with("note: run with") {
                continue;
            }
            if is_stack_frame(line) {
                current_stack.push(line.to_string());
            } else {
                current_msgs.push(line.to_string());
            }
        }
    }
    flush_failure(
        &mut report,
        &mut current_failure_name,
        &mut current_msgs,
        &mut current_stack,
    );
    report
}

fn flush_failure(
    report: &mut TestReport,
    name: &mut Option<String>,
    msgs: &mut Vec<String>,
    stack: &mut Vec<String>,
) {
    if let Some(n) = name.take() {
        let group = derive_group_key(&n);
        let short_name = n.rsplit("::").next().unwrap_or(&n).to_string();
        report.failures.push(TestFailure {
            file: group,
            test: short_name,
            messages: std::mem::take(msgs),
            stack: std::mem::take(stack),
        });
    } else {
        msgs.clear();
        stack.clear();
    }
}

enum Status {
    Ok,
    Failed,
    Ignored,
}

fn split_test_line(rest: &str) -> Option<(String, Status)> {
    // Rest looks like: "foo::bar ... ok" possibly with trailing "(ignored, reason)".
    let idx = rest.rfind(" ... ")?;
    let (name, suffix) = rest.split_at(idx);
    let suffix = suffix.trim_start_matches(" ... ").trim();
    let status = if suffix.starts_with("ok") {
        Status::Ok
    } else if suffix.starts_with("FAILED") {
        Status::Failed
    } else if suffix.starts_with("ignored") {
        Status::Ignored
    } else {
        return None;
    };
    Some((name.trim().to_string(), status))
}

fn is_stack_frame(line: &str) -> bool {
    let trimmed = line.trim_start();
    let mut chars = trimmed.chars();
    let first = chars.next();
    let second = chars.next();
    matches!(first, Some(c) if c.is_ascii_digit())
        && matches!(second, Some(c) if c.is_ascii_digit() || c == ':')
}

fn parse_summary_extras(report: &mut TestReport, line: &str) {
    // "FAILED. 2 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s"
    if let Some(finished) = line.find("finished in ") {
        let rest = &line[finished + "finished in ".len()..];
        let dur: String = rest.trim().trim_end_matches('.').to_string();
        report.duration = Some(dur);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Verbosity;

    const HAPPY_PATH_INPUT: &str = "\
running 3 tests
test lossless::tests::strip_ansi_removes_colour_codes ... ok
test lossless::tests::collapse_cr_keeps_final_segment ... ok
test lossless::tests::dedup_collapses_runs ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
";

    const FAILING_INPUT: &str = "\
running 3 tests
test lossless::tests::strip_ansi_removes_colour_codes ... ok
test lossless::tests::collapse_cr_keeps_final_segment ... FAILED
test lossless::tests::dedup_collapses_runs ... ok

failures:

---- lossless::tests::collapse_cr_keeps_final_segment stdout ----
thread 'lossless::tests::collapse_cr_keeps_final_segment' panicked at src/lossless.rs:42:5:
assertion `left == right` failed
  left: \"100% done\\n\"
 right: \"oops\\n\"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
   0: rust_begin_unwind
   1: core::panicking::panic_fmt

failures:
    lossless::tests::collapse_cr_keeps_final_segment

test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
";

    #[test]
    fn happy_path_all_passed() {
        let p = CargoTestParser;
        let out = p.parse(HAPPY_PATH_INPUT, &CompressOptions::default());
        assert!(out.content.starts_with("3 passed"));
        assert!(out.content.contains("PASSED:"));
        assert!(out.content.contains("lossless::tests (3)"));
        // Passing collapsed -> omitted note, not lossless.
        assert!(!out.lossless);
        assert!(out.omitted.as_deref().unwrap().contains("passing"));
    }

    #[test]
    fn failure_case_shows_assertion_message() {
        let p = CargoTestParser;
        let out = p.parse(FAILING_INPUT, &CompressOptions::default());
        assert!(out.content.starts_with("1 failed, 2 passed"));
        assert!(out.content.contains("FAILED:"));
        assert!(out.content.contains("collapse_cr_keeps_final_segment"));
        assert!(out.content.contains("assertion `left == right` failed"));
        assert!(out.content.contains("100% done"));
        // Stack traces omitted by default.
        assert!(!out.content.contains("rust_begin_unwind"));
        let note = out.omitted.unwrap();
        assert!(note.contains("stack traces omitted"));
    }

    #[test]
    fn stack_traces_flag_includes_frames() {
        let p = CargoTestParser;
        let opts = CompressOptions {
            stack_traces: true,
            ..Default::default()
        };
        let out = p.parse(FAILING_INPUT, &opts);
        assert!(out.content.contains("rust_begin_unwind"));
    }

    #[test]
    fn passing_hidden_suppresses_passed_section() {
        let p = CargoTestParser;
        let opts = CompressOptions {
            passing_tests: Verbosity::Hidden,
            ..Default::default()
        };
        let out = p.parse(HAPPY_PATH_INPUT, &opts);
        assert!(!out.content.contains("PASSED:"));
        assert!(out.omitted.as_deref().unwrap().contains("passing tests hidden"));
    }

    #[test]
    fn empty_input_yields_zero_report() {
        let p = CargoTestParser;
        let out = p.parse("", &CompressOptions::default());
        assert!(out.content.starts_with("0 tests"));
    }

    #[test]
    fn mixed_file_shows_fraction() {
        let p = CargoTestParser;
        let out = p.parse(FAILING_INPUT, &CompressOptions::default());
        // lossless::tests has 3 total, 2 passed -> (2/3).
        assert!(out.content.contains("lossless::tests (2/3)"), "{}", out.content);
    }

    #[test]
    fn detects_command() {
        let p = CargoTestParser;
        assert!(p.can_handle("cargo test", ""));
        assert!(p.can_handle("cargo test --lib", ""));
        assert!(p.can_handle("cargo nextest run", ""));
    }

    #[test]
    fn lossless_failure_content_preserved() {
        // All assertion-message words must be present in output.
        let p = CargoTestParser;
        let out = p.parse(FAILING_INPUT, &CompressOptions::default());
        for s in [
            "assertion `left == right` failed",
            "100% done",
            "oops",
            "collapse_cr_keeps_final_segment",
        ] {
            assert!(out.content.contains(s), "missing: {s}");
        }
    }
}
