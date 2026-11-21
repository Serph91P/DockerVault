# Frozen Python compatibility controls for #248

This is an **initial R0 corpus, not completed R0 or Rust parity**. It records the
unchanged Python control at `18d8c3d7f5d464195ac3fb3918048c794ef1079a`.
The rewrite must preserve these contracts and the independent existing tests.

## Corpus

- `source-contracts.json`: SHA-256 inventory of all 385 tracked source paths,
  157 owned Python paths (40 application / 117 tests and tools), 114 mounted HTTP
  routes (including `/health`) and one WebSocket route. Router mounts, signatures,
  options, annotated model fields, ORM declarations and process callsites were
  parsed as AST without importing the application or invoking its lifespan.
- `sqlite-fresh.json`: actual isolated fresh migration output, published ledger
  1 through 7, 17 tables, SQL definitions, columns and foreign keys.
- `sqlite-legacy-migrated.json`: actual isolated migration of the independent
  `backend/tests/fixtures/legacy_pre_schedule.sql` fixture; not regenerated from
  current models. Historical extension columns must not be discarded.
- `sqlite-values.json`: synthetic real ORM writes and their exact SQLite values.
  In particular, persisted SQLAlchemy enums are **names** (`COMPLETED`, `FULL`,
  `INFO`), not the lowercase wire values. Datetimes are naive UTC SQLite text with
  microseconds, and the captured JSON retains null, false, zero and empty
  collections. Omitted request-field semantics remain a separate open gate.
  A Rust adapter must not substitute its default encodings.

The source inventory intentionally describes the original control, not these
new corpus files or the future Rust tree. Its hashes must not be refreshed to
make a changed control appear unchanged.

## Executed controls

On Linux/amd64, Python 3.14.10, a product image built from the exact control
contained all 40 application modules byte-for-byte. The unchanged full pytest
suite passed as UID 1000: **888 passed**, 32 warnings, 78.60% coverage with the
original 34% gate. An initial root test invocation failed three explicit
unprivileged-permission assertions; the harness UID was corrected without
modifying tests, assertions, timeouts or gates.

The unchanged real HTTP roundtrip runner passed plain and encrypted
backup -> source mutation -> local restore, then restore solely from a LOCAL
storage copy after an actual backend restart. Bytes, symlink layout and cleanup
were checked. **That is not an S3 or Rust result.**

## Open gates

Full valid/error/auth/CSRF/null/WS wire controls, the remaining archive/filter/key
corpus, independently asserted native test transfers, all real provider flows,
paired/interleaved resource measurements, native platform execution and
pre/post-write rollback remain required. Static declarations, missing-ID HTTP
requests, historical Python measurements and an initial control suite do not
satisfy those gates. No R0–R14 checkbox is closed by this corpus.

Historical Python extraction/generation runners and raw measurements remain
outside the maintained product. Future native testkit assertions must consume
these language-neutral controls without importing Python or weakening their
original independent assertions.
