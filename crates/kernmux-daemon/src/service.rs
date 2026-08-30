//! Environment-backed configuration and lifetime of the host service.

use std::{ffi::OsString, sync::Arc};

use kernmux_api::v1::{ApiError, ErrorCode};

use crate::{
    host_api::{RunningHostApi, RunningHostApiConfig},
    security::AuthorizationPolicy,
    transport::{LocalHttpServer, TransportConfig, TransportError},
};

/// Fully validated configuration of one local daemon process.
#[derive(Clone, Debug)]
pub struct DaemonConfig {
    pub transport: TransportConfig,
    pub host: RunningHostApiConfig,
    pub authorization: AuthorizationPolicy,
}

impl DaemonConfig {
    /// Reads documented `KERNMUX_*` environment variables over secure defaults.
    ///
    /// # Errors
    ///
    /// Rejects malformed IDs, booleans, modes, limits, and non-Unicode values.
    pub fn from_environment() -> Result<Self, ApiError> {
        Self::from_lookup(|name| std::env::var_os(name))
    }

    fn from_lookup(mut lookup: impl FnMut(&str) -> Option<OsString>) -> Result<Self, ApiError> {
        let mut transport = TransportConfig::system_default();
        let mut host = RunningHostApiConfig::system_default();
        let mut authorization = AuthorizationPolicy::deny_by_default();

        if let Some(value) = optional_string(&mut lookup, "KERNMUX_SOCKET_PATH")? {
            if value.is_empty() {
                return Err(config_error("socket path must not be empty"));
            }
            transport.socket_path = value.into();
        }
        if let Some(value) = optional_string(&mut lookup, "KERNMUX_SOCKET_MODE")? {
            transport.socket_mode = u32::from_str_radix(&value, 8)
                .map_err(|_| config_error("socket mode must be octal"))?;
        }
        if let Some(value) = optional_string(&mut lookup, "KERNMUX_IMAGE_ROOTS")? {
            let roots = std::env::split_paths(&OsString::from(value)).collect::<Vec<_>>();
            if roots.is_empty() {
                return Err(config_error("image roots must not be empty"));
            }
            host.image_roots = roots;
        }
        if let Some(value) = optional_string(&mut lookup, "KERNMUX_ALLOW_UNPRIVILEGED_READS")? {
            authorization = authorization.with_unprivileged_reads(parse_bool(&value)?);
        }
        for uid in id_list(&mut lookup, "KERNMUX_READER_UIDS")? {
            authorization = authorization.with_reader_uid(uid);
        }
        for gid in id_list(&mut lookup, "KERNMUX_READER_GIDS")? {
            authorization = authorization.with_reader_gid(gid);
        }
        for uid in id_list(&mut lookup, "KERNMUX_OPERATOR_UIDS")? {
            authorization = authorization.with_operator_uid(uid);
        }
        for gid in id_list(&mut lookup, "KERNMUX_OPERATOR_GIDS")? {
            authorization = authorization.with_operator_gid(gid);
        }
        for uid in id_list(&mut lookup, "KERNMUX_ADMINISTRATOR_UIDS")? {
            authorization = authorization.with_administrator_uid(uid);
        }
        for gid in id_list(&mut lookup, "KERNMUX_ADMINISTRATOR_GIDS")? {
            authorization = authorization.with_administrator_gid(gid);
        }

        override_usize(
            &mut lookup,
            "KERNMUX_MAX_CONNECTIONS",
            &mut host.service_limits.connections,
        )?;
        override_usize(
            &mut lookup,
            "KERNMUX_MAX_MUTATIONS",
            &mut host.service_limits.mutations,
        )?;
        override_usize(
            &mut lookup,
            "KERNMUX_MAX_CONSOLES",
            &mut host.service_limits.consoles,
        )?;
        override_usize(
            &mut lookup,
            "KERNMUX_MAX_REQUEST_BYTES",
            &mut transport.max_request_bytes,
        )?;
        override_usize(
            &mut lookup,
            "KERNMUX_MAX_HEADER_BYTES",
            &mut transport.max_header_bytes,
        )?;

        host.service_limits.validate()?;
        Ok(Self {
            transport,
            host,
            authorization,
        })
    }
}

