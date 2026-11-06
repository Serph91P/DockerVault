# Python I/O and workload interruption: issue #231

## Result and limits

This change removes archive-sized S3 download buffers, avoids redundant upload-verification reads, reuses bounded metadata across archive pages, and moves single-file preparation off the event loop. It shortens the stopped-workload window without changing posthook order, source claims, job permits, archive formats, keys, or complete remote-byte verification.

The measurements are **mixed**, not a claim that every backup is faster. Repeated listing, S3 memory use, verification reads and measured workload interruption improve. Some complete backup/restore cases get slower. Compression/encryption remain substantial costs; these runs do not establish why individual regressions occurred.

[Machine-readable results](benchmarks/python-io-231-summary.json) contain all canonical per-run interval metrics, medians/ranges, component samples, immutable image bindings and execution return codes. Full original snapshots/cProfile output and diagnostic failures are retained in the accompanying evidence bundle. Nothing here is a cloud-throughput, operation-specific RSS-peak, or production-deployment claim.

## Source and environment binding

- Baseline product commit: `773679fca82813cc7d8c9f4ed1442a8a86f17c84`.
- Implemented product commit: `4d2578bd712823e688621bcd86ea1e0e2611a283`. Subsequent report-only changes do not alter product modules.
- Baseline image: `sha256:7890dd3158539468f20cc6faca408c3d06b447ac275585d70d12803fa8dc7513`.
- Historical component-candidate image: `sha256:944cf6599e709a74d2c4a8d36095f87a3c7664cc42f2ed0dd74f8c5b4cfb0bb4`.
- Final product measurement image: `sha256:4c36da935a4530f54760cabaca59462aaa90236c5923fef4228eb03fc03f0a24`.
- All three measured images report the same Python **3.14.10**, GCC 14.2.0 runtime string. Native worker pytest used Python 3.14.4; independent owner validation used 3.14.7. Those native test environments are not substituted for the image measurements.
- Six final product-module SHA256 fingerprints match the worktree. Historical component profiling uses identical remote-storage, browse, encryption and recovery bytes; the checksum method, executor-settling helper and engine initializer have identical AST fingerprints. Later engine/Docker changes address restore staging, queued errors and enumeration, not the profiled operations. Final macro/runtime measurements exercise those later changes.
- Each macro/profile container: **2 CPUs, 6 GiB** memory. Actual service: private, disposable SeaweedFS **4.35**, 4 GiB limit, native S3 SDK/HTTP, no cloud account or published host port.
- Large-fixture sizes are **1,073,741,824 bytes (1 GiB) for random content** and **1,140,850,688 bytes (1.0625 GiB) for compressible content**. Both exceed or meet the required 1 GiB class; identical sizes/patterns are used before and after. Small fixtures contain **25,000 files**, plus directory/link metadata in app archives. The separate component-profiler payload is exactly **1 GiB** of seeded random bytes.
- Sources are warmed by fixture creation and independent byte hashing. No global cache flush. Each app run has fresh owned paths/process state. The host is shared: collection-start load averages range from `4.22 5.50 7.57` to `28.56 20.65 13.67`; environment JSON preserves each collection's snapshot. Other owned test collections sometimes overlap. These observations do not prove a cause for any individual regression.
- Two/four means concurrent **HTTP job submissions on independent source paths**. The existing global product capacity remains **two executing jobs**, including upload/cleanup. Four submitted jobs are queued under that same limit, not represented as four simultaneously executing archives.

The canonical comparison contains **84 runs**: 30 baseline + 30 final main matrix, six + six S3 matrix, six + six workload matrix; each case has three repetitions. The separate component profiles contain 24 measurements per image, including nine listing pages. Historical/incomplete diagnostic collections are not added to these totals.

## What changed

