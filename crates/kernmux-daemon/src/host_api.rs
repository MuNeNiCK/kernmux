//! Versioned host API dispatch over the authenticated local transport.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use hyper::{Method, StatusCode};
use kernmux_api::v1::{
    ApiError, CreateInstanceMutation, ErrorCode, EventPage, EventSequence, Generation, InstanceId,
    InstanceLifecycleMutation, LoadInstanceMutation, Operation, OperationId, OperationKind,
    ResourceKind, ResourcePoolMutation, ResourceReference, Response, StopInstanceMutation,
    UpdateInstanceMutation,
};

use crate::{
    inventory::{InventoryService, ProcessInventorySource},
    lifecycle::{
        CreateRequest, InstanceRequest, LifecycleRequest, LoadRequest, StopRequest, UpdateRequest,
    },
    lifecycle_executor::{KerfRunner, LifecycleExecutor, ProcessKerfRunner, SnapshotRefresher},
    operations::{NewOperation, OperationRegistry, OperationRegistryError},
    resource_pool::{ResourcePoolExecutor, ResourcePoolRequest},
    scheduler::{
        CancellationToken, OperationScheduler, OperationTaskResult, ScheduleError,
        lifecycle_task_result, resource_pool_task_result,
    },
    security::{AuditAction, LimitKind, PeerIdentity, RequestClass, ServiceLimiter, ServiceLimits},
    transport::{LocalApi, LocalRequest, LocalResponse, RouteSecurity},
};

const DEFAULT_OPERATION_CAPACITY: usize = 1024;
const DEFAULT_EVENT_CAPACITY: usize = 4096;
const DEFAULT_PROBE_DEADLINE: Duration = Duration::from_secs(10);
const DEFAULT_KERF_DEADLINE: Duration = Duration::from_secs(120);
const DEFAULT_KERF_OUTPUT_LIMIT: u64 = 4 * 1024 * 1024;
const MAX_COMMAND_LINE_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InstanceAction {
    Load,
    Start,
    Stop,
    Unload,
}

/// Canonical roots from which kernel and initrd files may be loaded.
#[derive(Clone, Debug)]
pub struct ImagePolicy {
    roots: Vec<PathBuf>,
}

impl ImagePolicy {
    /// Canonicalizes a nonempty set of administrator-controlled roots.
    ///
    /// # Errors
    ///
    /// Rejects missing, non-directory, or empty roots.
    pub fn new(roots: impl IntoIterator<Item = PathBuf>) -> Result<Self, ApiError> {
        let mut canonical = Vec::new();
        for root in roots {
            let root = root
                .canonicalize()
                .map_err(|_| invalid("image root is unavailable"))?;
            if !root.is_dir() {
                return Err(invalid("image root is not a directory"));
            }
            if !canonical.contains(&root) {
                canonical.push(root);
            }
        }
        if canonical.is_empty() {
            return Err(invalid("at least one image root is required"));
        }
        Ok(Self { roots: canonical })
    }

    /// Resolves a regular file contained by an allowed canonical root.
    ///
    /// # Errors
    ///
    /// Rejects missing files, directories, symlinks escaping a root, and
    /// relative paths.
    pub fn resolve(&self, path: &str) -> Result<PathBuf, ApiError> {
        let requested = Path::new(path);
        if !requested.is_absolute() {
            return Err(invalid("image path must be absolute"));
        }
        let canonical = requested
            .canonicalize()
            .map_err(|_| invalid("image file is unavailable"))?;
        if !self.roots.iter().any(|root| canonical.starts_with(root)) {
            return Err(ApiError {
                code: ErrorCode::Forbidden,
                message: "image path is outside configured roots".into(),
                retryable: false,
                current_generation: None,
                diagnostics: Vec::new(),
            });
        }
        let metadata = canonical
            .metadata()
            .map_err(|_| invalid("image file is unavailable"))?;
        if !metadata.is_file() {
            return Err(invalid("image path is not a regular file"));
        }
        Ok(canonical)
    }
}

