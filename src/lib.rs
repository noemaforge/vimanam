//! Parse OpenAPI specs, compare their contracts, and render focused Markdown context.
//!
//! The default `cli` feature includes the binary. Depend on the core alone with
//! `vimanam = { version = "1.6", default-features = false }`.
//! Library routines never write to stdout/stderr: rendering accepts a writer,
//! optional notices go to a caller callback, and parsing logs through `log`.
//!
//! ```no_run
//! # fn main() -> anyhow::Result<()> {
//! let old = vimanam::parse_openapi("immich-v1.105.1.json")?;
//! let new = vimanam::parse_openapi("immich-v1.106.0.json")?;
//! let old_sha = vimanam::diff::json::sha256_hex(&std::fs::read("immich-v1.105.1.json")?);
//! let new_sha = vimanam::diff::json::sha256_hex(&std::fs::read("immich-v1.106.0.json")?);
//! let diff = vimanam::diff::diff(&old, &new);
//! let document = vimanam::diff::json::to_json(&diff, None, &old_sha, &new_sha);
//! for change in &document.changes {
//!     println!("{} {:?} {:?}", change.id, change.kind, change.severity);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! For hashes tied to exactly the bytes parsed, use [`parse_openapi_bytes`].
//! Select operations with [`DocConfig::operation_selector`] and schemas with
//! [`DocConfig::schema_names`] or [`DocConfig::schema_fields`], then call
//! [`markdown::generate_markdown`]. See [`DocConfig::unfiltered`] for defaults.

#![warn(missing_docs)]

#[cfg(feature = "cli")]
mod costs;
pub mod diff;
pub mod markdown;
mod models;
mod parser;
mod report;
pub mod selection;
#[cfg(feature = "cli")]
mod stats;
mod utils;

// The argument/application adapter is outside the core library surface.
#[cfg(feature = "cli")]
#[doc(hidden)]
#[allow(missing_docs)]
pub mod cli;

pub use models::{
    AdditionalProperties, ApiDocumentation, DetailLevel, DocConfig, Encoding, Endpoint, Example,
    GroupBy, Header, MediaType, OperationRef, OperationSelector, Parameter, Response, Schema,
    Service, SortMethod,
};
pub use parser::{parse_openapi, parse_openapi_bytes};
