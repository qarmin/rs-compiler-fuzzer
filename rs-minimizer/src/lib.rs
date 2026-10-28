//! Test-case reducer for Rust programs. It does what generic reducers (Shrink Ray, treereduce, C-Vise) do badly on generated or
//! machine-renamed code: expanding `macro_rules!`, removing `catch_unwind` plumbing, hoisting modules, deleting whole items and
//! compiler-reported dead code, simplifying expression chains, and has the generic passes of those tools (hollowing brackets, lifting braces,
//! repeated-sequence deletion) built in, scheduled by what they yielded so far.
//!
//! The reduction is described by an [`Oracle`]: "is this program still showing the problem?".

pub mod bracket;
pub mod cli;
pub mod combine;
pub mod control;
pub mod deadcode;
pub mod diag;
pub mod exprs;
pub mod format;
pub mod inline;
pub mod items;
pub mod lines;
pub mod modules;
pub mod oracle;
pub mod par;
pub mod pipeline;
pub mod rename;
pub mod rustc_oracle;
pub mod peep;
pub mod scan;
pub mod structure;
pub mod tokens;
pub mod traits;
pub mod ui;
pub mod unroll;

pub use oracle::{Cached, Oracle, script_predicate};
pub use par::par_map;
pub use pipeline::{Hook, Options, ReduceLog, Stage, minimize};
