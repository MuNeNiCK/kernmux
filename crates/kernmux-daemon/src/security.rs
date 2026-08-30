//! Local peer authentication, authorization, limits, and audit records.

use std::{
    collections::BTreeSet,
    os::fd::AsFd,
    os::unix::net::UnixStream,
    sync::{Arc, Mutex},
};

use kernmux_api::v1::{Actor, ApiError, ErrorCode, OperationId, ResourceReference};
use rustix::net::sockopt::socket_peercred;

/// Kernel-authenticated identity of one Unix socket peer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerIdentity {
    pub pid: i32,
    pub actor: Actor,
}

impl PeerIdentity {
    /// Reads credentials from any connected Unix socket descriptor.
    ///
    /// # Errors
    ///
    /// Returns a redacted backend error when credentials cannot be read.
    pub fn from_socket(socket: &impl AsFd) -> Result<Self, ApiError> {
        let credentials = socket_peercred(socket).map_err(|_| ApiError {
            code: ErrorCode::BackendUnavailable,
            message: "peer credentials are unavailable".into(),
            retryable: false,
            current_generation: None,
            diagnostics: Vec::new(),
        })?;
        Ok(Self {
            pid: credentials.pid.as_raw_nonzero().get(),
            actor: Actor {
                uid: credentials.uid.as_raw(),
                gid: credentials.gid.as_raw(),
                label: None,
            },
        })
    }

    /// Reads credentials fixed by the kernel when the socket connected.
    ///
    /// # Errors
    ///
    /// Returns a redacted backend error when credentials cannot be read.
    pub fn from_stream(stream: &UnixStream) -> Result<Self, ApiError> {
        Self::from_socket(stream)
    }
}

/// Security class of one API request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestClass {
    ReadOnly,
    Mutation,
    Console,
    Administration,
}

/// Effective local role derived exclusively from peer credentials.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Role {
    Reader,
    Operator,
    Administrator,
}

/// Explicit local authorization policy. Empty sets deny non-root peers.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AuthorizationPolicy {
    reader_uids: BTreeSet<u32>,
    reader_gids: BTreeSet<u32>,
    operator_uids: BTreeSet<u32>,
    operator_gids: BTreeSet<u32>,
    administrator_uids: BTreeSet<u32>,
    administrator_gids: BTreeSet<u32>,
    allow_unprivileged_reads: bool,
}

impl AuthorizationPolicy {
    /// Creates a default-deny policy in which only root is administrator.
    #[must_use]
    pub fn deny_by_default() -> Self {
        Self::default()
    }

    /// Allows any authenticated local peer to perform read-only requests.
    #[must_use]
    pub const fn with_unprivileged_reads(mut self, allowed: bool) -> Self {
        self.allow_unprivileged_reads = allowed;
        self
    }

    /// Adds a user to the reader role.
    #[must_use]
    pub fn with_reader_uid(mut self, uid: u32) -> Self {
        self.reader_uids.insert(uid);
        self
    }

    /// Adds a group to the reader role.
    #[must_use]
    pub fn with_reader_gid(mut self, gid: u32) -> Self {
        self.reader_gids.insert(gid);
        self
    }

    /// Adds a user to the operator role.
    #[must_use]
    pub fn with_operator_uid(mut self, uid: u32) -> Self {
        self.operator_uids.insert(uid);
        self
    }

    /// Adds a group to the operator role.
    #[must_use]
    pub fn with_operator_gid(mut self, gid: u32) -> Self {
        self.operator_gids.insert(gid);
        self
    }

    /// Adds a user to the administrator role.
    #[must_use]
    pub fn with_administrator_uid(mut self, uid: u32) -> Self {
        self.administrator_uids.insert(uid);
        self
    }

    /// Adds a group to the administrator role.
    #[must_use]
    pub fn with_administrator_gid(mut self, gid: u32) -> Self {
        self.administrator_gids.insert(gid);
        self
    }

