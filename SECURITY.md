# Security Policy

## Supported versions

Herdr Desktop is pre-1.0. Only the latest release receives fixes.

## Reporting a vulnerability

Please do not open a public issue for a security problem.

Report it privately through GitHub's
[security advisories](https://github.com/fabiojansenbr/herdr-desktop/security/advisories/new)
for this repository. Include what you observed, the steps to reproduce it, the version of the
app and of the Herdr engine, and your operating system.

You can expect an acknowledgement within a week. Once a fix is released the advisory is
published, with credit to the reporter unless you prefer otherwise.

## Scope

This app is a client for a Herdr engine. Things worth reporting here:

- anything the WebView can reach beyond the published IPC command list;
- credentials, tokens or command lines leaking into the WebView, into logs or onto disk;
- a connection accepted without the endpoint, session, generation, boot id and pane checks
  passing;
- input or file content reaching the wrong host, pane or session.

Vulnerabilities in the Herdr engine itself belong to
[that project](https://github.com/herdrdev/herdr). SSH host-key and key-material handling is
OpenSSH's; this app reuses your existing configuration and never stores a password.
