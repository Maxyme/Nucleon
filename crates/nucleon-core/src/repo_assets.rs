//! Embedded directories. File names and format-specific parsing stay at their use sites.
use include_dir::{Dir, include_dir};

pub(crate) static ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../assets");

#[cfg(test)]
pub(crate) static FIXTURES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/tests/fixtures");
