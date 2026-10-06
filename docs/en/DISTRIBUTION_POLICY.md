# Releases and distribution

English · [简体中文](../zh-CN/DISTRIBUTION_POLICY.md) · [日本語](../ja/DISTRIBUTION_POLICY.md)

How Kumi and its bridge reach people, what that does and doesn't prove, and
what the bridge's package may contain. The steps for cutting a release are in
[the developer guide](DEVELOPER_GUIDE.md#releasing).

## Channels

| What | Where | How people get it |
| --- | --- | --- |
| Kumi | GitHub Releases of `user1303836/kumi`: native `kumi-<target>.tar.gz` bundles, the compatibility `kumi.tar.gz`, `kumi-release.json` and `SHA256SUMS`, attached to each `vX.Y.Z` tag by the Installer workflow | `install.sh` or `install.ps1`, then `kumi update` |
| The bridge (`@ableton-mcp/mcp-server`) | Inside each Kumi bundle, as a native bridge tarball and its prepared package | `kumi bridge`, which installs it through the bridge's lifecycle ([delivery](DELIVERY.md)) |
| The bridge on its own | No release of its own. Build it with `python3 scripts/build-native-release.py --bridge-only` ([build options](DEVELOPER_GUIDE.md#releasing)) | The lifecycle CLI ([delivery](DELIVERY.md#the-standalone-bridge)) |

The installer scripts are read from the `main` branch; the bundle they install
comes from the latest published release (or the one `KUMI_VERSION` names). A
release is a draft until the maintainer publishes it, and only a published
release is "latest". Nothing is published to npm: every package is
`private: true`, so `npm publish` refuses.

## Integrity, not identity

The app and bridge have no publisher signature or notarization, and there are
no `.pkg` or `.msi` installers. The macOS Hands helper is ad-hoc signed; that
does not establish publisher identity. The installer and `kumi update` check
the bundle against the sha256 in `kumi-release.json`. Fresh native installs
do not download Node. `kumi bridge` checks the bridge tarball against the hash
recorded when the bundle was built. A checksum from the same place as the
download proves the bytes arrived intact, not who made them.

The software is [MIT licensed](../../LICENSE.md). The licence grants no rights
to Ableton's trademarks, and Kumi isn't affiliated with or endorsed by Ableton.

## What the bridge's package may contain

- native `ableton-mcp-server` and `ableton-mcp-analysis-worker` executables
  (`.exe` on Windows);
- the Remote Script, its README, the operation registry and their hash
  manifest;
- Kumi's Live extension: its manifest, `package.json`, built `extension.js` and
  that file's sha256;
- the bridge's guides (`README.md` and `release-docs/`);
- `release-manifest.json`, `package.json` and `LICENSE.md`.

Nothing else: no build scripts, test fixtures, `node_modules`, credentials,
configuration, local state, logs, captured media or evidence. The native
producer and lifecycle enforce the exact file inventory and hashes in
`release-manifest.json`.

## The release manifest

`release-manifest.json` (schema `ableton-mcp-native-release/v1`) records the
package name and version, source commit and dirty state, Rust target, rustc
and Cargo versions, runner image, SHA-256 of `Cargo.lock` and the CI workflow,
build recipe, protocol registry hash, and each payload file's role and SHA-256.

Its distribution fields are `channel: "local-native-tarball"`, with
`published`, `signed`, `notarized` and `integrityIsIdentityProof` all `false`.
The lifecycle requires these values. The tarball is installed from a local
path by its hash and reaches users inside the Kumi bundle on GitHub Releases;
it is not published to a package registry.

For existing installations, the lifecycle also accepts the legacy
`ableton-mcp-release/v2` and `ableton-mcp-private-release/v1` schemas for
upgrade and rollback. Their Node/npm/TypeScript build evidence and
`local-npm-tarball` channel describe the bridge packages of Kumi 1.7.5 and
earlier.

## Merge gate

The repository has two rulesets:

- **`main`:**
  - changes arrive by pull request; no approving review is required;
  - two required checks, `Required CI` and `Willington files`, which must pass
    on the branch as it is up to date with `main`;
  - `main` can't be deleted or force-pushed;
  - the repository admin role can bypass these rules for pull requests.
- **`Release tags`:** only the repository admin role creates, moves or deletes
  `v*` tags. A tag push runs the Installer, which publishes the release.

`Willington files` lets only the repository owner's pull requests, from a
branch in this repository, change `vendor/willington/`, and such a pull request
changes nothing else. Those files ship to every producer, and their native
libraries can't be reviewed, so who sends them is the check. The
[developer guide](DEVELOPER_GUIDE.md#willingtons-files) describes an update.

The Installer workflow isn't a required check, but on a tag its `publish` job
runs only after the bundle has installed on macOS, Linux and Windows.
[Testing](TESTING.md#ci) describes every job.

## Open owner decisions

- **Signing and notarization** of Kumi's bundle and installers on macOS and
  Windows.
- **Redistributing the Extensions SDK.** Kumi's Live extension is built from a
  locally supplied pre-release Ableton Extensions SDK, which the repository
  never commits because its licence restricts redistributing the SDK. The built
  `extension.js` bundles the extension with the SDK code it uses, and it is
  committed and shipped in the bridge's package and the Kumi bundle. Whether
  that is allowed is for the owner to settle.
- **The admin bypass** on the `main` ruleset: keep it, or remove it.
