mod error;
mod ports;

pub use error::CurrentSpaceIdentityError;
pub use ports::{
    CurrentSpaceIdentityPort, InitialSpaceActivationPort, PortableCurrentSpaceIdentityPort,
};
