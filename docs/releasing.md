# Releasing

Installers are built by GitHub Actions (`.github/workflows/release.yml`) and
attached to a GitHub release, one per version tag:

| Platform | Installers |
|---|---|
| Linux x86-64 | `.AppImage`, `.deb`, `.rpm` (built on Ubuntu 22.04, so the AppImage runs on glibc ≥ 2.35) |
| Windows x86-64 | NSIS `-setup.exe`, `.msi` |
| macOS Apple silicon and Intel | `.dmg` each |

Each one bundles the official Stockfish 19 for its platform (ENG-09) via
`scripts/fetch-stockfish.sh` and `app/src-tauri/tauri.stockfish.json`.

## Cutting a release

1. Set the new version in all three places (they must agree):
   - `Cargo.toml` → `[workspace.package] version`
   - `app/package.json` → `version` (and run `npm install` in `app/` to update
     the lock file)
   - `app/src-tauri/tauri.conf.json` → `version` (this one names the
     installers)
2. Run the checks (AGENTS.md → Commands), commit and merge to `main`.
3. Tag the merged commit and push the tag:
   ```sh
   git tag v0.2.0
   git push origin v0.2.0
   ```
4. The workflow fails at once if the tag doesn't match `tauri.conf.json`.
   Otherwise, after about 20 minutes, a **draft** release named
   "Chess Database v0.2.0" has every installer attached.
5. On GitHub → Releases, open the draft, add what changed to the
   notes, and **Publish**.

A failed build leaves the draft partly filled. Delete the draft and the tag
(`git push --delete origin v0.2.0`, `git tag -d v0.2.0`), fix, and tag
again.

To build the installers without releasing, run the workflow by hand
(Actions → Release → Run workflow) or open a PR that changes it: the
installers are attached to the run as artifacts.

## Signing

Nothing is code-signed yet, so the first launch warns (the release notes
say how to get past it):

- **Windows:** SmartScreen. Signing needs a code-signing certificate (or
  Azure Trusted Signing); Tauri reads it from `bundle.windows`.
- **macOS:** the app is ad-hoc signed (`bundle.macOS.signingIdentity: "-"`),
  so Gatekeeper offers "Open Anyway" instead of calling it damaged. Signing
  and notarizing properly needs an Apple Developer account ($99/year) and
  the `APPLE_*` secrets tauri-action reads.

There's no auto-updater yet (Tauri's updater plugin needs a signing key
pair and `latest.json` on the release; tauri-action can produce both).