| Confirmed cost | Smallest production change | Safety retained |
|---|---|---|
| Complete S3 `Body.read()` | Native body context and 1 MiB reads; joined threaded writes; private same-directory partial followed by atomic replace | Content length, independently computed SHA256 during verification, complete body lifetime, existing destination preserved on failure, partial cleanup |
| Rehash source per destination and reread S3 verification file | One invocation-local source digest, bound to device/inode/size/mtime/ctime; hash S3 verification bytes while writing | Source identity checked around hashing/upload/verification; complete independent downloaded-byte verification; no ETag/size shortcut |
| Python 8 KiB checksum loop | Native `hashlib.file_digest`, through the existing cancellation-settled executor | Same SHA256/plaintext catalog meaning; reader joined before cleanup |
| Repeated full scan on every page | Private RAM metadata prefix, keyed by immutable file identity; cross-page reuse | Deterministic order/totals, mutation/replacement/deletion checks, real authentication on every encrypted request |
| Async endpoints synchronously open/search/extract | Shared ASGI response owns capacity, worker preparation, streamed reads and final cleanup | Both endpoints; pending readers are joined even on disconnect or failure before the first body chunk; native duplicate/link behavior preserved without an unbounded TarInfo list |
| Hash/encryption unnecessarily extend stop window | Without a posthook, restart after the completed immutable archive and persist confirmed restart results | Source claims/global job permit retained to job completion; failed restarts retain obligations; initially stopped workloads stay stopped |
| Restore decryption extends interruption and leaves a normal-named plaintext file after hard crash | Authenticate before stop; pre-register unique `.dockervault-restore-*.tmp` under the managed backup root and pass explicit output path | Existing startup staging cleanup; no blanket `/tmp/*.tar` deletion or overwrite of legitimate archives; compression comes from the original archive format |
| Cancelled hook can outlive its job | Shield spawn, kill its process group, drain/reap before cancellation returns | Existing command validation and posthook-after-encryption/before-restart semantics |
| Unrelated container removed between list and inspect | Pinned Docker SDK's native `ignore_removed=True` | Other API errors propagate; explicitly requested/claimed missing identity still fails closed |
| Preflight exception leaves accepted job pending | Persist FAILED/completion time for the still-PENDING row | Do not overwrite terminal results or swallow cancellation; normal claims/permits/journal handling unchanged |

The listing cache is limited to **16 archive entries, 8 MiB serialized metadata globally, 32,768 members per prefix, 60 seconds TTL**. This is not an 8 MiB RSS promise: decoded response objects and up to four active workers also consume bounded memory. Pages beyond the cached prefix scan normally. The 25k fixture fits (3,825,009 serialized bytes in an independent cross-page check). There is no private-key cache, persistent plaintext index, or archive-byte cache. Encrypted cache hits still fully authenticate/decrypt, so they do not eliminate cryptographic work.

## Component attribution (seconds, medians)

These are equally instrumented component runs, **not** complete backup times. CPU measurements include self and waited children; cProfile also records async waiting.

| Operation | Baseline wall | Component candidate wall |
|---|---:|---:|
| SHA256, 1 GiB | 1.1062 | 0.6283 |
| Native S3 download, 1 GiB | 4.5522 | 3.0464 |
| Two-destination upload + independent verification, 1 GiB | 20.6899 | 19.1103 |
| Nine listing pages, mixed cache states | 1.2253 | 0.0418 |
| Unchanged encryption, 1 GiB | 5.6087 | **6.2368** |
| Unchanged decryption, 1 GiB | 6.4448 | 5.2696 |

Two-target verification CPU falls from **6.9496 to 5.7558 seconds**; logical process `rchar` falls from **7,516,193,203 to 5,368,709,533 bytes**. That is fewer full reads, not weaker integrity. The plaintext catalog checksum is distinct from ciphertext and sidecar upload digests.

The candidate listing's two first/TTL-expired scans take **1.5326/1.4497 seconds** (median 1.4912); the other seven page accesses have median **0.0353 seconds**. Their logical reads are roughly 0.4 KiB rather than about 211 KiB for a scan. First access is not universally faster: constructing the reusable index has a cost. A single mixed median would hide that distinction.

