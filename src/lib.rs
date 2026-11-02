pub mod detector;
pub mod engine;
pub mod geometry;
#[cfg(feature = "gui")]
pub mod gui;
pub mod input;
pub mod overlay;
pub mod parser;
pub mod report;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
