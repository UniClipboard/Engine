//! Space-scoped application workflows.
//!
//! A member roster and every membership transition only exist inside a space.
//! Keeping both under this module gives callers and maintainers one place to
//! follow the complete lifecycle of a space.

pub mod convergence;
pub mod roster;
