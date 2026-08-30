use std::{env, io, process::ExitCode};

use kernmux_daemon::inventory::run_inventory_helper;
use tracing::info;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    if env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--inventory-helper")) {
        return match run_inventory_helper(io::stdout().lock()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        };
    }

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .init();

    info!(
        product = kernmux_core::PRODUCT_NAME,
        api_major = kernmux_api::API_MAJOR_VERSION,
        "host management service initialized"
    );
    ExitCode::SUCCESS
}