/// Concrete dispatch configuration for a running host.
#[derive(Clone, Debug)]
pub struct RunningHostApiConfig {
    pub image_roots: Vec<PathBuf>,
    pub service_limits: ServiceLimits,
    pub probe_deadline: Duration,
    pub kerf_deadline: Duration,
    pub kerf_output_limit: u64,
    pub operation_capacity: usize,
    pub event_capacity: usize,
}

impl RunningHostApiConfig {
    /// Secure local defaults. Packaging creates the managed image directory.
    #[must_use]
    pub fn system_default() -> Self {
        Self {
            image_roots: vec![
                PathBuf::from("/boot"),
                PathBuf::from("/var/lib/kernmux/images"),
            ],
            service_limits: ServiceLimits {
                connections: 64,
                mutations: 4,
                consoles: 8,
            },
            probe_deadline: DEFAULT_PROBE_DEADLINE,
            kerf_deadline: DEFAULT_KERF_DEADLINE,
            kerf_output_limit: DEFAULT_KERF_OUTPUT_LIMIT,
            operation_capacity: DEFAULT_OPERATION_CAPACITY,
            event_capacity: DEFAULT_EVENT_CAPACITY,
        }
    }
}

/// Authenticated dispatcher connected to authoritative host backends.
pub struct HostApi<I, LR, LS, PR, PS> {
    inventory: Mutex<I>,
    lifecycle: Arc<Mutex<LifecycleExecutor<LR, LS>>>,
    resource_pool: Arc<Mutex<ResourcePoolExecutor<PR, PS>>>,
    registry: OperationRegistry,
    scheduler: OperationScheduler,
    limiter: ServiceLimiter,
    image_policy: ImagePolicy,
}

impl<I, LR, LS, PR, PS> std::fmt::Debug for HostApi<I, LR, LS, PR, PS> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostApi")
            .field("registry", &self.registry)
            .field("scheduler", &self.scheduler)
            .field("image_policy", &self.image_policy)
            .finish_non_exhaustive()
    }
}

impl<I, LR, LS, PR, PS> HostApi<I, LR, LS, PR, PS> {
    /// Assembles the dispatcher from replaceable backend components.
    #[must_use]
    pub fn new(
        inventory: I,
        lifecycle: LifecycleExecutor<LR, LS>,
        resource_pool: ResourcePoolExecutor<PR, PS>,
        registry: OperationRegistry,
        scheduler: OperationScheduler,
        limiter: ServiceLimiter,
        image_policy: ImagePolicy,
    ) -> Self {
        Self {
            inventory: Mutex::new(inventory),
            lifecycle: Arc::new(Mutex::new(lifecycle)),
            resource_pool: Arc::new(Mutex::new(resource_pool)),
            registry,
            scheduler,
            limiter,
            image_policy,
        }
    }
}

/// Running-host dispatcher type used by `kernmuxd`.
pub type RunningHostApi = HostApi<
    InventoryService<ProcessInventorySource>,
    ProcessKerfRunner,
    InventoryService<ProcessInventorySource>,
    ProcessKerfRunner,
    InventoryService<ProcessInventorySource>,
>;

impl RunningHostApi {
    /// Creates isolated inventory sources and bounded Kerf executors.
    ///
    /// # Errors
    ///
    /// Rejects invalid configuration and failures to resolve the daemon
    /// executable used by isolated probes.
    pub fn running_host(
        config: RunningHostApiConfig,
    ) -> Result<(Self, ServiceLimiter), HostApiBuildError> {
        let limiter =
            ServiceLimiter::new(config.service_limits).map_err(HostApiBuildError::Configuration)?;
        let registry = OperationRegistry::new(config.operation_capacity, config.event_capacity)
            .map_err(HostApiBuildError::Registry)?;
        let scheduler = OperationScheduler::new(registry.clone(), timestamp);
        let inventory = isolated_inventory(config.probe_deadline)?;
        let lifecycle = LifecycleExecutor::new(
            ProcessKerfRunner::system(config.kerf_deadline, config.kerf_output_limit),
            isolated_inventory(config.probe_deadline)?,
        );
        let resource_pool = ResourcePoolExecutor::new(
            ProcessKerfRunner::system(config.kerf_deadline, config.kerf_output_limit),
            isolated_inventory(config.probe_deadline)?,
        );
        let image_policy =
            ImagePolicy::new(config.image_roots).map_err(HostApiBuildError::Configuration)?;
        Ok((
            Self::new(
                inventory,
                lifecycle,
                resource_pool,
                registry,
                scheduler,
                limiter.clone(),
                image_policy,
            ),
            limiter,
        ))
    }
}

