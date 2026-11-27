# Local native image candidate (R13)

Building this checkout without `--target` selects the final `native-production` stage, which inherits `native-candidate`. The explicit `--target production` still builds the Python reference, including its original recovery download. Neither a local build nor a green PR check changes the published image. Do not deploy the candidate, reuse production data directories, or mount a host Docker socket for this smoke test.

```sh
docker build --platform linux/amd64 \
  --build-arg COMMIT_SHA="$(git rev-parse HEAD)" -t dockervault:native-candidate .
docker build --platform linux/amd64 -f backend/tests/browser.Dockerfile -t dockervault:native-browser .
sh docker/test-native-candidate.sh dockervault:native-candidate dockervault:native-browser
```

The smoke runs with no network other than the container's loopback, no published ports, disposable Docker volumes, and no host mounts. It exercises the normal entrypoint with s6 as PID 1 and the built frontend/nginx configuration. The Rust backend listens on loopback as UID 1000. The test verifies health through nginx, the data/SSH paths, and that a clean stop/start reopens the same SQLite file. A separate network-disabled verifier uses Node SQLite from the browser image to query the disposable database read-only before and after the restart. It checks `SELECT 1` and that migration ledger rows 1–7, including their timestamps, are unchanged. The runtime package receives no test tooling. The smoke also executes the process guard and recovery CLI as UID 1000 and checks the archive worker's rejection of that UID and successful use by an isolated root test process against a synthetic fixture.

The worker is packaged as a root-owned executable, **not** delegated via sudo: its current request allows the caller to supply source prefixes. Mode 0755 alone is not a privilege grant. The production authorization of sources needs a trusted boundary before a service-to-root rule can be qualified. No recovery executable is served at the legacy Python URL. The native recovery CLI's presence and startup are checked here; this does not qualify packaged offline restoration.

## Authenticated encrypted backup and offline recovery

Build the separate recovery image from the same checkout and for the same architecture as the application image above. The test runner requires Bash, Docker, and `jq`:

```sh
docker build --platform linux/amd64 -f docker/recover.Dockerfile \
  --build-arg VCS_REF="$(git rev-parse HEAD)" -t dockervault:native-recovery .
bash docker/test-native-encrypted-roundtrip.sh \
  dockervault:native-candidate dockervault:native-recovery
```

The runner checks both image revision labels and architectures. It submits an encrypted path backup through authenticated HTTP, changes the source, restarts the app, and verifies that it reopens the same SQLite file. It then stops the app and recovers the original payload byte-for-byte in the separate network-disabled recovery image. The runner creates its own synthetic identity; no production key, copied database, or host mount is needed. The native CI matrix builds without a target and includes this check on Linux amd64 and arm64. Legacy Python integration checks explicitly build `production` from the candidate tree; that image is not fenced and is not the rollback image below.

`docker/test-native-s3-restart.sh` checks plain and encrypted S3-only restore after restart on both native GitHub-hosted CI architectures, using digest-pinned SeaweedFS on isolated loopback. It requires automatic removal of the local originals after verified sync, unchanged remote archive/sidecar bytes, and restoration of the original bytes, mode and mtime. For the encrypted backup, authenticated removal preview/execute and duplicate confirmation before and after a clean restart must report no removal and preserve the archive, sidecar, unrelated object, listing and catalog/sync entry. This is fail-closed coverage, not acceptance of external providers, successful remote deletion, partial settlement or crash recovery. Versioned and suspended buckets are not configured or exercised; the same non-LOCAL guard rejects them without inspecting bucket versioning.

These checks do not qualify runtime Docker socket/GID or trusted proxy handling, active-job shutdown or worker drain, browser recovery downloads, performance, or a deployment cutover.

## Release gate bootstrap before cutover (R14)

GitHub runs the `workflow_run` listener from the default branch, not from the tested `develop` revision it checks out. The transition safeguard [#266](https://github.com/Serph91P/DockerVault/pull/266) on `main` explicitly pins `target: production` (Python). A change in the draft does not update that listener. Check its effective definition and writer skips separately at integration.

An ordinary PR against `develop` validates the default native stage with `BUILD_ENV=production`. On a direct `develop` push the production-profile checks still run and a second profile tests the actual development release package with `BUILD_ENV=development` (`NODE_ENV` at frontend build time, `APP_ENV=development` at runtime). On both architectures, the tested OCI path imports the platform from its Buildx export via Skopeo, checks the Docker config ID, starts that image and binds the browser-tested offline download to the same package. PR receipts are validation only. The separate standalone recovery image is not a substitute for the app or an input to its release.

The Develop release requires successful Tests and the exact native push run on the same commit. It admits only the two `native-dev-*` artifacts from the successful attempt, checks the receipts and downloads, and copies the verified multiarch OCI index with its attestations without rebuilding. All three Dev image tags must resolve to that index before the GitHub prerelease. The package's embedded version is the Dockerfile default `dev`; the numbered `-dev` tag is selected after testing and does not change the image config. Main pushes and explicit version-tag paths still build separately under their own release conditions; they do not use Develop receipts. No new release assets are produced.

## Rehearsal-only Python rollback image (R14)

The unmodified Python image is a control, not a rollback target. Build the separate rollback stage from the verified historical source on the platform being tested. The stage keeps the Python application, original entrypoint, schema and data format unchanged.

```sh
python_ref=39124ea3a98963b6307a69a0fbaf5902a17cde3d
git archive "$python_ref" | docker build --target production \
  --build-arg "COMMIT_SHA=$python_ref" -t dockervault-r14-python-ref:39124ea -
test "$(docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' \
  dockervault-r14-python-ref:39124ea)" = "$python_ref"
docker build --target rollback-preflight -t dockervault:r14-rollback-preflight .
docker build -f docker/python-rollback.Dockerfile --target python-rollback \
  -t dockervault:python-rollback .
sh docker/test-writer-fence.sh dockervault:native-candidate dockervault:python-rollback
```

The two fenced images always take a nonblocking exclusive Linux `flock` on the `/app/data` **directory inode** before running either init. On contention the new container exits with status 73; it does not start nginx, SQLite or background jobs. Use the same trusted local Docker volume mounted at `/app/data`, the default SQLite filename on that mount, and the default entrypoints. The lock does not coordinate separate volume copies, network filesystems, other hosts, or an unfenced Python reference. Do not rename or replace the shared mount or start any unfenced service against it.

**Lock acquisition is not permission to hand off unsettled work.** The rollback image checks the current SQLite ledger and journals read-only under the fence, before executing legacy init. Exit 74 means the database or its settlement could not be verified; neither app nor scheduler starts. Stop admission, schedulers and workers, confirm the old container and all children have stopped, and verify external effects independently. Reconcile pending native work with the Rust candidate on the same data before retrying. If Rust cannot start, keep both services stopped, preserve a consistent SQLite copy (including outstanding WAL state), archive/sidecar bytes and the credential-key source, and investigate on a disposable copy. Do not start the unfenced Python reference, clear the journal by hand or restore a stale DB over newer archives. The rehearsal is not a production handoff or a substitute for verifying provider settlement.
