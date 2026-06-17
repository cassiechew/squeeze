//! Parser for `git status` output. Handles both the default human-readable
//! format and `--porcelain`. Preserves every file name; never collapses
//! into "N files changed" without listing them.

use crate::core::{CommandParser, CompressOptions, CompressedOutput, Strategy};
use crate::lossless::strip_ansi;

pub struct GitStatusParser;

#[derive(Default)]
struct Groups {
    branch: Option<String>,
    tracking: Option<String>,
    staged: Vec<String>,
    modified: Vec<String>,
    deleted: Vec<String>,
    renamed: Vec<String>,
    untracked: Vec<String>,
    conflicted: Vec<String>,
}

impl Groups {
    fn any_files(&self) -> bool {
        !self.staged.is_empty()
            || !self.modified.is_empty()
            || !self.deleted.is_empty()
            || !self.renamed.is_empty()
            || !self.untracked.is_empty()
            || !self.conflicted.is_empty()
    }
}

impl CommandParser for GitStatusParser {
    fn strategy(&self) -> Strategy {
        Strategy::GitStatus
    }

    fn can_handle(&self, command: &str, output: &str) -> bool {
        if command_is_git_status(command) {
            return true;
        }
        // Output-based detection as a fallback when command is unavailable.
        output.starts_with("On branch ")
            || output.starts_with("HEAD detached")
            || output.contains("Changes to be committed:")
            || output.contains("Changes not staged for commit:")
            || output.contains("Untracked files:")
            || is_porcelain_shape(output)
    }

    fn parse(&self, output: &str, _opts: &CompressOptions) -> CompressedOutput {
        let clean = strip_ansi(output);
        let groups = if is_porcelain_shape(&clean) {
            parse_porcelain(&clean)
        } else {
            parse_human(&clean)
        };
        let content = render(&groups);
        CompressedOutput {
            content,
            strategy: Strategy::GitStatus,
            omitted: None,
            lossless: true,
        }
    }
}

fn command_is_git_status(command: &str) -> bool {
    // Permissive detection: `status` appearing as any bare token after `git`
    // is good enough. This correctly handles flag-with-value forms such as
    // `git -C path status` and `git -c color.ui=never status`.
    let mut saw_git = false;
    for t in command.split_whitespace() {
        if t == "git" {
            saw_git = true;
        } else if saw_git && t == "status" {
            return true;
        }
    }
    false
}

fn is_porcelain_shape(output: &str) -> bool {
    // Porcelain lines are `XY path` where XY is a two-char status code.
    // We take the first few non-empty lines and check.
    let mut saw_any = false;
    for line in output.lines().take(10) {
        if line.trim().is_empty() {
            continue;
        }
        saw_any = true;
        if line.len() < 4 {
            return false;
        }
        let bytes = line.as_bytes();
        let xy_ok = (bytes[0].is_ascii_alphabetic() || bytes[0] == b' ' || bytes[0] == b'?')
            && (bytes[1].is_ascii_alphabetic() || bytes[1] == b' ' || bytes[1] == b'?');
        let sep_ok = bytes[2] == b' ';
        if !(xy_ok && sep_ok) {
            return false;
        }
    }
    saw_any
}