fn isolated_inventory(
    deadline: Duration,
) -> Result<InventoryService<ProcessInventorySource>, HostApiBuildError> {
    ProcessInventorySource::running_host(deadline)
        .map(InventoryService::new)
        .map_err(HostApiBuildError::CurrentExecutable)
}

impl<I, LR, LS, PR, PS> LocalApi for HostApi<I, LR, LS, PR, PS>
where
    I: SnapshotRefresher + Send + 'static,
    LR: KerfRunner + Send + 'static,
    LS: SnapshotRefresher + Send + 'static,
    PR: KerfRunner + Send + 'static,
    PS: SnapshotRefresher + Send + 'static,
{
    fn route(&self, method: &Method, path: &str) -> Option<RouteSecurity> {
        route_for(method, path)
    }

    fn handle(&self, request: LocalRequest, peer: &PeerIdentity) -> LocalResponse {
        self.dispatch(request, peer)
            .unwrap_or_else(LocalResponse::api_error)
    }
}

impl<I, LR, LS, PR, PS> HostApi<I, LR, LS, PR, PS>
where
    I: SnapshotRefresher,
    LR: KerfRunner + Send + 'static,
    LS: SnapshotRefresher + Send + 'static,
    PR: KerfRunner + Send + 'static,
    PS: SnapshotRefresher + Send + 'static,
{
    fn dispatch(
        &self,
        request: LocalRequest,
        peer: &PeerIdentity,
    ) -> Result<LocalResponse, ApiError> {
        let path = request.uri.path();
        match (&request.method, path) {
            (&Method::GET, "/1.0") => self.snapshot(),
            (&Method::GET, "/1.0/operations") => self.operations(),
            (&Method::GET, "/1.0/events") => self.events(request.uri.query()),
            (&Method::PUT, "/1.0/resource-pool") => {
                let mutation = decode::<ResourcePoolMutation>(&request.body)?;
                self.submit_resource_pool(mutation, peer)
            }
            (&Method::POST, "/1.0/instances") => {
                let mutation = decode::<CreateInstanceMutation>(&request.body)?;
                self.submit_lifecycle(
                    LifecycleRequest::Create(CreateRequest {
                        expected_generation: mutation.expected_generation,
                        id: mutation.id,
                        name: mutation.name,
                        cpu_hardware_ids: mutation.cpu_hardware_ids,
                        memory_bytes: mutation.memory_bytes,
                    }),
                    OperationKind::CreateInstance,
                    mutation.expected_generation,
                    mutation.id,
                    peer,
                )
            }
            _ => self.dispatch_resource(request, peer),
        }
    }

    fn dispatch_resource(
        &self,
        request: LocalRequest,
        peer: &PeerIdentity,
    ) -> Result<LocalResponse, ApiError> {
        if let Some(operation_id) = operation_id_from_path(request.uri.path()) {
            return match request.method {
                Method::GET => self.operation(&operation_id),
                Method::DELETE => self.cancel(&operation_id),
                _ => Err(not_found()),
            };
        }
        let Some((instance_id, action)) = instance_resource(request.uri.path()) else {
            return Err(not_found());
        };
        self.dispatch_instance(request, peer, instance_id, action)
    }

    fn dispatch_instance(
        &self,
        request: LocalRequest,
        peer: &PeerIdentity,
        instance_id: InstanceId,
        action: Option<InstanceAction>,
    ) -> Result<LocalResponse, ApiError> {
        match (request.method, action) {
            (Method::GET, None) => self.instance(instance_id),
            (Method::PATCH, None) => {
                let mutation = decode::<UpdateInstanceMutation>(&request.body)?;
                self.submit_lifecycle(
                    LifecycleRequest::Update(UpdateRequest {
                        instance: instance_request(mutation.expected_generation, instance_id),
                        cpu_hardware_ids: mutation.cpu_hardware_ids,
                        memory_bytes: mutation.memory_bytes,
                        dry_run: mutation.dry_run,
                    }),
                    OperationKind::UpdateInstance,
                    mutation.expected_generation,
                    instance_id,
                    peer,
                )
            }
            (Method::DELETE, None) => {
                let mutation = decode::<InstanceLifecycleMutation>(&request.body)?;
                self.submit_lifecycle(
                    LifecycleRequest::Delete(instance_request(
                        mutation.expected_generation,
                        instance_id,
                    )),
                    OperationKind::DeleteInstance,
                    mutation.expected_generation,
                    instance_id,
                    peer,
                )
            }
            (Method::POST, Some(InstanceAction::Load)) => {
                let mutation = decode::<LoadInstanceMutation>(&request.body)?;
                let kernel = self.image_policy.resolve(&mutation.kernel_path)?;
                let initrd = mutation
                    .initrd_path
                    .as_deref()
                    .map(|path| self.image_policy.resolve(path))
                    .transpose()?;
                validate_command_line(mutation.command_line.as_deref())?;
                self.submit_lifecycle(
                    LifecycleRequest::Load(LoadRequest {
                        instance: instance_request(mutation.expected_generation, instance_id),
                        kernel,
                        initrd,
                        cmdline: mutation.command_line,
                    }),
                    OperationKind::LoadInstance,
                    mutation.expected_generation,
                    instance_id,
                    peer,
                )
            }
            (Method::POST, Some(InstanceAction::Start)) => {
                let mutation = decode::<InstanceLifecycleMutation>(&request.body)?;
                self.submit_lifecycle(
                    LifecycleRequest::Start(instance_request(
                        mutation.expected_generation,
                        instance_id,
                    )),
                    OperationKind::StartInstance,
                    mutation.expected_generation,
                    instance_id,
                    peer,
                )
            }
            (Method::POST, Some(InstanceAction::Stop)) => {
                let mutation = decode::<StopInstanceMutation>(&request.body)?;
                self.submit_lifecycle(
                    LifecycleRequest::Stop(StopRequest {
                        instance: instance_request(mutation.expected_generation, instance_id),
                        force: mutation.force,
                    }),
                    OperationKind::StopInstance,
                    mutation.expected_generation,
                    instance_id,
                    peer,
                )
            }
            (Method::POST, Some(InstanceAction::Unload)) => {
                let mutation = decode::<InstanceLifecycleMutation>(&request.body)?;
                self.submit_lifecycle(
                    LifecycleRequest::Unload(instance_request(
                        mutation.expected_generation,
                        instance_id,
                    )),
                    OperationKind::UnloadInstance,
                    mutation.expected_generation,
                    instance_id,
                    peer,
                )
            }
            _ => Err(not_found()),
        }
    }

    fn snapshot(&self) -> Result<LocalResponse, ApiError> {
        let mut snapshot = self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .refresh_snapshot()
            .map_err(|_| backend("host inventory is unavailable"))?;
        snapshot.operations = self.registry.operations();
        result_response(snapshot.generation, snapshot)
    }

    fn instance(&self, id: InstanceId) -> Result<LocalResponse, ApiError> {
        let snapshot = self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .refresh_snapshot()
            .map_err(|_| backend("host inventory is unavailable"))?;
        let instance = snapshot
            .instances
            .into_iter()
            .find(|instance| instance.id == id)
            .ok_or_else(not_found)?;
        result_response(snapshot.generation, instance)
    }

    fn operations(&self) -> Result<LocalResponse, ApiError> {
        result_response(
            self.registry.latest_generation(),
            self.registry.operations(),
        )
    }

    fn operation(&self, id: &OperationId) -> Result<LocalResponse, ApiError> {
        let operation = self.registry.get(id).ok_or_else(not_found)?;
        result_response(self.registry.latest_generation(), operation)
    }

    fn events(&self, query: Option<&str>) -> Result<LocalResponse, ApiError> {
        let cursor = parse_cursor(query)?;
        let batch = self.registry.events_after(cursor);
        result_response(
            self.registry.latest_generation(),
            EventPage {
                events: batch.events,
                overflowed: batch.overflowed,
                latest_sequence: batch.latest_sequence,
            },
        )
    }

    fn cancel(&self, id: &OperationId) -> Result<LocalResponse, ApiError> {
        self.scheduler.cancel(id).map_err(registry_error)?;
        let operation = self.registry.get(id).ok_or_else(not_found)?;
        accepted_response(operation)
    }

    fn submit_lifecycle(
        &self,
        request: LifecycleRequest,
        kind: OperationKind,
        expected_generation: Generation,
        instance_id: InstanceId,
        peer: &PeerIdentity,
    ) -> Result<LocalResponse, ApiError> {
        let permit = self.limiter.acquire(LimitKind::Mutation)?;
        let executor = Arc::clone(&self.lifecycle);
        let operation = self
            .scheduler
            .submit(
                new_operation(
                    kind,
                    expected_generation,
                    ResourceReference {
                        kind: ResourceKind::Instance,
                        id: instance_id.0.to_string(),
                    },
                    peer,
                ),
                move |cancellation| {
                    let _permit = permit;
                    run_lifecycle(&executor, &request, &cancellation)
                },
            )
            .map_err(schedule_error)?;
        accepted_response(operation)
    }

    fn submit_resource_pool(
        &self,
        mutation: ResourcePoolMutation,
        peer: &PeerIdentity,
    ) -> Result<LocalResponse, ApiError> {
        let permit = self.limiter.acquire(LimitKind::Mutation)?;
        let executor = Arc::clone(&self.resource_pool);
        let expected_generation = mutation.expected_generation;
        let operation_kind = if mutation.cpu_hardware_ids.is_empty() && mutation.memory_bytes == 0 {
            OperationKind::ReleaseResourcePool
        } else {
            OperationKind::InitializeResourcePool
        };
        let operation = self
            .scheduler
            .submit(
                new_operation(
                    operation_kind,
                    expected_generation,
                    ResourceReference {
                        kind: ResourceKind::ResourcePool,
                        id: "host".into(),
                    },
                    peer,
                ),
                move |cancellation| {
                    let _permit = permit;
                    if cancellation.is_cancelled() {
                        return OperationTaskResult::cancelled(None);
                    }
                    let mut executor = executor
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if cancellation.is_cancelled() {
                        return OperationTaskResult::cancelled(None);
                    }
                    resource_pool_task_result(executor.execute(&ResourcePoolRequest {
                        expected_generation,
                        cpu_hardware_ids: mutation.cpu_hardware_ids,
                        memory_bytes: mutation.memory_bytes,
                    }))
                },
            )
            .map_err(schedule_error)?;
        accepted_response(operation)
    }
}

