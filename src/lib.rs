//! squeeze - lossless CLI output compressor.
//!
//! This crate exposes the compression machinery as a library so the binary
//! (src/main.rs) stays thin and the logic stays testable.
//!
//! Design principle: lossless-first. Every piece of information an agent
//! might need is preserved. When something is omitted, the omission is
//! declared explicitly in the output via a footer line.

pub mod core;
pub mod dispatch;
pub mod lossless;
pub mod parsers;

pub use core::{CommandParser, CompressOptions, CompressedOutput, Strategy, Verbosity};
pub use dispatch::compress;