fn parse_porcelain(output: &str) -> Groups {
    let mut g = Groups::default();
    for line in output.lines() {
        if line.len() < 4 {
            continue;
        }
        let (code, rest) = line.split_at(2);
        let path = rest.trim_start().to_string();
        let x = code.as_bytes()[0];
        let y = code.as_bytes()[1];
        match (x, y) {
            (b'?', b'?') => g.untracked.push(path),
            (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => g.conflicted.push(path),
            (b'R', _) => g.renamed.push(path),
            (_, b'D') => g.deleted.push(path),
            (b'D', _) => g.deleted.push(path),
            (_, b'M') => g.modified.push(path),
            (b'M', _) | (b'A', _) | (b'C', _) => g.staged.push(path),
            _ => g.modified.push(path),
        }
    }
    g
}

fn parse_human(output: &str) -> Groups {
    let mut g = Groups::default();
    let mut section: Option<&'static str> = None;

    for raw in output.lines() {
        let line = raw.trim_end();

        if let Some(branch) = line.strip_prefix("On branch ") {
            g.branch = Some(branch.to_string());
            continue;
        }
        if line.starts_with("HEAD detached") {
            g.branch = Some(line.to_string());
            continue;
        }
        if line.starts_with("Your branch ") {
            g.tracking = Some(line.to_string());
            continue;
        }
        if line.starts_with("Changes to be committed:") {
            section = Some("staged");
            continue;
        }
        if line.starts_with("Changes not staged for commit:") {
            section = Some("unstaged");
            continue;
        }
        if line.starts_with("Untracked files:") {
            section = Some("untracked");
            continue;
        }
        if line.starts_with("Unmerged paths:") {
            section = Some("conflicted");
            continue;
        }
        // Skip hint lines.
        let trimmed = line.trim_start();
        if trimmed.starts_with("(use \"git") || trimmed.starts_with("(") && trimmed.ends_with(")") {
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        // Summary lines we ignore — we have our own.
        if line.starts_with("nothing to commit")
            || line.starts_with("no changes added")
            || line.starts_with("All conflicts fixed")
        {
            continue;
        }

        // Actual file lines have a tab or multiple spaces of indent.
        match section {
            Some("staged") => {
                if let Some((kind, path)) = parse_human_file_line(trimmed) {
                    match kind {
                        "modified" => g.staged.push(path),
                        "new file" => g.staged.push(path),
                        "deleted" => g.deleted.push(path),
                        "renamed" => g.renamed.push(path),
                        "copied" => g.staged.push(path),
                        "typechange" => g.staged.push(path),
                        _ => g.staged.push(path),
                    }
                }
            }
            Some("unstaged") => {
                if let Some((kind, path)) = parse_human_file_line(trimmed) {
                    match kind {
                        "modified" => g.modified.push(path),
                        "deleted" => g.deleted.push(path),
                        "typechange" => g.modified.push(path),
                        _ => g.modified.push(path),
                    }
                }
            }
            Some("conflicted") => {
                if let Some((_, path)) = parse_human_file_line(trimmed) {
                    g.conflicted.push(path);
                } else {
                    g.conflicted.push(trimmed.to_string());
                }
            }
            Some("untracked") => {
                g.untracked.push(trimmed.to_string());
            }
            _ => {}
        }
    }
    g
}

/// Parse a line like `modified:   src/foo.rs` into ("modified", "src/foo.rs").
fn parse_human_file_line(line: &str) -> Option<(&'static str, String)> {
    let kinds = [
        "modified",
        "new file",
        "deleted",
        "renamed",
        "copied",
        "typechange",
        "both modified",
        "both added",
        "both deleted",
        "added by us",
        "added by them",
        "deleted by us",
        "deleted by them",
    ];
    for kind in kinds {
        let prefix = format!("{kind}:");
        if let Some(rest) = line.strip_prefix(&prefix) {
            return Some((kind_to_static(kind), rest.trim().to_string()));
        }
    }
    None
}

fn kind_to_static(kind: &str) -> &'static str {
    match kind {
        "modified" => "modified",
        "new file" => "new file",
        "deleted" => "deleted",
        "renamed" => "renamed",
        "copied" => "copied",
        "typechange" => "typechange",
        _ => "modified",
    }
}

fn render(g: &Groups) -> String {
    let mut out = String::new();
    if let Some(branch) = &g.branch {
        if let Some(track) = &g.tracking {
            out.push_str(&format!("branch: {branch} ({})\n", shorten_tracking(track)));
        } else {
            out.push_str(&format!("branch: {branch}\n"));
        }
    }
    if !g.any_files() {
        out.push_str("working tree clean\n");
        return out;
    }

    let mut sections: Vec<(&str, &Vec<String>)> = Vec::new();
    if !g.staged.is_empty() {
        sections.push(("Staged", &g.staged));
    }
    if !g.modified.is_empty() {
        sections.push(("Modified", &g.modified));
    }
    if !g.deleted.is_empty() {
        sections.push(("Deleted", &g.deleted));
    }
    if !g.renamed.is_empty() {
        sections.push(("Renamed", &g.renamed));
    }
    if !g.untracked.is_empty() {
        sections.push(("Untracked", &g.untracked));
    }
    if !g.conflicted.is_empty() {
        sections.push(("Conflicted", &g.conflicted));
    }

    for (i, (name, files)) in sections.iter().enumerate() {
        if i > 0 || g.branch.is_some() {
            out.push('\n');
        }
        out.push_str(&format!("{name} ({}):\n", files.len()));
        for f in files.iter() {
            out.push_str(&format!("  {f}\n"));
        }
    }
    out
}

/// Turn `Your branch is up to date with 'origin/main'.` into a compact form.
fn shorten_tracking(t: &str) -> String {
    if let Some(rest) = t.strip_prefix("Your branch is up to date with '") {
        if let Some(name) = rest.strip_suffix("'.") {
            return format!("-> {name} (up to date)");
        }
    }
    if let Some(rest) = t.strip_prefix("Your branch is ahead of '") {
        // e.g. "...' by 2 commits."
        return format!("-> {}", rest.trim_end_matches('.'));
    }
    if let Some(rest) = t.strip_prefix("Your branch is behind '") {
        return format!("-> {}", rest.trim_end_matches('.'));
    }
    t.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HAPPY_PATH_INPUT: &str = "\
On branch main
Your branch is up to date with 'origin/main'.

Changes to be committed:
  (use \"git restore --staged <file>...\" to unstage)
\tmodified:   src/auth.rs
\tnew file:   src/config.rs

Changes not staged for commit:
  (use \"git add <file>...\" to update what will be committed)
  (use \"git restore <file>...\" to discard changes in working directory)
\tmodified:   src/main.rs
\tmodified:   src/lib.rs
\tmodified:   tests/integration.rs

Untracked files:
  (use \"git add <file>...\" to include in what will be committed)
\t.env.local

no changes added to commit (use \"git add\" and/or \"git commit -a\")
";

    #[test]
    fn happy_path_groups_by_status() {
        let p = GitStatusParser;
        let out = p.parse(HAPPY_PATH_INPUT, &CompressOptions::default());
        assert!(out.content.contains("branch: main"));
        assert!(out.content.contains("Staged (2):"));
        assert!(out.content.contains("src/auth.rs"));
        assert!(out.content.contains("src/config.rs"));
        assert!(out.content.contains("Modified (3):"));
        assert!(out.content.contains("src/main.rs"));
        assert!(out.content.contains("Untracked (1):"));
        assert!(out.content.contains(".env.local"));
        assert!(out.lossless);
    }

    #[test]
    fn happy_path_strips_hint_lines() {
        let p = GitStatusParser;
        let out = p.parse(HAPPY_PATH_INPUT, &CompressOptions::default());
        assert!(!out.content.contains("(use \"git"));
    }

    #[test]
    fn clean_tree_reports_clean() {
        let p = GitStatusParser;
        let input = "On branch main\nYour branch is up to date with 'origin/main'.\n\nnothing to commit, working tree clean\n";
        let out = p.parse(input, &CompressOptions::default());
        assert!(out.content.contains("working tree clean"));
    }

    #[test]
    fn porcelain_format_parses() {
        let p = GitStatusParser;
        let input = "M  src/a.rs\nA  src/b.rs\n D src/c.rs\n?? .env\nUU conflict.rs\n";
        let out = p.parse(input, &CompressOptions::default());
        assert!(out.content.contains("Staged"));
        assert!(out.content.contains("src/a.rs"));
        assert!(out.content.contains("src/b.rs"));
        assert!(out.content.contains("Deleted"));
        assert!(out.content.contains("src/c.rs"));
        assert!(out.content.contains("Untracked"));
        assert!(out.content.contains(".env"));
        assert!(out.content.contains("Conflicted"));
        assert!(out.content.contains("conflict.rs"));
    }

    #[test]
    fn empty_input_does_not_panic() {
        let p = GitStatusParser;
        let out = p.parse("", &CompressOptions::default());
        assert!(out.lossless);
    }

    #[test]
    fn detects_command() {
        let p = GitStatusParser;
        assert!(p.can_handle("git status", ""));
        assert!(p.can_handle("git -C path status", ""));
        assert!(p.can_handle("git status --short", ""));
        assert!(!p.can_handle("git diff", ""));
    }

    #[test]
    fn detects_human_output_without_command() {
        let p = GitStatusParser;
        assert!(p.can_handle("", "On branch main\n"));
    }

    #[test]
    fn lossless_every_file_preserved() {
        let p = GitStatusParser;
        let out = p.parse(HAPPY_PATH_INPUT, &CompressOptions::default());
        for f in [
            "src/auth.rs",
            "src/config.rs",
            "src/main.rs",
            "src/lib.rs",
            "tests/integration.rs",
            ".env.local",
        ] {
            assert!(out.content.contains(f), "missing {f}");
        }
    }
}
