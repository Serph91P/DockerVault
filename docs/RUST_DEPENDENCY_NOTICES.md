# Rust dependency notices

The pinned Rust dependencies and licenses are recorded in `Cargo.lock` and checked
by `cargo deny`. Runtime distribution must retain the corresponding license texts,
notices and source-availability information alongside its SBOM.

## Mozilla Fernet 0.2.2

The unmodified `fernet` crate is licensed under **MPL-2.0**. DockerVault links the
published implementation rather than implementing a new Fernet cryptosystem.
The bounded package/version allowance in `deny.toml` does not waive advisories or
allow other copyleft dependencies.

- License: <https://www.mozilla.org/MPL/2.0/>
- Exact published source and license: <https://docs.rs/crate/fernet/0.2.2/source/>
- Source archive: <https://crates.io/api/v1/crates/fernet/0.2.2/download>
- Upstream: <https://github.com/mozilla-services/fernet-rs/>

MPL obligations apply to the covered dependency files; they are not a change to
DockerVault's own license. Final image/offline packaging must include the full
MPL license and this source-availability notice. Packaging verification remains
part of the Rust image/recovery deliverable, not established by this notice alone.

## Unicode classification for rclone references

`unicode-general-category` 1.1.0 (Apache-2.0, no dependencies or declared MSRV;
compiled with the pinned Rust1.99.1 toolchain) supplies the accepted provider's
Unicode16.0.0 letter/number categories. Rust's Unicode17 `is_alphanumeric` also
includes combining alphabetic marks, so it is not Python's `\\w` name contract.
The native test checks every Unicode scalar against the actual provider oracle.
Source: <https://github.com/yeslogic/unicode-general-category>.

## Native cron and TZif compatibility

`tz-rs` 0.7.3 (MIT OR Apache-2.0, Rust1.85+, dependency-free) is pinned with
only its `alloc` feature: it parses caller-owned TZif bytes and resolves UTC,
ambiguous and skipped wall times without filesystem, clock or ambient-TZ APIs.
It does not embed a different timezone database. Seven public TZif fixtures are
captured from the actual accepted provider image; IANA timezone data is public
domain. One explicitly labelled synthetic TZif additionally tests Python's
ignore-leap-record behavior against that image's actual ZoneInfo/CronTrigger.
Source: <https://docs.rs/crate/tz-rs/0.7.3>.

`dockervault-core/src/cron.rs` adapts APScheduler3.11.3's five-field expression
and field-stepping algorithms to Chrono and tz-rs. The executed stock Croner4.0.3
five-field/AND-mode comparison did not preserve the accepted provider's skipped
DST time or named/prefix expression grammar, so Croner is **not** a dependency.
The adapter preserves Monday-zero weekdays, day-of-month **and** weekday,
Unicode16 decimal digits, last-day and named-prefix behavior, fold/inter-zone
comparison semantics, UTC arithmetic and Python's 1..9999 intermediate range.
Its explicit-base preview is not a running scheduler, job admission or misfire
implementation. Copyright (c) Alex Grönholm. Retain the full MIT license in
[`licenses/APSCHEDULER-LICENSE.txt`](licenses/APSCHEDULER-LICENSE.txt).

- Exact reference: <https://github.com/agronholm/apscheduler/tree/3.11.3/src/apscheduler/triggers/cron>
- License: <https://github.com/agronholm/apscheduler/blob/3.11.3/LICENSE.txt>

## CPython compatibility algorithms

Character-class range normalization in `dockervault-core/src/filters.rs` adapts
CPython's `fnmatch.py` behavior to native Rust tokens (no Python interpreter).
Public SSH metadata decoding in `dockervault-core/src/ssh_public.rs` also adapts
CPython 3.14.10 `binascii.a2b_base64` filtering/padding acceptance around the existing
native Base64 decoder; it never retains comments or following input lines.
Timestamp ISO framing in `dockervault-core/src/timestamp/iso.rs` adapts CPython
3.14.10 `_datetimemodule.c` around Chrono's calendar validation. This preserves
basic/week dates, Unicode separators, midnight rollover and microsecond offsets
without loading Python or reducing timestamp precision to whole offset seconds.
Provider configuration repair hints in `dockervault-core/src/storage_validation.rs`
adapt CPython 3.14.10 `urllib.parse` authority/port framing around the standard
IPv6 parser. The 19 Unicode 16 NFKC delimiter scalars come from an actual full-scalar
source enumeration. This reproduces **metadata hints only**, not network URL
validation, SSRF authorization or connection success; transports retain their
separate strict parsers and safety gates. Existing Reqwest URL parsing was
actually executed and rejects legacy metadata forms accepted by the source.
Copyright Python Software Foundation and contributors. The complete upstream
license, including its historical notices, is retained in
[`licenses/PYTHON-LICENSE.txt`](licenses/PYTHON-LICENSE.txt).

- Exact reference source: <https://github.com/python/cpython/blob/v3.14.10/Lib/fnmatch.py>
- Exact Base64 reference: <https://github.com/python/cpython/blob/v3.14.10/Modules/binascii.c>
- Exact datetime reference: <https://github.com/python/cpython/blob/v3.14.10/Modules/_datetimemodule.c>
- License source: <https://github.com/python/cpython/blob/v3.14.10/LICENSE>

Distributions containing the native compatibility implementation retain this
attribution and the complete license alongside the other notices.
