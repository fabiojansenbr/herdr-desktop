//! Project gateways of the composed window (007): opening a project uses the connection the
//! window already holds for the project's own endpoint and session, never a new Local
//! connection, and authorizes the project root in that host's file provider.

use std::time::{Duration, Instant};

use herdr_client::{LiveIdentity, ProjectRef, RuntimeBinding, RuntimeError, RuntimeGateway};
use serde_json::json;

use super::selection::SelectionState;
use crate::connections::hub::{EndpointScope, HostKind};
use crate::files::local::FilesState;
use crate::files::sftp::RemoteRootAuthorizer;
use crate::project_store::ProjectGateways;

/// How long a focus waits for a workspace the engine just created to reach the host snapshot.
/// Nothing is sent while waiting.
pub const FOCUS_SNAPSHOT_WAIT: Duration = Duration::from_secs(3);

/// [`ProjectGateways`] over the window's hub.
pub struct HostedProjectGateways {
    selection: SelectionState,
    local_files: FilesState,
    remote_roots: RemoteRootAuthorizer,
}

impl HostedProjectGateways {
    /// `local_files` must be a clone of the managed Local `FilesState` (shared roots) and
    /// `remote_roots` the authorizer of the managed `RemoteFilesState`.
    pub fn new(
        selection: SelectionState,
        local_files: FilesState,
        remote_roots: RemoteRootAuthorizer,
    ) -> Self {
        Self {
            selection,
            local_files,
            remote_roots,
        }
    }
}

impl ProjectGateways for HostedProjectGateways {
    fn gateway(&self, project: &ProjectRef) -> Result<Box<dyn RuntimeGateway>, RuntimeError> {
        let endpoint = project.endpoint_profile_id.as_str();
        let spec = self.selection.connections().hub().spec(endpoint)?;
        if spec.session != project.session_name {
            return Err(RuntimeError::new(
                "target_session_mismatch",
                "the connected host uses another session; nothing was sent",
            )
            .with_endpoint(endpoint));
        }
        self.selection.gateway_for(endpoint)
    }

    fn focus_workspace(
        &self,
        project: &ProjectRef,
        binding: &RuntimeBinding,
    ) -> Result<(), RuntimeError> {
        let hub = self.selection.connections().hub();
        let identity = LiveIdentity {
            endpoint: project.endpoint_profile_id.clone(),
            session: project.session_name.clone(),
            connection_generation: binding.connection_generation,
            boot_id: binding.boot_id.clone(),
        };
        let scope = EndpointScope::Workspace(binding.workspace_id.clone());
        let params = json!({ "workspace_id": binding.workspace_id });
        let deadline = Instant::now() + FOCUS_SNAPSHOT_WAIT;
        loop {
            let revision = hub.revision();
            match hub.run_endpoint_scoped(&identity, &scope, "workspace.focus", params.clone()) {
                // Refused before sending: safe to try again once the snapshot changed.
                Err(error) if error.code == "workspace_not_in_snapshot" => {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(error);
                    }
                    hub.wait_changed(revision, deadline - now);
                }
                result => return result.map(|_| ()),
            }
        }
    }

    fn authorize_root(&self, project: &ProjectRef) -> Result<(), RuntimeError> {
        let endpoint = project.endpoint_profile_id.as_str();
        match self.selection.connections().hub().spec(endpoint)?.kind {
            HostKind::Local => self
                .local_files
                .authorize_root(&project.root)
                .map(|_| ())
                .map_err(|error| error.with_endpoint(endpoint)),
            HostKind::Ssh => self.remote_roots.authorize_root(endpoint, &project.root),
        }
    }
}
