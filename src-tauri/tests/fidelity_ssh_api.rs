//! Spec 007 — response parsing of the SSH JSON API lane (`bridge::ssh::SshApiLane`).
//!
//! Engine bodies below are the raw lines `herdr-03749ae remote-api-bridge` returned in a private
//! namespace (evidencias/007/native-ssh-live/raw-api-bridge.txt): a request the engine cannot
//! deserialize (missing required field, unknown method) is answered with `"id": ""`, before the
//! request id is known. The lane must keep that error (the agents capability probe depends on
//! `invalid_request`) and mark the id divergence exactly like the Local `ApiClient`, while a
//! success with a foreign id is never accepted. No engine, SSH or GUI here.

#![cfg(unix)]

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use herdr_client::api::ApiClient;
use herdr_client::RuntimeError;
use herdr_desktop::bridge::ssh::{ProcessOutput, SshChild, SshConnector, SshRunner};
use herdr_desktop::connections::hub::ApiLane;
use herdr_desktop::connections::ssh_options::{OpenSshCommand, ProfileId, SshIdentity};
use serde_json::{json, Value};

/// Raw engine replies, parameterized by the request id so success/error with the right id can be
/// built too (bodies that ignore it keep the engine's literal id).
type Body = fn(&str) -> String;

const MISSING_TARGET: &str = r#"{"id":"","error":{"code":"invalid_request","message":"invalid request: missing field `target` at line 1 column 55"}}"#;
const UNKNOWN_METHOD: &str = r#"{"id":"","error":{"code":"invalid_request","message":"invalid request: unknown variant `no.such_method`, expected one of `ping`, `server.stop`"}}"#;

/// Fake OpenSSH runner: answers the one API bridge request with `body(request id)`.
struct BodyRunner {
    body: Body,
    seen: Mutex<Vec<Value>>,
}

impl SshRunner for BodyRunner {
    fn output(
        &self,
        _command: &OpenSshCommand,
        _timeout: Duration,
        stdin: Option<&[u8]>,
    ) -> io::Result<ProcessOutput> {
        let request: Value = serde_json::from_slice(stdin.expect("request line")).unwrap();
        let body = (self.body)(request["id"].as_str().unwrap());
        self.seen.lock().unwrap().push(request);
        Ok(ProcessOutput {
            status: Some(0),
            stdout: format!("{body}\n").into_bytes(),
            stderr: String::new(),
        })
    }
    fn spawn(&self, _command: &OpenSshCommand) -> io::Result<Box<dyn SshChild>> {
        unreachable!("the API lane never spawns a long-lived child")
    }
}

fn lane(body: Body) -> (impl ApiLane, Arc<BodyRunner>) {
    let runner = Arc::new(BodyRunner {
        body,
        seen: Mutex::new(Vec::new()),
    });
    let identity = SshIdentity::new(
        ProfileId::parse("0f3c2a107d1e4b8a9c55aa0000000007").unwrap(),
        "dev@203.0.113.7",
        Some(2222),
        "hd007-remote-api",
    )
    .unwrap();
    (
        SshConnector::new(identity, None, runner.clone()).api_lane(),
        runner,
    )
}

/// The same body through the Local `ApiClient` over a real Unix socket (the reference contract).
fn local(body: Body, method: &str) -> Result<Value, RuntimeError> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("api.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        let mut stream = stream;
        writeln!(stream, "{}", body(request["id"].as_str().unwrap())).unwrap();
    });
    let result = ApiClient::new(&path, "local").request(method, json!({}));
    server.join().unwrap();
    result
}

fn code(result: &Result<Value, RuntimeError>) -> String {
    match result {
        Ok(_) => "ok".into(),
        Err(e) => e.code.clone(),
    }
}

// Would catch: the lane rejecting the id before reading `error`, so the probe of agent.get/start/
// prompt with `{}` fails discovery with a generic response_id_mismatch (run1 hosts-identity).
#[test]
fn engine_invalid_request_with_empty_id_keeps_its_code_and_message() {
    let (lane, runner) = lane(|_| MISSING_TARGET.into());
    let error = lane.request("agent.get", json!({})).unwrap_err();
    assert_eq!(error.code, "invalid_request:id_mismatch");
    assert!(
        error.message.contains("missing field `target`"),
        "{error:?}"
    );
    assert!(error.code.starts_with("invalid_request"));
    assert_eq!(runner.seen.lock().unwrap()[0]["method"], "agent.get");
}

