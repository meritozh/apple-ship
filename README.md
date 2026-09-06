# apple-ship

One command to configure GitHub Actions CI for shipping a macOS **GPUI**,
**Tauri**, or **native Xcode** app. Run it **in the app repository**, not as
install-time setup for this tool.

Packaging never runs on your laptop. `setup` writes the workflow and GitHub
secrets; `ci` runs only on a `macos-*` runner.

```bash
cargo install apple-ship --git https://github.com/meritozh/apple-ship --locked
```

From the app repo:

```bash
apple-ship setup --channel developer-id --cert ~/Documents/certs/developerID_application.p12
apple-ship setup --channel app-store --cert ~/Documents/certs/distribution.p12
```

`--channel` selects the certificate that must be in `--cert`:

| Channel | Required certificate | Result |
|---|---|---|
| `developer-id` | Developer ID Application | Notarized `.dmg` on tag / workflow_dispatch |
| `app-store` | Apple Distribution | `.pkg` (not implemented in v0.1) |

A file that does not contain that certificate is an error. Passwords are
prompted on a TTY (hidden). Non-TTY: `--password-stdin`.

`setup` writes or updates `apple-ship.toml` in this repo. Kind is auto-detected
when the file is missing. Pass `--kind gpui|tauri|native` to skip detection
or to change kind on a later run.

`setup` writes `apple-ship.toml` and `.github/workflows/macos-ship.yml` for
this repo, and stores the cert in the GitHub Environment `release`. Commit
and push the workflow, then dispatch **macOS Ship** on GitHub (or push a `v*`
tag for Developer ID).

`apple-ship ci` is the job body. It exits unless `GITHUB_ACTIONS=true`.