    /// Resolves the effective role and authorizes one request class.
    ///
    /// # Errors
    ///
    /// Returns `Forbidden` when an authenticated peer lacks the required role.
    pub fn authorize(&self, actor: &Actor, class: RequestClass) -> Result<Role, ApiError> {
        let role = self.role(actor);
        let allowed = match class {
            RequestClass::ReadOnly => role.is_some() || self.allow_unprivileged_reads,
            RequestClass::Mutation | RequestClass::Console => {
                role.is_some_and(|role| role >= Role::Operator)
            }
            RequestClass::Administration => role == Some(Role::Administrator),
        };
        if allowed {
            Ok(role.unwrap_or(Role::Reader))
        } else {
            Err(forbidden())
        }
    }

    fn role(&self, actor: &Actor) -> Option<Role> {
        if actor.uid == 0
            || self.administrator_uids.contains(&actor.uid)
            || self.administrator_gids.contains(&actor.gid)
        {
            Some(Role::Administrator)
        } else if self.operator_uids.contains(&actor.uid) || self.operator_gids.contains(&actor.gid)
        {
            Some(Role::Operator)
        } else if self.reader_uids.contains(&actor.uid) || self.reader_gids.contains(&actor.gid) {
            Some(Role::Reader)
        } else {
            None
        }
    }
}

fn forbidden() -> ApiError {
    ApiError {
        code: ErrorCode::Forbidden,
        message: "peer is not authorized for this request".into(),
        retryable: false,
        current_generation: None,
        diagnostics: Vec::new(),
    }
}

/// Independently bounded daemon resource.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LimitKind {
    Connection,
    Mutation,
    Console,
}

/// Configured concurrency limits for privileged service resources.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceLimits {
    pub connections: usize,
    pub mutations: usize,
    pub consoles: usize,
}

impl ServiceLimits {
    /// Validates that each limit can admit at least one request.
    ///
    /// # Errors
    ///
    /// Rejects zero limits as invalid service configuration.
    pub fn validate(self) -> Result<Self, ApiError> {
        if self.connections == 0 || self.mutations == 0 || self.consoles == 0 {
            return Err(ApiError {
                code: ErrorCode::Internal,
                message: "service concurrency limits are invalid".into(),
                retryable: false,
                current_generation: None,
                diagnostics: Vec::new(),
            });
        }
        Ok(self)
    }
}

/// Thread-safe concurrency limiter with RAII release.
#[derive(Clone, Debug)]
pub struct ServiceLimiter {
    shared: Arc<Mutex<LimitState>>,
    limits: ServiceLimits,
}

#[derive(Debug, Default)]
struct LimitState {
    connections: usize,
    mutations: usize,
    consoles: usize,
}

impl ServiceLimiter {
    /// Creates a limiter from validated limits.
    ///
    /// # Errors
    ///
    /// Rejects zero limits.
    pub fn new(limits: ServiceLimits) -> Result<Self, ApiError> {
        Ok(Self {
            shared: Arc::new(Mutex::new(LimitState::default())),
            limits: limits.validate()?,
        })
    }

    /// Acquires one bounded service resource.
    ///
    /// # Errors
    ///
    /// Returns a retryable error when the configured limit is reached.
    pub fn acquire(&self, kind: LimitKind) -> Result<ServicePermit, ApiError> {
        let mut state = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (current, limit) = match kind {
            LimitKind::Connection => (&mut state.connections, self.limits.connections),
            LimitKind::Mutation => (&mut state.mutations, self.limits.mutations),
            LimitKind::Console => (&mut state.consoles, self.limits.consoles),
        };
        if *current >= limit {
            return Err(ApiError {
                code: ErrorCode::BackendUnavailable,
                message: "service concurrency limit reached".into(),
                retryable: true,
                current_generation: None,
                diagnostics: Vec::new(),
            });
        }
        *current += 1;
        Ok(ServicePermit {
            shared: Arc::clone(&self.shared),
            kind,
        })
    }
}

/// Held admission slot returned by [`ServiceLimiter`].
#[derive(Debug)]
pub struct ServicePermit {
    shared: Arc<Mutex<LimitState>>,
    kind: LimitKind,
}

impl Drop for ServicePermit {
    fn drop(&mut self) {
        let mut state = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = match self.kind {
            LimitKind::Connection => &mut state.connections,
            LimitKind::Mutation => &mut state.mutations,
            LimitKind::Console => &mut state.consoles,
        };
        *current = current.saturating_sub(1);
    }
}

