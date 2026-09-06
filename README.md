# apple-ship

Ship macOS **GPUI**, **Tauri**, and **native Xcode** apps from GitHub Actions.

Packaging never runs on your laptop. Locally you only ingest certificates and
dispatch the workflow. Signing, notarization, and artifacts happen on a
`macos-*` runner.

Two channels:

| Channel | Certificate | Result |
|---|---|---|
| `developer-id` | Developer ID Application | Notarized `.dmg` → GitHub Release |
| `app-store` | Apple Distribution | Local `.pkg` (not implemented in v0.1) |

v0.1 ships **Developer ID** for all three app kinds. App Store secrets can be
stored by `setup`; `ci` will refuse that channel until the next phase.

## Install

```bash
cargo install apple-ship --git https://github.com/meritozh/apple-ship --locked
```

## One-time setup (from the app repo)

Export a `.p12` from Keychain Access (My Certificates → Export). Then:

```bash
apple-ship setup --cert ~/Certificates/developer-id.p12
# optional second cert, auto-routed by type:
apple-ship setup --cert ~/Certificates/developer-id.p12 --cert ~/Certificates/distribution.p12 --force
```

`setup` prompts on the terminal for each `.p12` password (input is hidden).
If stdin is not a TTY, it exits; use `--password-stdin` instead. It does not
guess or reuse passwords.

`setup` will:

1. Read the certificate (type, Team ID, identity)
2. Create the GitHub Environment `release`
3. `gh secret set` the matching secrets (base64 p12, never printed)
4. Write `apple-ship.toml` and `.github/workflows/macos-ship.yml`

Notarization is not in the certificate. Add one of:

```bash
apple-ship setup --cert app.p12 --api-key AuthKey_XXXXXXXX.p8 --api-issuer <uuid>
# or
apple-ship setup --cert app.p12 --apple-id you@apple.com
# then:
gh secret set APPLE_APP_SPECIFIC_PASSWORD --env release
```

Push the workflow (or pass `--push`), then:

```bash
apple-ship doctor
apple-ship ship --channel developer-id
```

Tag pushes (`v*`) also run Developer ID.

## Commands

| Command | Where | What |
|---|---|---|
| `apple-ship setup --cert <p12>` | laptop | Detect certs, write workflow, set secrets |
| `apple-ship doctor` | laptop | Config + workflow + secret *names* |
| `apple-ship ship --channel developer-id` | laptop | `gh workflow run` + watch |
| `apple-ship ci --channel developer-id` | GitHub Actions only | Build, sign, notarize |

`.cer` files are rejected for secret upload: CI needs the private key, which
only a `.p12` carries. Apple Development certificates are rejected.

## `apple-ship.toml`

```toml
kind = "gpui"          # gpui | tauri | native
team_id = "N59353RP3W"
bundle_id = "com.daggy.app"
product_name = "daggy"

[gpui]
bin = "daggy"
package = "daggy-app"
info_plist = "crates/daggy-app/Info.plist"
icon = "crates/daggy-app/Assets/AppIcon.icns"
```

Secrets live in the repo's `release` environment, matching Climber's existing
layout (`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_TEAM_ID`, …).