fn run_lifecycle<R, S>(
    executor: &Arc<Mutex<LifecycleExecutor<R, S>>>,
    request: &LifecycleRequest,
    cancellation: &CancellationToken,
) -> OperationTaskResult
where
    R: KerfRunner,
    S: SnapshotRefresher,
{
    if cancellation.is_cancelled() {
        return OperationTaskResult::cancelled(None);
    }
    let mut executor = executor
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cancellation.is_cancelled() {
        return OperationTaskResult::cancelled(None);
    }
    lifecycle_task_result(executor.execute(request))
}

fn route_for(method: &Method, path: &str) -> Option<RouteSecurity> {
    let read = RouteSecurity {
        class: RequestClass::ReadOnly,
        audit_action: AuditAction::ReadInventory,
    };
    let mutation = RouteSecurity {
        class: RequestClass::Mutation,
        audit_action: AuditAction::MutateLifecycle,
    };
    match (method, path) {
        (&Method::GET, "/1.0" | "/1.0/operations" | "/1.0/events") => Some(read),
        (&Method::PUT, "/1.0/resource-pool") => Some(RouteSecurity {
            class: RequestClass::Mutation,
            audit_action: AuditAction::MutateResourcePool,
        }),
        (&Method::POST, "/1.0/instances") => Some(mutation),
        _ if operation_id_from_path(path).is_some() => match *method {
            Method::GET => Some(read),
            Method::DELETE => Some(RouteSecurity {
                class: RequestClass::Mutation,
                audit_action: AuditAction::CancelOperation,
            }),
            _ => None,
        },
        _ => {
            let (_, action) = instance_resource(path)?;
            match (method, action) {
                (&Method::GET | &Method::PATCH | &Method::DELETE, None) => {
                    Some(if method == Method::GET {
                        read
                    } else {
                        mutation
                    })
                }
                (&Method::POST, Some(_)) => Some(mutation),
                _ => None,
            }
        }
    }
}

