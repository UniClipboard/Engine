//! Local member roster and per-member preferences for the current space.
//!
//! Short member actions (admit, list, get, update preferences, reset
//! preferences) live directly in this subdomain; the facade entry stays in
//! `crate::facade::roster`.

pub(crate) mod admit_member;
pub(crate) mod errors;
pub(crate) mod get_member;
pub(crate) mod list_members;
pub(crate) mod reset_member_preferences_to_default;
pub(crate) mod update_member_settings;
