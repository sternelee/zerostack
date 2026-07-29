//! Crate version accessor, used by the extension host for the
//! `minimum_zerostack_version` compatibility check.

/// Compile-time version string from `Cargo.toml` (`workspace.package.version`).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
