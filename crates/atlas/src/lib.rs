//! pdfcraft-atlas — Atlas Switch's construction-drawing logic (L4).
//!
//! The rules the Atlas fork layers on top of PdfCraft, kept out of the upstream crates so the
//! fork stays easy to rebase:
//! - [`layers`]: group CAD layer names (NCS discipline designators, xref prefixes, per-page
//!   containers from combined files) and turn panel actions into layer visibility changes.
//!   Pure logic.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod layers;