Whole-profile Python-process HWM: **2226.07 → 236.98 MiB**. Whole-profile cgroup lifetime peak: **5451.42 → 5482.82 MiB**, essentially unchanged, including fixture creation/page cache. Neither number is a per-S3-operation peak. In the separate real S3 macro's large case, median backend-process HWM across all recorded snapshots, including remote-only restore, is **2359.41 → 196.79 MiB**; median whole-run cgroup peak is **4669.15 → 3628.03 MiB**. These are different scopes and must not be interchanged.

## Complete app backup/restore matrix

Medians of three runs, seconds. Backup ends only when the catalog is completed **and independently verified storage synchronization is complete**. Restore interval begins after listing and deliberate source mutation. Archive file creation alone and HTTP acceptance are not completion.

`C` = compression, `E` = encryption; four submissions retain the native two-job capacity.

| Dataset | C/E | Submitted jobs | Backup baseline/final | Restore baseline/final |
|---|---|---:|---:|---:|
| 1.0625 GiB compressible | no/no | 1 | 11.550 / 8.141 | 2.202 / 2.510 |
| 1 GiB random | no/no | 1 | 6.574 / 7.879 | 1.931 / 3.112 |
| 1.0625 GiB compressible | yes/yes | 1 | 3.511 / 4.041 | 2.821 / 4.948 |
| 1 GiB random | yes/yes | 1 | 28.959 / 43.877 | 7.904 / 9.317 |
| 25k small | no/no | 1 | 1.597 / 1.684 | 2.133 / 2.189 |
| 25k small | yes/yes | 1 | 1.999 / 1.841 | 2.747 / 2.510 |
| 1 GiB random | no/yes | 2 | 19.132 / 23.575 | 12.510 / 16.182 |
| 1.0625 GiB compressible | yes/no | 4 | 10.004 / 8.978 | 8.616 / 10.811 |
| 25k small | no/yes | 2 | 2.602 / 2.064 | 17.073 / 5.877 |
| 25k small | yes/no | 4 | 6.320 / 3.826 | 33.390 / 13.634 |

CPU medians below are **user/system separately**, seconds, for the disposable product cgroup including backend children, nginx and the probe. They are not attributed solely to a single function.

| Case (same order) | Backup baseline user/sys | Backup final user/sys | Restore baseline user/sys | Restore final user/sys |
|---|---:|---:|---:|---:|
| Compressible raw | 3.971 / 2.112 | 2.888 / 1.229 | 0.099 / 0.289 | 0.083 / 0.280 |
| Random raw | 2.553 / 1.117 | 2.414 / 1.114 | 0.085 / 0.255 | 0.097 / 0.274 |
| Compressible gzip+encrypted | 3.230 / 0.222 | 3.449 / 0.232 | 1.154 / 0.223 | 1.262 / 0.270 |
| Random gzip+encrypted | 23.743 / 2.419 | **25.715 / 2.877** | 1.691 / 1.407 | 1.473 / 1.476 |
| Small raw | 1.493 / 0.205 | 1.463 / 0.208 | 1.447 / 0.615 | 1.500 / 0.579 |
| Small gzip+encrypted | 2.057 / 0.220 | 1.765 / 0.195 | 2.008 / 0.660 | 1.825 / 0.602 |
| Random raw+encrypted, 2 | 8.751 / 4.257 | 10.122 / 5.044 | 2.077 / 2.873 | 2.562 / 3.372 |
| Compressible gzip, 4 | 14.489 / 0.750 | 13.925 / 0.679 | 5.081 / 1.010 | 5.850 / 1.261 |
| Small raw+encrypted, 2 | 3.735 / 0.688 | 3.172 / 0.551 | 6.314 / 3.631 | 4.936 / 2.788 |
| Small gzip, 4 | 9.648 / 0.997 | 6.623 / 0.600 | 16.614 / 10.233 | 12.253 / 7.103 |

