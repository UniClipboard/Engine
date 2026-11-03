mod client;
mod runtime;

pub(super) use runtime::{ManagedRelay, CLOUD_RELAY_API};

#[cfg(test)]
use runtime::BUILTIN_RELAY_ID;
#[cfg(test)]
mod tests;
