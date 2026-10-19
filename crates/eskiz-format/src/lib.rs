//! Reader and writer for the `.procreate` file format.
//!
//! A pure-Rust implementation, written from file format notes:
//!
//! * [`codecs`]: Apple framed LZ4 and LZO1X-1 tile decompressors, and the Apple framed LZ4 encoder.
//! * [`keyedarchive`]: binary property list and NSKeyedArchiver object-graph decoding and encoding.
//! * [`document`]: the ZIP package, `Document.archive`, the layer tree and tile assembly, and
//!   the writer ([`document::write`], [`DocumentBuilder`]).
#![deny(unsafe_code)]

pub mod brushes;
pub mod codecs;
pub mod document;
pub mod export;
pub mod icc;
mod error;
pub mod fsutil;
pub mod keyedarchive;
pub mod limits;
mod pack;
pub mod palettes;
pub mod psd;
#[cfg(test)]
mod reference;

pub use codecs::{apple_lz4_compress, apple_lz4_declared_size, apple_lz4_decompress, lz4_block_compress, lzo1x_decompress};
pub use document::{
    blend_mode_name, AnimationSpec, DecodedTile, Document, DocumentBuilder, Group, GroupSpec, Item, Layer,
    LayerSpec, Node, TextRunSpec, TextSpec, TextValue, TileEncoding, TilePixels, BLEND_MODES,
};
pub use error::{Error, Result};
pub use pack::{LayerTiles, TileSource};
pub use keyedarchive::{
    unarchive, ArchivedObject, KeyedUnarchiver, NsNumber, NsObject, NsValue, Object,
};