fn operation_id_from_path(path: &str) -> Option<OperationId> {
    let suffix = path.strip_prefix("/1.0/operations/")?;
    (!suffix.is_empty() && !suffix.contains('/')).then(|| OperationId(suffix.into()))
}

fn instance_resource(path: &str) -> Option<(InstanceId, Option<InstanceAction>)> {
    let suffix = path.strip_prefix("/1.0/instances/")?;
    let mut segments = suffix.split('/');
    let id = segments.next()?.parse::<u32>().ok().map(InstanceId)?;
    let action = match segments.next() {
        None => None,
        Some("load") => Some(InstanceAction::Load),
        Some("start") => Some(InstanceAction::Start),
        Some("stop") => Some(InstanceAction::Stop),
        Some("unload") => Some(InstanceAction::Unload),
        Some(_) => return None,
    };
    if segments.next().is_some() {
        return None;
    }
    Some((id, action))
}

fn parse_cursor(query: Option<&str>) -> Result<EventSequence, ApiError> {
    let Some(query) = query else {
        return Ok(EventSequence(0));
    };
    let value = query
        .strip_prefix("after=")
        .filter(|value| !value.is_empty() && !value.contains('&'))
        .ok_or_else(|| invalid("event cursor query is invalid"))?;
    value
        .parse::<u64>()
        .map(EventSequence)
        .map_err(|_| invalid("event cursor is invalid"))
}

