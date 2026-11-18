# Local native image candidate (R13)

`native-candidate` is opt-in. The final `production` stage is still the Python reference, including its original recovery download. Do not deploy the candidate, reuse production data directories, or mount a host Docker socket for this smoke test.

```sh
docker build --platform linux/amd64 --target native-candidate \
  --build-arg COMMIT_SHA="$(git rev-parse HEAD)" -t dockervault:native-candidate .
sh docker/test-native-candidate.sh dockervault:native-candidate
```

The smoke runs with no network other than the container's loopback, no published ports, disposable Docker volumes, and no host mounts. It exercises the normal entrypoint with s6 as PID 1 and the built frontend/nginx configuration. The Rust backend listens on loopback as UID 1000. The test verifies health through nginx, the data/SSH paths, and that a clean stop/start reopens the same SQLite file. It executes the process guard and recovery CLI as UID 1000 and checks the archive worker's rejection of that UID and successful use by an isolated root test process against a synthetic fixture.

The worker is packaged as a root-owned executable, **not** delegated via sudo: its current request allows the caller to supply source prefixes. Mode 0755 alone is not a privilege grant. The production authorization of sources needs a trusted boundary before a service-to-root rule can be qualified. No recovery executable is served at the legacy Python URL. The native recovery CLI's presence and startup are checked here; this does not qualify packaged offline restoration.

This does not qualify runtime Docker socket/GID or trusted proxy handling, rclone packaging, authenticated backup/restore, active-job shutdown or worker drain, recovery downloads, performance, or a deployment cutover.

## Rehearsal-only Python rollback image (R14)

The unmodified Python image is a control, not a rollback target. Build the separate rollback stage from the verified historical source on the platform being tested. The stage keeps the Python application, original entrypoint, schema and data format unchanged.

```sh
python_ref=39124ea3a98963b6307a69a0fbaf5902a17cde3d
git archive "$python_ref" | docker build --target production \
  --build-arg "COMMIT_SHA=$python_ref" -t dockervault-r14-python-ref:39124ea -
test "$(docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' \
  dockervault-r14-python-ref:39124ea)" = "$python_ref"
docker build -f docker/python-rollback.Dockerfile --target python-rollback \
  -t dockervault:python-rollback .
sh docker/test-writer-fence.sh dockervault:native-candidate dockervault:python-rollback
```

The two fenced images always take a nonblocking exclusive Linux `flock` on the `/app/data` **directory inode** before running either init. On contention the new container exits with status 73; it does not start nginx, SQLite or background jobs. Use the same trusted local Docker volume mounted at `/app/data`, the default SQLite filename on that mount, and the default entrypoints. The lock does not coordinate separate volume copies, network filesystems, other hosts, or an unfenced Python reference. Do not rename or replace the shared mount or start any unfenced service against it.

**Lock acquisition is not permission to hand off unsettled work.** Stop admission, schedulers and workers, check that the old container and its children have actually exited, and verify the current journal and external effects. Unresolved recovery, unknown provider effects or an incompatible migration ledger block a handoff. Do not start Python to clear those conditions and do not restore an older DB over newer archive bytes. The image smoke proves exclusion and release after a clean stop and a container SIGKILL, not journal safety during a crash with active I/O. No handoff on deployment data is authorized by this rehearsal.
