//! Local member roster and per-member preferences for the current space.
//!
//! Short member actions (admit, list, get, update preferences, reset
//! preferences) live directly in this subdomain; the facade entry stays in
//! `crate::facade::roster`.

pub(crate) mod rebuild;