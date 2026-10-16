//! Independent read-only native inspector guest for the PhotoCraft Plugin API.
#![deny(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod abi;
mod handler;

const MANIFEST: &str = include_str!("../manifest.json");
const MAX_JSON_BYTES: usize = 256 * 1024;
