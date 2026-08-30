//! Domain model and backend boundaries for Multikernel host management.

/// Human-readable product name shared by Kernmux components.
pub const PRODUCT_NAME: &str = "Kernmux";

pub mod host;

#[cfg(test)]
mod tests {
    use super::PRODUCT_NAME;

    #[test]
    fn product_name_is_stable() {
        assert_eq!(PRODUCT_NAME, "Kernmux");
    }
}
