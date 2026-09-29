# Releasing

A release is a tag. Everything else — building, packaging, publishing the artifacts — is done
by the release workflow that runs on that tag.

## Version numbers

The version is written in three files and they must always agree:

| File | Field |
|---|---|
| [`package.json`](../package.json) | `"version"` |
| [`src-tauri/Cargo.toml`](../src-tauri/Cargo.toml) | `[package] version` |
| [`src-tauri/tauri.conf.json`](../src-tauri/tauri.conf.json) | `"version"` |

`src-tauri/tauri.conf.json` is the one that ends up in the installed package metadata, so a
mismatch is visible to users. Check them together:

```bash
grep '"version"' package.json src-tauri/tauri.conf.json
grep '^version' src-tauri/Cargo.toml
```

The workspace crates (`crates/herdr-client`, `vendor/herdr-protocol`) carry their own
versions; they are not published to crates.io (`publish = false`) and do not have to follow
the application's version.

Versioning is [semantic](https://semver.org): `MAJOR.MINOR.PATCH`, and while the app is
pre-1.0 a breaking change bumps `MINOR`.

## Cutting a release

1. Start from a clean tree on the default branch, with the full gate green:

   ```bash
   mise exec -- bunx vitest run
   mise exec -- bun run check
   mise exec -- cargo nextest run --workspace
   mise exec -- cargo clippy --workspace --all-targets -- -D warnings
   mise exec -- cargo fmt --all -- --check
   ```

2. Bump the version in the three files above. `cargo check` once afterwards so `Cargo.lock`
   records the new version, and commit both.

3. Tag it and push the tag:

   ```bash
   git tag -a vX.Y.Z -m "vX.Y.Z"
   git push origin vX.Y.Z
   ```

   The tag name is `v` followed by the exact version in the three files. The release workflow
   is triggered by tags of that shape and by nothing else.

4. The workflow builds the app on each platform runner and uploads the packages to a GitHub
   release for the tag. Watch it in the repository's Actions tab; if a platform fails, fix it
   and move the tag rather than publishing a partial release.

5. Edit the release notes: what changed, which Herdr engine versions it was checked against,
   and which platforms are verified. Say plainly that the macOS and Windows artifacts are
   built but unverified.

## Artifacts

Each release carries the packages produced by `tauri build` for the bundle targets enabled in
[`src-tauri/tauri.conf.json`](../src-tauri/tauri.conf.json):

| Platform | Typical artifacts |
|---|---|
| Linux | `.deb`, `.rpm`, AppImage |
| macOS | `.dmg` / `.app` bundle |
| Windows | `.msi` / NSIS installer |

The exact set is whatever the bundle configuration lists at the time of the tag — read it
there rather than from this table.

Linux is the only platform the maintainers run. The macOS and Windows artifacts are produced
by the workflow and are not verified on those systems; that is stated in the README's
platform table and should stay in the release notes too.

## If a release has to be redone

Delete the tag locally and remotely, delete the draft or published release, fix the problem,
then tag again. A published version number is never reused for different bytes: if the
artifacts already reached anyone, cut `X.Y.Z+1` instead.
