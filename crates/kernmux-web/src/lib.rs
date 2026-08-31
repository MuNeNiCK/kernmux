//! Browser renderer adapter for the Kernmux host-management shell.

#[cfg(target_arch = "wasm32")]
mod shell;

#[cfg(target_arch = "wasm32")]
pub use shell::{fail_management_shell, install_management_snapshot, open_management_shell};
