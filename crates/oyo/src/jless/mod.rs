#![allow(dead_code, mismatched_lifetime_syntaxes)]
#![allow(clippy::all)]

//! Vendored jless core used for interactive JSON and YAML previews.
//!
//! Source: https://github.com/PaulJuliusMartinez/jless
//! Version: 0.9.0
//! License: MIT. See LICENSE-MIT in this directory.

pub(crate) mod flatjson;
pub(crate) mod highlighting;
pub(crate) mod jsonparser;
pub(crate) mod jsonstringunescaper;
pub(crate) mod jsontokenizer;
pub(crate) mod lineprinter;
pub(crate) mod search;
pub(crate) mod terminal;
pub(crate) mod truncatedstrview;
pub(crate) mod types;
pub(crate) mod viewer;
pub(crate) mod yamlparser;
