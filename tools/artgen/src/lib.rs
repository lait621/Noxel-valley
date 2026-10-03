//! The sprite generator for Noxel Valley, as a library.
//!
//! The binary is a thin front end over [`farm::write`]. Splitting them means the
//! contract tests — the 141 region names, every size, alpha in `{0, 255}`, every
//! pixel inside the palette, byte-stability across two builds — run under
//! `cargo test` rather than only when someone happens to regenerate the art.
//!
//! Built on `noxel-gen`'s library half: the palette, the drawing helpers and the
//! error type are the engine's, and what lives here is the part that is *this
//! game's* — which sprites exist, what they look like, and the region names the
//! game loads them by.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::all)]

pub mod farm;
pub mod farm_preview;
