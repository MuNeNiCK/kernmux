use tracing::info;
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .init();

    info!(
        product = kernmux_core::PRODUCT_NAME,
        api_major = kernmux_api::API_MAJOR_VERSION,
        "host management service initialized"
    );
}
