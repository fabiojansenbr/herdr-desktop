//! Spec 030's explicit read-only probe against the measured remote host.
//!
//! Run intentionally, never as part of the normal suite:
//! `cargo test -p herdr-desktop --test spec_030_ssh spec_030_real_read_only_remote_probe -- --ignored --nocapture --test-threads=1`
//!
//! The profile store is temporary. The only remote operations are the same client bridge
//! attachment used by the desktop and the read-only `workspace.list` query.

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use herdr_client::SurfaceGeometry;
use herdr_desktop::bridge::ssh::{OpenSshRunner, SshRunner};
use herdr_desktop::connections::commands::{ConnectionsConfig, ConnectionsState};
use herdr_desktop::connections::profiles::SshProfileDraft;
use herdr_desktop::connections::remote_binary::known_binary_candidate_script;
use herdr_desktop::connections::ssh_options::{
    build_ssh_script, IsolatedSshConfig, ProfileId, SshIdentity,
};

const TARGET: &str = "ec2-user@mac-mini";
const SESSION: &str = "default";
const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(70);

fn elapsed(start: Instant) -> u128 {
    start.elapsed().as_millis()
}

#[test]
#[ignore = "real host read-only probe for TASK-030-01/TASK-030-04"]
fn spec_030_real_read_only_remote_probe() {
    let started = Instant::now();
    let prefs = tempfile::tempdir().expect("temporary desktop preferences");
    let herdr_config = tempfile::tempdir().expect("temporary Herdr config path");
    let herdr_state = tempfile::tempdir().expect("temporary Herdr state path");
    let isolated_ssh = IsolatedSshConfig {
        identity_file: PathBuf::from("/home/user/.ssh/id_ed25519"),
        user_known_hosts_file: PathBuf::from("/home/user/.ssh/known_hosts"),
    };
    let identity = SshIdentity::new(
        ProfileId::parse("03003003003003003003003003003003").unwrap(),
        TARGET,
        None,
        SESSION,
    )
    .unwrap();
    let discovery_script = known_binary_candidate_script();
    let discovery = OpenSshRunner.output(
        &build_ssh_script(&identity, Some(&isolated_ssh), &discovery_script),
        Duration::from_secs(15),
        Some(discovery_script.as_bytes()),
    );
    match &discovery {
        Ok(output) => eprintln!(
            "[030 probe +{}ms] diagnóstico descoberta: status={:?} stdout={:?} stderr={:?}",
            elapsed(started),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            output.stderr
        ),
        Err(error) => eprintln!(
            "[030 probe +{}ms] diagnóstico descoberta: erro={error}",
            elapsed(started)
        ),
    }
    let state = ConnectionsState::new(ConnectionsConfig {
        prefs_dir: prefs.path().to_path_buf(),
        herdr_config_dir: herdr_config.path().to_path_buf(),
        herdr_state_dir: herdr_state.path().to_path_buf(),
        local_session: None,
        local_auto_start: false,
        herdr_bin: PathBuf::from("herdr"),
        // The current host's system-wide ssh_config is owned by the container user and OpenSSH
        // rejects it before resolving aliases. This is the same key/known_hosts pair used by the
        // TUI, with config files disabled only for this explicit probe.
        isolated_ssh: Some(isolated_ssh),
        geometry: SurfaceGeometry {
            cols: 100,
            rows: 30,
            cell_width_px: 9,
            cell_height_px: 18,
        },
    });

    eprintln!(
        "[030 probe +{}ms] início: alvo={TARGET} sessão={SESSION}; somente leitura",
        elapsed(started)
    );
    let view = state
        .save_profile(
            SshProfileDraft {
                id: None,
                label: "spec-030-probe".into(),
                target: TARGET.into(),
                port: None,
                session: SESSION.into(),
                auth: None,
            },
            true,
        )
        .unwrap_or_else(|error| {
            panic!(
                "[030 probe +{}ms] salvar/iniciar perfil: {error:?}",
                elapsed(started)
            )
        });
    let endpoint = view
        .profiles
        .last()
        .expect("saved probe profile")
        .id
        .as_str()
        .to_owned();
    eprintln!(
        "[030 probe +{}ms] passo 1 solicitado: endpoint={endpoint}",
        elapsed(started)
    );

    let deadline = started + OBSERVATION_TIMEOUT;
    let mut last_signature = String::new();
    let mut online = false;
    while Instant::now() < deadline {
        let view = state.view();
        let host = view
            .hub
            .hosts
            .iter()
            .find(|host| host.endpoint == endpoint)
            .expect("probe host remains registered");
        let signature = format!(
            "phase={:?} attempt={} latency={:?} version={:?} generation={:?} error={:?}",
            host.phase,
            host.attempt,
            host.latency_ms,
            host.server_version,
            host.generation,
            host.connection_error
                .as_ref()
                .map(|error| (&error.code, &error.message)),
        );
        if signature != last_signature {
            eprintln!(
                "[030 probe +{}ms] passo 2 conexão: {signature}",
                elapsed(started)
            );
            last_signature = signature;
        }
        if matches!(
            host.phase,
            herdr_desktop::connections::state::LinkPhase::Online
        ) {
            online = true;
            break;
        }
        if matches!(
            host.phase,
            herdr_desktop::connections::state::LinkPhase::Attention
        ) {
            break;
        }
        if host.connection_error.is_some()
            && matches!(
                host.phase,
                herdr_desktop::connections::state::LinkPhase::Offline
            )
        {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }

    if !online {
        state.detach_all();
        panic!("[030 probe +{}ms] parou antes de Online", elapsed(started));
    }
    eprintln!(
        "[030 probe +{}ms] passo 3 handshake/endpoint: Online; iniciando workspace.list",
        elapsed(started)
    );
    let workspace_started = Instant::now();
    let workspace_result = state.workspaces(&endpoint);
    state.detach_all();
    match workspace_result {
        Ok(workspaces) => {
            eprintln!(
                "[030 probe +{}ms] passo 4 workspaces: {} itens em {}ms: {:?}",
                elapsed(started),
                workspaces.len(),
                workspace_started.elapsed().as_millis(),
                workspaces
                    .iter()
                    .map(|workspace| (&workspace.workspace_id, &workspace.label))
                    .collect::<Vec<_>>()
            );
        }
        Err(error) => {
            eprintln!(
                "[030 probe +{}ms] passo 4 workspaces: ERRO após {}ms: code={} message={}",
                elapsed(started),
                workspace_started.elapsed().as_millis(),
                error.code,
                error.message
            );
            panic!("[030 probe] leitura de workspaces parou no hub: {error:?}");
        }
    }
    eprintln!(
        "[030 probe +{}ms] fim: cliente SSH destacado; servidor/sessão remotos não alterados",
        elapsed(started)
    );
}
