use std::{env, io, process::ExitCode};

use kernmux_daemon::{
    inventory::run_inventory_helper,
    service::{DaemonConfig, run},
};
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

    let config = match DaemonConfig::from_environment() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("invalid daemon configuration: {}", error.message);
            return ExitCode::FAILURE;
        }
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("failed to initialize async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("host management service failed: {error}");
            ExitCode::FAILURE
        }
    }
}