fn validate_command_line(command_line: Option<&str>) -> Result<(), ApiError> {
    if command_line.is_some_and(|line| line.len() > MAX_COMMAND_LINE_BYTES || line.contains('\0')) {
        return Err(invalid("kernel command line is invalid"));
    }
    Ok(())
}

fn decode<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|_| invalid("request JSON is invalid"))
}

fn instance_request(expected_generation: Generation, id: InstanceId) -> InstanceRequest {
    InstanceRequest {
        expected_generation,
        id,
    }
}

fn new_operation(
    kind: OperationKind,
    expected_generation: Generation,
    resource: ResourceReference,
    peer: &PeerIdentity,
) -> NewOperation {
    NewOperation {
        kind,
        expected_generation,
        affected_resources: vec![resource],
        actor: Some(peer.actor.clone()),
        audit_id: None,
        created_at: timestamp(),
    }
}

fn timestamp() -> String {
    jiff::Timestamp::now().to_string()
}

fn result_response<T: serde::Serialize>(
    generation: Generation,
    data: T,
) -> Result<LocalResponse, ApiError> {
    LocalResponse::json(StatusCode::OK, &Response::Result { generation, data })
}

fn accepted_response(operation: Operation) -> Result<LocalResponse, ApiError> {
    LocalResponse::json(
        StatusCode::ACCEPTED,
        &Response::<()>::Accepted { operation },
    )
}

