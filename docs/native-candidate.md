# Local native image candidate (R13)

`native-candidate` is opt-in. The final `production` stage is still the Python reference, including its original recovery download. Do not deploy the candidate, reuse production data directories, or mount a host Docker socket for this smoke test.

```sh
docker build --platform linux/amd64 --target native-candidate \
  --build-arg COMMIT_SHA="$(git rev-parse HEAD)" -t dockervault:native-candidate .
sh docker/test-native-candidate.sh dockervault:native-candidate
```

The smoke runs with no network other than the container's loopback, no published ports, disposable Docker volumes, and no host mounts. It exercises the normal entrypoint with s6 as PID 1 and the built frontend/nginx configuration. The Rust backend listens on loopback as UID 1000. The test verifies health through nginx, the data/SSH paths, and that a clean stop/start reopens the same SQLite file. It executes the process guard and recovery CLI as UID 1000 and checks the archive worker's rejection of that UID and successful use by an isolated root test process against a synthetic fixture.

The worker is packaged as a root-owned executable, **not** delegated via sudo: its current request allows the caller to supply source prefixes. Mode 0755 alone is not a privilege grant. The production authorization of sources needs a trusted boundary before a service-to-root rule can be qualified. No recovery executable is served at the legacy Python URL. The native recovery CLI's presence and startup are checked here; this does not qualify packaged offline restoration.

This does not qualify arm64, runtime Docker socket/GID or trusted proxy handling, rclone packaging, authenticated backup/restore, active-job shutdown or worker drain, recovery downloads, performance, or cross-runtime handoff. The candidate has only the Rust service's existing native-writer lock; it is not a shared fence with the Python reference. No migration or rollback on shared deployment state is authorized by this test.