/// Audited class of daemon action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditAction {
    ReadInventory,
    ManageImages,
    MutateLifecycle,
    MutateResourcePool,
    CancelOperation,
    AttachConsole,
    ChangePolicy,
}

/// Authorization decision recorded for one API action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditDecision {
    Allowed,
    Denied,
    Failed,
}

/// Redacted structured audit event for one local API request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditEvent {
    pub actor: Actor,
    pub peer_pid: i32,
    pub action: AuditAction,
    pub resource: Option<ResourceReference>,
    pub decision: AuditDecision,
    pub operation_id: Option<OperationId>,
    pub audit_id: Option<String>,
}

impl AuditEvent {
    /// Emits stable structured fields without backend output or request data.
    pub fn emit(&self) {
        tracing::info!(
            audit = true,
            peer_uid = self.actor.uid,
            peer_gid = self.actor.gid,
            peer_pid = self.peer_pid,
            action = ?self.action,
            decision = ?self.decision,
            resource_kind = ?self.resource.as_ref().map(|resource| resource.kind),
            resource_id = self.resource.as_ref().map(|resource| resource.id.as_str()),
            operation_id = self.operation_id.as_ref().map(|id| id.0.as_str()),
            audit_id = self.audit_id.as_deref(),
            "privileged API audit"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream;

    use super::*;

    fn actor(uid: u32, gid: u32) -> Actor {
        Actor {
            uid,
            gid,
            label: None,
        }
    }

    #[test]
    fn peer_identity_comes_from_kernel_socket_credentials() {
        let (server, _client) = UnixStream::pair().unwrap();
        let identity = PeerIdentity::from_stream(&server).unwrap();

        assert_eq!(identity.actor.uid, rustix::process::getuid().as_raw());
        assert_eq!(identity.actor.gid, rustix::process::getgid().as_raw());
        assert!(identity.pid > 0);
    }

    #[test]
    fn default_policy_denies_unprivileged_bypass_and_root_is_administrator() {
        let policy = AuthorizationPolicy::deny_by_default();

        assert_eq!(
            policy
                .authorize(&actor(0, 0), RequestClass::Administration)
                .unwrap(),
            Role::Administrator
        );
        let error = policy
            .authorize(&actor(1000, 1000), RequestClass::ReadOnly)
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Forbidden);
    }

    #[test]
    fn configured_roles_have_only_their_required_authority() {
        let policy = AuthorizationPolicy::deny_by_default()
            .with_reader_gid(10)
            .with_operator_uid(20)
            .with_administrator_gid(30);

        assert_eq!(
            policy
                .authorize(&actor(11, 10), RequestClass::ReadOnly)
                .unwrap(),
            Role::Reader
        );
        assert!(
            policy
                .authorize(&actor(11, 10), RequestClass::Mutation)
                .is_err()
        );
        assert_eq!(
            policy
                .authorize(&actor(20, 11), RequestClass::Console)
                .unwrap(),
            Role::Operator
        );
        assert!(
            policy
                .authorize(&actor(20, 11), RequestClass::Administration)
                .is_err()
        );
        assert_eq!(
            policy
                .authorize(&actor(31, 30), RequestClass::Administration)
                .unwrap(),
            Role::Administrator
        );
    }

    #[test]
    fn optional_unprivileged_reads_never_grant_mutation() {
        let policy = AuthorizationPolicy::deny_by_default().with_unprivileged_reads(true);
        let peer = actor(1000, 1000);

        assert_eq!(
            policy.authorize(&peer, RequestClass::ReadOnly).unwrap(),
            Role::Reader
        );
        assert!(policy.authorize(&peer, RequestClass::Mutation).is_err());
    }

    #[test]
    fn permits_enforce_limits_and_release_on_drop() {
        let limiter = ServiceLimiter::new(ServiceLimits {
            connections: 1,
            mutations: 1,
            consoles: 1,
        })
        .unwrap();

        let permit = limiter.acquire(LimitKind::Mutation).unwrap();
        let error = limiter.acquire(LimitKind::Mutation).unwrap_err();
        assert_eq!(error.code, ErrorCode::BackendUnavailable);
        assert!(error.retryable);

        drop(permit);
        assert!(limiter.acquire(LimitKind::Mutation).is_ok());
    }
}
