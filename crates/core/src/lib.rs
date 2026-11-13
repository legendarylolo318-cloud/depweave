//! depweave-core: read-only pacman database parsing, dependency graph
//! construction and analysis. No UI code lives here.

pub mod cache;
pub mod conf;
pub mod dep;
pub mod desc;
pub mod error;
pub mod export;
pub mod graph;
pub mod layout;
pub mod loader;
pub mod syncdb;
pub mod version;
pub mod why;

pub use error::{Error, Result};
pub use graph::{DepGraph, Direction, EdgeKind, Impact, Package, Reason, Stats};
pub use loader::{load, LoadOptions};
