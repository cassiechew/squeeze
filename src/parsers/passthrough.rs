//! Fallback parser. Applies only lossless operations: ANSI strip,
//! blank-line collapse, CR progress-bar collapse, consecutive dedup.
//! Passes everything else through unchanged.

use crate::core::{CommandParser, CompressOptions, CompressedOutput, Strategy};
use crate::lossless::lossless_pipeline;

pub struct AnsiStripPassthrough;

impl CommandParser for AnsiStripPassthrough {
    fn strategy(&self) -> Strategy {
        Strategy::Passthrough
    }

    fn can_handle(&self, _command: &str, _output: &str) -> bool {
        true
    }

    fn parse(&self, output: &str, _opts: &CompressOptions) -> CompressedOutput {
        CompressedOutput {
            content: lossless_pipeline(output),
            strategy: Strategy::Passthrough,
            omitted: None,
            lossless: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn happy_path_passes_text_through() {
        let p = AnsiStripPassthrough;
        let out = p.parse("hello world\n", &CompressOptions::default());
        assert_eq!(out.content, "hello world\n");
        assert!(out.lossless);
        assert!(out.omitted.is_none());
    }

    #[test]
    fn strips_ansi_while_preserving_text() {
        let p = AnsiStripPassthrough;
        let out = p.parse("\x1b[31merror\x1b[0m: wat\n", &CompressOptions::default());
        assert_eq!(out.content, "error: wat\n");
    }

    #[test]
    fn empty_input_yields_empty_output() {
        let p = AnsiStripPassthrough;
        let out = p.parse("", &CompressOptions::default());
        assert_eq!(out.content, "");
        assert!(out.lossless);
    }

    #[test]
    fn always_handles() {
        let p = AnsiStripPassthrough;
        assert!(p.can_handle("any command", "any output"));
        assert!(p.can_handle("", ""));
    }

    #[test]
    fn lossless_assertion_content_preserved() {
        // Every word in the clean input must remain in the output.
        let p = AnsiStripPassthrough;
        let input = "\x1b[1mHeader\x1b[0m\nrow alpha\nrow beta\n";
        let out = p.parse(input, &CompressOptions::default());
        for word in ["Header", "row", "alpha", "beta"] {
            assert!(out.content.contains(word), "lost '{word}': {}", out.content);
        }
    }
}