All per-run logical backend-PID counters, cgroup physical read/write counters, full-run memory peaks and ranges are in the JSON. For example, large compressible raw backup has median logical `rchar` **6,847,412,070 → 6,847,353,648**, but physical cgroup `rbytes` **4,907,008 → 380,928** and `wbytes` **3,541,921,792 → 3,176,640,512**. Cache/writeback makes physical I/O differ from logical reads; these are available counter intervals, not an isolated disk-throughput or durable-flush benchmark. Backend-PID logical I/O does not aggregate every child process; cgroup CPU/physical I/O does. Missing metrics in diagnostic collections remain explicitly unavailable, never zero-filled.

## Measured Docker interruption and responsiveness

These are real owned container IDs, stop/start and HTTP response-body checks, not simulated running flags. Values bracket sampled transitions (first observed down to observed recovery), not exact event timestamps. Each backup-ready series has one contiguous down interval; raw sampling timestamps and maximum polling gaps are retained. Dependency/app return under the same IDs; the initially stopped fixture never starts.

| Workload case | Backup app-running down, baseline/final | Backup app-ready down, baseline/final | Restore app-ready down, baseline/final |
|---|---:|---:|---:|
| 1.0625 GiB compressible raw | 5.013 / 2.068 | 5.580 / 2.747 | 3.095 / 2.383 |
| 1 GiB random gzip+encrypted | 32.192 / 19.566 | 32.614 / 19.949 | 7.174 / 2.910 |

Across **54 real single-file downloads per main matrix**, concurrently issued health calls show **10 → 0 one-second timeouts**; the maximum successful sampled health latency is **0.9903 → 0.2197 seconds**. The regression tests additionally hold actual worker threads to verify event-loop progress, shared four-slot capacity and cleanup before/after response start. This is evidence for these fixtures, not a universal latency SLA.

## Integrity and failure matrix

- Real HTTP/API/SQLite backup → mutate owned source → local restore and verify bytes, size, mode, mtime, symlinks; plain/encrypted tar/tar.gz combinations and independent jobs.
- Real private SeaweedFS S3: upload plus complete byte verification, delete local archive/sidecar, mutate source again, remote-only restore and compare. Six baseline/six final runs. This S3 matrix does **not** restart the backend during remote restore; restart coverage is a separate gate, not an untested S3×restart cross-product claim.
- Native S3 SDK against deterministic HTTP faults: short/truncated body, wrong bytes, stalled/timeout body and cancelled disk write; existing destination preserved and body/worker/partial resources settled. Existing same-size corruption, tamper, missing-sidecar and safe extraction/link tests remain enabled.
- Ten real product-process scenarios: active archive reader cancellation in the supported in-process fallback; checksum-thread cancellation; real codec-child cancellation; real hook-child cancellation; hard Uvicorn exit 81 after settled archive, after settled hash and after authenticated restore preparation; posthook failure; real HTTP download disconnect; supported legacy CBC/raw-DEK restore with the same age key and byte/mode/mtime comparison. Overlapping source claims serialize; permits remain held until workers settle.
- Restore-crash regression is RED on the old image after **full startup**, GREEN on the final image: the actual uniquely owned plaintext output disappears; an adjacent legitimate plaintext archive and ciphertext/sidecar survive. This was a private 0600 resource leak, not a claim of public secret disclosure.
- Cached encrypted listing still rejects a wrong key (400) and an unauthenticated request (401). Cache replacement/mutation/deletion and byte/count eviction have explicit regression tests.
- Native real-daemon enumeration probe physically removes only a registered owned object between real list and inspect. Survivor stays visible, explicit missing identity raises, exact owned IDs/anonymous volumes are removed and verified absent. Unit tests preserve other API errors and terminal SQLite status on queued preflight failure without swallowing cancellation.
- Existing `run_backup_roundtrip.py` passes plain+encrypted HTTP backup/local restore and storage-only restore after real Supervisor backend restart. Its storage provider is **LOCAL**, not S3.

## Verification and integration

