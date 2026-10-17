//! The document model: a layer tree with groups, masks, clipping, lock transparency and a background
//! colour. The types live in `eskiz-core` so that the compositor can use them; they are
//! re-exported here for the app. See [`eskiz_core::document`].

pub use eskiz_core::document::*;
