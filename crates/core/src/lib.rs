//! The core of harness-multitool (IF-0): the agent loop, the event bus, the registry. It holds
//! no concrete implementation (INV-2) and touches no platform (INV-1).

pub mod engine;
pub mod plan;
pub mod registry;

pub use engine::{Engine, RunEnd, StopFlag};
pub use registry::{BuildContext, Parts, Registry, RegistryError, Selection};