- Untouched baseline: **666 tests** pass. Final product: **703 tests**, 27 existing warnings, **76.23%** coverage; existing **34%** gate unchanged. Independent owner full run also passes 703 tests (76.17%, instrumentation-dependent).
- Ruff checks/format, frontend TypeScript/ESLint/build and product Docker build pass. Existing tests/assertions/timeouts and CI workflows are not weakened.
- PR #230 integration: conflict-free dry merge tree `c1739085c28f286d470f04811eb09ffcf1e876b1`, combining this product commit with #230 head `6d32642a74d6682a14a5c973206ffa2843d3084f`. Full combined backend: **768 passed**, 27 existing warnings, **76.41%**, same 34% gate. This was an isolated test snapshot, not a branch/PR merge.
- Integration test setup: native disk-backed SQLite DDL was extremely slow; root-container permission assertions and a codec-wrapper-ready failure were preserved as failed diagnostics. Final run uses the existing unprivileged app UID and an executable temporary test filesystem. No assertion, timeout or skip was changed. Source was copied into the disposable container with `docker cp`, not assumed to share the daemon's host filesystem.
- Exact product-head GitHub CI: backend, frontend, lint/typecheck, security scan, Trivy, GitGuardian and real runtime integration on both amd64 and arm64 pass. Report-only head CI is checked again in the PR handoff; independent review remains a separate gate, not self-approval.

## Reproduce

Run from a disposable checkout with Docker access, using only test-owned fixtures. Never point these runners at production paths. Restartable workload subjects intentionally do not auto-remove at their stop boundary; their registered IDs/labels and anonymous volumes are checked and cleaned in `finally`. Short-lived runner/service containers auto-remove.

```sh
# Keep source/image references immutable and output outside the checkout.
EVIDENCE=/absolute/path/to/dv231-evidence
python3 backend/tests/performance_231.py BASELINE_IMAGE "$EVIDENCE/baseline-v2"
python3 backend/tests/performance_231.py CANDIDATE_IMAGE "$EVIDENCE/candidate-final"
python3 backend/tests/performance_231.py CANDIDATE_IMAGE "$EVIDENCE/workloads-final-verified" --workloads --case 0 --case 3
python3 backend/tests/performance_231.py CANDIDATE_IMAGE "$EVIDENCE/phases-final" --workloads --phase-suite --case 5 --repeats 1
PYTHONPATH=backend python3 backend/tests/runtime_enumeration_231.py CANDIDATE_IMAGE "$EVIDENCE/native-enumeration.json"
python3 backend/tests/run_backup_roundtrip.py CANDIDATE_IMAGE
```

For S3, create a uniquely named private **owned** SeaweedFS 4.35 service (`server -s3 -dir=/data -ip=127.0.0.1 -ip.bind=127.0.0.1 -master.volumeSizeLimitMB=1024 -volume.max=8`), no published ports, 4 GiB memory and `dockervault.fixture=dv231` label. The macro runner joins its network namespace:

```sh
python3 backend/tests/performance_231.py CANDIDATE_IMAGE "$EVIDENCE/candidate-s3-final" --case 0 --case 5 --s3-container OWNED_S3_CONTAINER
```

The component script `backend/tests/profile_231.py` runs inside a disposable product image with `DV231_OWNED=1`, 2 CPUs/6 GiB, sharing that service's namespace; copy it with `docker cp`, then execute `python /app/profile_231.py /app/data/profile.json` and copy the result before removing the container. It writes progress after every measured operation. Its fixture credentials are synthetic and confined to the private service. Capture the process return code **before** copying results/cleanup. Record immutable image IDs and compare source bytes; do not label a mutable tag as a measured commit.

The evidence bundle's stdlib-only `analyze.py --complete` checks all 84 canonical samples, completed+verified effects and available backend counters before producing `comparison-complete.json`. Diagnostic directories, earlier missing-before-restore samples and the old stalled PENDING reproduction are retained separately, not silently repaired or counted as successful measurements.

### Remaining limits

No cloud/network-distance claim, privileged-tarworker cancellation claim, exhaustive S3×crash cross-product, production deployment, Rust implementation or universal speedup. Large-archive pages beyond the metadata prefix still rescan; encrypted listing still pays authentication/decryption; compression/crypto and some final macro cases remain expensive. These are disclosed bounds, not removed security checks or invented improvements.
