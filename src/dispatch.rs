//! Parser dispatch. Walks the parser registry in order, uses the first
//! match. The passthrough fallback is guaranteed to match any input.

use crate::core::{CommandParser, CompressOptions, CompressedOutput};
use crate::parsers;

/// Compress `output` produced by the given `command`.
///
/// The passthrough fallback is guaranteed to be the terminal parser and
/// always matches, so this function is infallible in practice.
pub fn compress(command: &str, output: &str, opts: &CompressOptions) -> CompressedOutput {
    for parser in parsers::all() {
        if parser.can_handle(command, output) {
            return parser.parse(output, opts);
        }
    }
    // Unreachable in practice: passthrough always matches.
    parsers::passthrough::AnsiStripPassthrough.parse(output, opts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Strategy;

    #[test]
    fn dispatch_falls_back_to_passthrough_for_unknown_command() {
        let out = compress("echo hello", "hello\n", &CompressOptions::default());
        assert_eq!(out.strategy, Strategy::Passthrough);
    }
}
