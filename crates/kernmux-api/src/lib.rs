//! Versioned management API shared by the host service and its clients.

/// Major version of the local management API.
pub const API_MAJOR_VERSION: u16 = 1;

pub mod v1;