/// Builds and runs the daemon until a termination signal arrives.
///
/// # Errors
///
/// Returns configuration, backend construction, transport, or signal errors.
pub async fn run(config: DaemonConfig) -> Result<(), ServiceError> {
    let (api, limiter) =
        RunningHostApi::running_host(config.host).map_err(ServiceError::HostApi)?;
    let server = LocalHttpServer::bind(
        config.transport,
        config.authorization,
        limiter,
        Arc::new(api),
    )
    .map_err(ServiceError::Transport)?;
    tracing::info!(socket = %server.socket_path().display(), "local host API ready");
    server
        .run(wait_for_shutdown())
        .await
        .map_err(ServiceError::Transport)
}

async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    result = tokio::signal::ctrl_c() => {
                        if let Err(error) = result {
                            tracing::error!(%error, "failed to wait for interrupt signal");
                        }
                    }
                    _ = terminate.recv() => {}
                }
            }
            Err(error) => {
                tracing::error!(%error, "failed to register termination signal");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn optional_string(
    lookup: &mut impl FnMut(&str) -> Option<OsString>,
    name: &str,
) -> Result<Option<String>, ApiError> {
    lookup(name)
        .map(|value| {
            value
                .into_string()
                .map_err(|_| config_error("configuration value must be Unicode"))
        })
        .transpose()
}

fn id_list(
    lookup: &mut impl FnMut(&str) -> Option<OsString>,
    name: &str,
) -> Result<Vec<u32>, ApiError> {
    let Some(value) = optional_string(lookup, name)? else {
        return Ok(Vec::new());
    };
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    value
        .split(',')
        .map(|part| {
            part.trim()
                .parse::<u32>()
                .map_err(|_| config_error("identity list is invalid"))
        })
        .collect()
}

fn parse_bool(value: &str) -> Result<bool, ApiError> {
    match value {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err(config_error("boolean configuration value is invalid")),
    }
}

fn override_usize(
    lookup: &mut impl FnMut(&str) -> Option<OsString>,
    name: &str,
    target: &mut usize,
) -> Result<(), ApiError> {
    if let Some(value) = optional_string(lookup, name)? {
        *target = value
            .parse::<usize>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| config_error("numeric limit must be greater than zero"))?;
    }
    Ok(())
}

fn config_error(message: &str) -> ApiError {
    ApiError {
        code: ErrorCode::InvalidRequest,
        message: message.into(),
        retryable: false,
        current_generation: None,
        diagnostics: Vec::new(),
    }
}

/// Failure while constructing or running the daemon service.
#[derive(Debug)]
pub enum ServiceError {
    HostApi(crate::host_api::HostApiBuildError),
    Transport(TransportError),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HostApi(error) => error.fmt(formatter),
            Self::Transport(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ServiceError {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::security::{RequestClass, Role};

    use super::*;

    #[test]
    fn defaults_deny_unprivileged_callers() {
        let config = DaemonConfig::from_lookup(|_| None).unwrap();
        let actor = kernmux_api::v1::Actor {
            uid: 1000,
            gid: 1000,
            label: None,
        };

        assert!(
            config
                .authorization
                .authorize(&actor, RequestClass::ReadOnly)
                .is_err()
        );
        assert_eq!(config.transport.socket_mode, 0o660);
    }

    #[test]
    fn explicit_identity_and_limits_are_parsed_without_commands() {
        let values = BTreeMap::from([
            ("KERNMUX_OPERATOR_UIDS", "1000,1001"),
            ("KERNMUX_ADMINISTRATOR_GIDS", "2000"),
            ("KERNMUX_MAX_CONNECTIONS", "12"),
            ("KERNMUX_SOCKET_MODE", "600"),
        ]);
        let config =
            DaemonConfig::from_lookup(|name| values.get(name).map(OsString::from)).unwrap();

        assert_eq!(config.host.service_limits.connections, 12);
        assert_eq!(config.transport.socket_mode, 0o600);
        assert_eq!(
            config
                .authorization
                .authorize(
                    &kernmux_api::v1::Actor {
                        uid: 1001,
                        gid: 10,
                        label: None,
                    },
                    RequestClass::Mutation,
                )
                .unwrap(),
            Role::Operator
        );
    }

    #[test]
    fn malformed_configuration_is_rejected() {
        assert!(
            DaemonConfig::from_lookup(|name| {
                (name == "KERNMUX_OPERATOR_UIDS").then(|| "not-a-uid".into())
            })
            .is_err()
        );
        assert!(
            DaemonConfig::from_lookup(|name| {
                (name == "KERNMUX_MAX_CONNECTIONS").then(|| "0".into())
            })
            .is_err()
        );
    }
}