fn invalid(message: &str) -> ApiError {
    error(ErrorCode::InvalidRequest, message, false)
}

fn not_found() -> ApiError {
    error(ErrorCode::NotFound, "resource was not found", false)
}

fn backend(message: &str) -> ApiError {
    error(ErrorCode::BackendUnavailable, message, true)
}

fn error(code: ErrorCode, message: &str, retryable: bool) -> ApiError {
    ApiError {
        code,
        message: message.into(),
        retryable,
        current_generation: None,
        diagnostics: Vec::new(),
    }
}

fn registry_error(registry_error: OperationRegistryError) -> ApiError {
    match registry_error {
        OperationRegistryError::NotFound => not_found(),
        OperationRegistryError::InvalidTransition | OperationRegistryError::InvalidProgress => {
            error(
                ErrorCode::Conflict,
                "operation cannot be changed in its current state",
                false,
            )
        }
        OperationRegistryError::InvalidCapacity => error(
            ErrorCode::Internal,
            "operation registry is unavailable",
            false,
        ),
    }
}

fn schedule_error(_error: ScheduleError) -> ApiError {
    error(
        ErrorCode::BackendUnavailable,
        "operation worker could not be scheduled",
        true,
    )
}

/// Failure to assemble the running-host API.
#[derive(Debug)]
pub enum HostApiBuildError {
    Configuration(ApiError),
    Registry(OperationRegistryError),
    CurrentExecutable(std::io::Error),
}

impl std::fmt::Display for HostApiBuildError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Configuration(error) => error.message.fmt(formatter),
            Self::Registry(_) => formatter.write_str("operation registry configuration is invalid"),
            Self::CurrentExecutable(error) => {
                write!(formatter, "failed to resolve service executable: {error}")
            }
        }
    }
}

impl std::error::Error for HostApiBuildError {}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn routes_only_versioned_typed_resources() {
        assert_eq!(
            route_for(&Method::GET, "/1.0").unwrap().class,
            RequestClass::ReadOnly
        );
        assert_eq!(
            route_for(&Method::POST, "/1.0/instances/1/start")
                .unwrap()
                .class,
            RequestClass::Mutation
        );
        assert!(route_for(&Method::POST, "/1.0/instances/1/unknown").is_none());
        assert!(route_for(&Method::GET, "/2.0").is_none());
        assert!(route_for(&Method::POST, "/1.0/instances/1/start/extra").is_none());
    }

    #[test]
    fn image_policy_blocks_relative_escape_and_non_files() {
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let kernel = root.path().join("vmlinux");
        let escaped = outside.path().join("outside");
        fs::write(&kernel, b"kernel").unwrap();
        fs::write(&escaped, b"outside").unwrap();
        let policy = ImagePolicy::new([root.path().to_path_buf()]).unwrap();

        assert_eq!(policy.resolve(kernel.to_str().unwrap()).unwrap(), kernel);
        assert_eq!(
            policy.resolve(escaped.to_str().unwrap()).unwrap_err().code,
            ErrorCode::Forbidden
        );
        assert!(policy.resolve("relative/vmlinux").is_err());
        assert!(policy.resolve(root.path().to_str().unwrap()).is_err());
    }

    #[test]
    fn parses_event_cursor_strictly() {
        assert_eq!(parse_cursor(None).unwrap(), EventSequence(0));
        assert_eq!(parse_cursor(Some("after=42")).unwrap(), EventSequence(42));
        assert!(parse_cursor(Some("cursor=42")).is_err());
        assert!(parse_cursor(Some("after=1&after=2")).is_err());
    }

    #[test]
    fn command_line_is_bounded_and_rejects_nul() {
        assert!(validate_command_line(Some("console=mktty0")).is_ok());
        assert!(validate_command_line(Some("bad\0value")).is_err());
        assert!(validate_command_line(Some(&"x".repeat(MAX_COMMAND_LINE_BYTES + 1))).is_err());
    }
}