// Would catch: an unknown method reported as a generic mismatch, so an absent capability could not
// be told apart from a transport failure (the probe needs "unknown variant").
#[test]
fn unknown_method_error_is_preserved_for_capability_probe() {
    let (lane, _) = lane(|_| UNKNOWN_METHOD.into());
    let error = lane.request("agent.prompt", json!({})).unwrap_err();
    assert_eq!(error.code, "invalid_request:id_mismatch");
    assert!(error.message.contains("unknown variant"), "{error:?}");
}

// Would catch: an error with id null treated as the engine's empty-id refusal or a foreign request's
// error passed on with the exact code,
// as if it answered this request; or a foreign SUCCESS accepted as this action's result.
#[test]
fn foreign_ids_never_look_like_this_request() {
    let (null_id, _) =
        lane(|_| r#"{"id":null,"error":{"code":"invalid_request","message":"bad"}}"#.into());
    // Not a wire response at all (the Local contract requires a string id): never the engine's
    // `"id": ""` refusal, never an error of this request.
    assert_eq!(
        code(&null_id.request("tab.list", json!({}))),
        "protocol_error"
    );
    let (foreign_error, _) = lane(|_| {
        r#"{"id":"desktop-ssh:999999","error":{"code":"workspace_not_found","message":"no"}}"#
            .into()
    });
    assert_eq!(
        code(&foreign_error.request("workspace.focus", json!({}))),
        "workspace_not_found:id_mismatch"
    );
    let (foreign_success, _) = lane(|_| {
        r#"{"id":"desktop-ssh:999999","result":{"type":"agent_list","agents":[]}}"#.into()
    });
    assert_eq!(
        code(&foreign_success.request("agent.list", json!({}))),
        "response_id_mismatch"
    );
    let (empty_success, _) = lane(|_| r#"{"id":"","result":{"type":"tab_list","tabs":[]}}"#.into());
    assert_eq!(
        code(&empty_success.request("tab.list", json!({}))),
        "response_id_mismatch"
    );
}

// Would catch: a regression of the correlated paths (echoed id success/error).
#[test]
fn echoed_id_success_and_error_are_exact() {
    let (ok, _) = lane(|id| format!(r#"{{"id":"{id}","result":{{"type":"tab_list","tabs":[]}}}}"#));
    assert_eq!(
        ok.request("tab.list", json!({})).unwrap()["type"],
        "tab_list"
    );
    let (err, _) = lane(|id| {
        format!(r#"{{"id":"{id}","error":{{"code":"agent_not_found","message":"no agent"}}}}"#)
    });
    let error = err
        .request("agent.get", json!({ "target": "w1:p1" }))
        .unwrap_err();
    assert_eq!(
        (error.code.as_str(), error.message.as_str()),
        ("agent_not_found", "no agent")
    );
}

// Would catch: the SSH lane and the Local client diverging on the same engine reply (a probe
// that works on Local and fails on SSH, or the reverse).
#[test]
fn ssh_lane_matches_local_api_client_on_the_same_bodies() {
    let bodies: [(&str, Body); 6] = [
        ("agent.get", |_| MISSING_TARGET.into()),
        ("no.such_method", |_| UNKNOWN_METHOD.into()),
        ("agent.list", |_| r#"{"id":"other","result":{}}"#.into()),
        ("tab.list", |_| {
            r#"{"id":null,"error":{"code":"invalid_request","message":"bad"}}"#.into()
        }),
        ("tab.list", |id| {
            format!(r#"{{"id":"{id}","result":{{"tabs":[]}}}}"#)
        }),
        ("agent.get", |id| {
            format!(r#"{{"id":"{id}","error":{{"code":"agent_not_found","message":"no"}}}}"#)
        }),
    ];
    for (method, body) in bodies {
        let (ssh, _) = lane(body);
        assert_eq!(
            code(&ssh.request(method, json!({}))),
            code(&local(body, method)),
            "{method}: {}",
            body("desktop:1")
        );
    }
}
