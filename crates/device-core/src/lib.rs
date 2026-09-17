//! Shared, window-independent device-group core.

pub mod business;
pub mod discovery;
pub mod identity;
pub mod join_transport;
pub mod membership;
pub mod mesh;
pub mod node;
pub mod storage;
pub mod transport;
pub mod types;

pub use types::{
    AddressCandidate, CandidateSource, ConnectionState, DEVICE_PROTOCOL_VERSION, DeviceProfile,
    GroupProfile, TRANSPORT_PROTOCOL_VERSION,
};
