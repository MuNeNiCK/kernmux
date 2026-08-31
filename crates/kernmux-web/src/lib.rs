//! Browser renderer adapter for the Kernmux host-management shell.

#[cfg(target_arch = "wasm32")]
mod shell;

#[cfg(target_arch = "wasm32")]
pub use shell::{ManagementBackend, ManagementFuture, open_management_shell};
