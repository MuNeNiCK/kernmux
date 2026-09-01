use std::{collections::BTreeSet, net::SocketAddr, path::PathBuf, str::FromStr};

use kernmux_client::UnixTransport;
use kernmux_gateway::{Gateway, GatewayConfig, read_token_file};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    if rustix::process::geteuid().is_root() {
        eprintln!("kernmux-gateway refuses to run as root");
        std::process::exit(1);
    }
    let config = match config_from_environment() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("invalid gateway configuration: {error}");
            std::process::exit(2);
        }
    };
    let listener = match TcpListener::bind(config.bind).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("failed to bind {}: {error}", config.bind);
            std::process::exit(1);
        }
    };
    let transport = UnixTransport::new(std::env::var_os("KERNMUXD_SOCKET").map_or_else(
        || PathBuf::from("/run/kernmux/kernmuxd.sock"),
        PathBuf::from,
    ));
    let mut config = config;
    config.daemon_socket = transport.socket_path().to_owned();
    let gateway = Gateway::new(config, transport).expect("validated gateway configuration");
    if let Err(error) = gateway.serve(listener, shutdown()).await {
        eprintln!("gateway stopped: {error}");
        std::process::exit(1);
    }
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}

fn config_from_environment() -> Result<GatewayConfig, Box<dyn std::error::Error>> {
    let bind = std::env::var("KERNMUX_GATEWAY_BIND").unwrap_or_else(|_| "127.0.0.1:9443".into());
    let bind = SocketAddr::from_str(&bind)?;
    let token_path = std::env::var_os("KERNMUX_GATEWAY_TOKEN_FILE").map_or_else(
        || PathBuf::from("/etc/kernmux/gateway.token"),
        PathBuf::from,
    );
    let token = read_token_file(&token_path)?;
    let assets = std::env::var_os("KERNMUX_GATEWAY_ASSETS")
        .map_or_else(|| PathBuf::from("/usr/share/kernmux/web"), PathBuf::from);
    let mut config = GatewayConfig::loopback(token, assets);
    config.bind = bind;
    config.allow_non_loopback =
        std::env::var("KERNMUX_GATEWAY_ALLOW_NON_LOOPBACK").is_ok_and(|value| value == "1");
    if let Ok(origins) = std::env::var("KERNMUX_GATEWAY_ORIGINS") {
        config.allowed_origins = origins
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
    } else {
        config.allowed_origins = BTreeSet::from([
            format!("http://{}", config.bind),
            format!("http://localhost:{}", config.bind.port()),
        ]);
    }
    config.validate()?;
    Ok(config)
}
