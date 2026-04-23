//! YouTube and Instagram cookies management.
//!
//! Split into submodules for readability (previously a 2243-LOC monolith).

pub(super) mod file_ops;
mod instagram;
mod manager;
mod probes;
mod types;
mod watchdog;

pub use file_ops::*;
pub use instagram::*;
pub use manager::*;
pub use probes::*;
pub use types::*;
pub use watchdog::*;

#[cfg(test)]
mod tests;
