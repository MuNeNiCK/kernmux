//! Versioned management API types shared by the host service and clients.

/// Major version of the local management API.
pub const API_MAJOR_VERSION: u16 = 1;

#[cfg(test)]
mod tests {
    use super::API_MAJOR_VERSION;

    #[test]
    fn initial_api_version_is_one() {
        assert_eq!(API_MAJOR_VERSION, 1);
    }
}
