# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.7.8] - 2026-08-26

### Fixed

- **`ws://[::1]:port/...` URIs are now connectable on every client.** `parse_ws_uri` kept the URL crate's bracketed IPv6 literal, and `getaddrinfo("[::1]")` fails — so IPv6 loopback (and any literal IPv6 host) raised `gaierror` before a socket was ever opened. `parse_ws_uri` now strips the brackets; `build_handshake` re-brackets the literal for the `Host` header as RFC 6874 requires. The sync client had the same failure through a second path — `http::Uri::host()` keeps the brackets, so its dialer fed `"[::1]"` to `getaddrinfo` and rustls; both now get the bare literal. The unit test asserting this had existed since the monolith but was unrunnable (below), so the drift went unnoticed.

- **Rust unit tests revived**: the #44 module split left `mod.rs`'s test imports pointing at pre-split paths (`cargo test --lib` failed with E0432; all 13 pre-existing tests dead, plus two new `build_handshake` Host-header tests). Imports now name their `codec::` / `protocol::` homes. pyo3's `extension-module` moved behind a default feature so `cargo test --lib --no-default-features` can link libpython; maturin builds keep the default and behave identically. CI now runs the suite in both workflows.

### Changed

- **Sync client dials eagerly**: `websocket_rs.sync.client.connect()` returns a connected client, matching native/async semantics. Re-entering `with` is a no-op instead of re-dialing over the live socket.
- **Sync client rejects unknown keyword arguments** with `TypeError`. It previously accepted `**kwargs` silently, so `headers=`, `proxy=` etc. disappeared without effect.
- **Sync client accepts `subprotocols=`**, matching native/async; the negotiated value surfaces on the new `subprotocol` property. Previously a silent no-op. On `SyncClientConnection` and `sync.client.connect()` it sits **last** in the parameter list; keyword callers are unaffected.
- **Sync `ping()`/`pong()` reject payloads over 125 bytes** with `ValueError`, matching native. tungstenite only validates control-frame size on read, so oversized pings used to go on the wire until an RFC-compliant server killed the connection with 1002.

### Added

- **Native client surface parity**: `NativeClient` gains `pong()` (same 125-byte limit as `ping()`), plus `closed`, `local_address`, `remote_address` properties. `SyncClientConnection` gains `subprotocol` (captured from the handshake response). All three clients now expose the same core introspection surface; differences that remain are deliberate and documented (native close is fire-and-forget, so `close_timeout` applies only to sync).

### Internal

- `scripts/test.sh` no longer references deleted files (`test_monkeypatch.py`, `benchmark_optimized.py`, `benchmark_latency.py`); it runs the same pytest suite as CI. `make bench` points at the existing `tests/bench_ab.py`. Docs corrected: API.md module paths (`websocket_rs.sync.client`), timeout defaults (10.0), README proxy/close semantics. Benchmarks use `inspect.iscoroutinefunction` ahead of the `asyncio` deprecation.

## [0.7.7] - 2026-08-23

### Internal

- **The three receive fast paths now share one frame-walking core**: `data_received`, the zero-copy `PyBytes` variant, and `parse_recv_data` each carried their own copy of the opcode dispatch loop, and 0.7.6's ping fix had to be applied to all three by hand. They now delegate to a single `scan_frame_aligned` visitor next to `ProtocolCore`'s walker, so ping answering, close handling, and message delivery have one implementation instead of four near-copies. One observable timing change: the buffered `parse_recv_data` window used to write each pong mid-scan; it now queues pongs until the scan releases the State borrow, matching the zero-copy path, the plain-chunk path, and ProtocolCore's "pong, then close" event order (a mid-scan error discards queued pongs, exactly as the zero-copy path already did). Three new tests pin ping-then-message, ping-then-close ordering, and a ping split across a buffered window boundary.

- **Future-resolution guards and protocol-error teardown consolidated**: the four inline `!future.done()` probe-then-resolve sites became `set_future_result` / `set_future_exception`, which name the two policies (resolve even if the probe loses a race, never throw during teardown), and protocol-error close framing moved into `begin_protocol_error`, mirroring `begin_peer_close`. One narrow error-path delta on top of the disclosed set: at the handshake-completion site the whole resolve now goes through `set_future_result` with its result discarded, so a `done()`-probe error is swallowed there too (pre-refactor only a `set_result` failure was swallowed; the probe error propagated). Unreachable through stock asyncio (`Future.done()` does not raise); noted for instrumentation that wraps futures.

- **native_client.rs split into modules**: the 2481-line file is now a directory module — `codec.rs` (frame primitives: masking, header parse, frame walk), `protocol.rs` (ProtocolCore state machine, handshake accept key, permessage-deflate decode), `client.rs` (pyclass bindings, State, send-side control frames), and `mod.rs` (`connect()`, URI parsing, registration, unit tests). The ~110-line embedded SOCKS5 connect helper moves from an `r#"..."#` literal to `include_str!("connect_helper.py")`, so it is visible to editors and linters.

- **No performance change measured**: paired interleaved A/B against the 0.7.6 release binary (21 rounds per cell, bootstrap 95% CI) puts every cell inside noise — plain transport medians +0.34% at 256 B [-0.60%, +1.58%], −0.20% at 8 KiB [-0.59%, +0.20%], +0.18% at 100 KiB [-0.17%, +0.99%], +0.20% at 1 MiB [-0.64%, +0.56%]; TLS 1 MiB −0.34% [-1.32%, +0.42%]. All intervals straddle zero; none meets or breaches the ±2% gate. The split .so is also ~3 KB smaller than 0.7.6's.

## [0.7.6] - 2026-08-22

### Fixed

- **Unfragmented server pings are now answered on every receive path**: the three receive fast paths — plain `data_received` chunks, the zero-copy `PyBytes` variant, and the buffered `parse_recv_data` window — skipped opcode 0x9 entirely, while the `ProtocolCore` slow path answered pings with a masked pong. Whether a ping got a reply depended on which path the frame happened to take (buffered transports, TLS chunk sizes, and paused writers all route differently). The fast paths now queue a masked pong built by the same `encode_control_frame` used elsewhere, written after the `State` borrow is released. The strict xfail test that pinned this gap is replaced by parametrized tests covering all three fast paths plus the existing slow-path coverage.

### Documentation

- The native module docstring still described the 0.1 MVP scope ("ping/pong, fragmented messages, permessage-deflate deliberately NOT in this commit") while the module implements all of them.
- MIGRATION.md claimed SOCKS5 was "not yet ported to the native client"; it is available as `proxy=` on `websocket_rs.connect`.
- The deprecated async client is now marked as such everywhere it is taught: a deprecation notice at the top of docs/API.md and on its reference section, a note in the `websocket_rs.async_client` type stub, and README quick-start examples switched to the canonical `websocket_rs.connect`.

### Internal

- `copy_masked_fallback`'s 4-byte loop adopts the `as_chunks` form requested by clippy 1.98 (`chunks_exact_to_as_chunks`), which this repository's unpinned CI toolchain now ships. Same iteration, same loads, byte-identical output; the mask test matrix covers every size class including non-multiple-of-4 tails.

## [0.7.5] - 2026-08-09

### Performance

- **Outbound frames masked in a single pass (#39)**: sending a frame copied the payload into the outbound buffer and then XOR-masked it in place, touching every byte twice. `copy_masked` now reads the source and writes the masked destination in one pass, on both the raw-fd fast path and the merged-frame path used when the transport is paused or has no raw fd (which includes every `wss://` send).

  Isolated on the primitive, fused masking is 1.37x faster at 1 MiB and ~2x at 8 KiB and below, with byte-identical output. End-to-end the win is bounded by masking's share of the round trip, so it only shows up on large frames: 1 MiB request-response is **+2.55% median (66 paired rounds, 95% CI [+1.76%, +3.67%])**. Measured against the exact shipped binary the last 15 of those rounds give +2.99% (95% CI [+1.92%, +5.04%], 13/15 positive). The measured 4.75 µs saved per 1 MiB send is 2.0% of the 238 µs round trip, which is what the end-to-end number independently reproduces.

  256 B and 8 KiB are flat (+0.06% median, 44 paired rounds, 95% CI [-0.68%, +0.62%]) — masking is too small a share of those round trips to move. The **+2% gate is met at 1 MiB only**, and the host was not idle during measurement (other workloads at load average ~1), which is why the confidence interval is reported rather than a bare median.

  `copy_masked` asserts that its two slices are the same length, because the AVX-512 kernel takes its loop bound from the destination and its loads from the source: a shorter source would read out of bounds, and a `debug_assert!` would have been compiled out of the released wheel. All three call sites pass equal lengths today, so this guards a future one. The panic is a cold out-of-line call, and the guard measured neutral against the unguarded build (+0.62% median, 17 paired rounds, 95% CI [-0.55%, +3.96%]).

### Internal

- **CI runs the whole test suite (#38)**: both workflows ran `tests/test_compatibility.py` and `tests/test_timeout_and_errors.py` as scripts, leaving `tests/test_native_features.py` — the file holding the frame-level receive, fragmentation, ping/pong, compression and SOCKS5 coverage — out of CI entirely. They now run `pytest tests/`, which collects all three. Verified against a deliberately corrupted outbound-masking build: the old command passed all 26 tests, the new one fails 23.

  `pytest-asyncio` is now named in both install lists too. It backs the `asyncio_mode = auto` in `pytest.ini`, and without it every `async def` test fails outright; it happened to be present already because `maturin develop` syncs `uv.lock`, which is not a dependency worth leaning on silently.

  What kept that file out was an unguarded module-level `import uvloop`, which cannot resolve on the Windows leg of the `test.yml` matrix. uvloop is installed where it exists and CPython's own loop is used otherwise, matching the idiom `test_timeout_and_errors.py` already used, so the tests run on every platform rather than being skipped on one. `tests/bench_socks5_handshake.py` installed uvloop at import time as well, which the SOCKS5 tests inherited just by importing its proxy helper; that install moved into the benchmark's `__main__`, leaving the helper import side-effect free.

- **One masking primitive instead of two (#39)**: `apply_mask` and its scalar/AVX-512 pair existed only to serve control frames once `copy_masked` took over the data path. Control-frame encoding now uses `copy_masked` too, and the duplicate AVX-512 kernel is deleted — the Rust side is net smaller while doing strictly more.

- **Outbound masking pinned byte-for-byte (#39)**: a regression suite sweeps payload sizes across the 1-/2-/8-byte length headers and every shape the vectorised loop can take (empty, sub-word, sub-vector, exact 64-byte multiples, and multiples plus a scalar-tail remainder), decoding the frame off the wire and unmasking it. Verified to have teeth: dropping the scalar tail from the AVX-512 path fails 12 of its 15 cases. The scalar kernel is unreachable on an AVX-512 host, so it is the ARM legs of the CI matrix that exercise it — which #38 is what makes possible.

- **A/B benchmark harness (`tests/bench_ab.py`) (#40)**: the existing benchmark scripts answer "how does websocket-rs compare to other clients"; none of them answer "did my change help", which is the question the +2% CHANGELOG gate is actually about. Measuring that by hand is where the time goes: the 0.7.5 masking change measured +8.02%, +4.62%, +2.40%, +2.20% and +2.69% across five sessions on this machine, and believing the first would have put a wrong figure in the CHANGELOG.

  The harness takes two built `.so` files and runs paired interleaved rounds: both builds back to back against the same server process, with the arm order alternating each round, reported as the median of per-round ratios with a percentile bootstrap confidence interval. Each cell runs in its own interpreter with its own staged copy of the package, so the two builds never share a process and the working tree's `.so` is untouched while a comparison runs.

  `--transport tls` drives the wss:// path, which is a different code path rather than just a slower one: no raw fd means every send goes through `build_merged_frame`. `make bench-servers` builds both echo servers. Calibrated by passing one build as both arms (+0.16% / -0.09% at 256 B / 1 MiB, intervals containing zero) and by reproducing a known effect (the 0.7.5 masking change, +4.08% at 1 MiB, 11/11 rounds positive).

- **Measured findings for the send and receive paths (#41)**: two directions that looked open are now measured rather than estimated, using `tests/bench_ab.py`.

  The TLS send path does not benefit from single-pass masking. `wss://` has no raw fd, so every send goes through `build_merged_frame`, and 0.7.5's change does run there — it just cannot be seen: TLS 1 MiB round-trip time is 791 µs against plain's 231 µs, so the same 4.75 µs saving is 0.60% instead of 2.05% and falls under the noise floor (-0.95%, 15 rounds, 95% CI [-5.86%, +1.50%]). Prediction and measurement agree at all four points tested. Memory-pass optimizations on that path have roughly a third of the leverage they have on plain, which is now a documented reason not to try.

  The receive-path `Bytes::copy_from_slice` cost is the pass, not the allocation. Removing the per-message allocation entirely measured -0.98% (95% CI [-2.25%, +0.35%]) — nothing, because the allocator hands the just-freed same-size block straight back. A footprint-controlled probe that changes only the number of passes measured -7.58% (95% CI [-9.48%, -2.07%]) at 1 MiB and nothing at 100 KiB, which puts the ceiling on removing that copy at **+7.6%**, not the +10-18% previously estimated.

  Both results are written up in `docs/OPTIMIZATION_RESEARCH.md`, including why the abandoned owned-slab design's memory-bound objection does not apply once a size threshold restricts it to one message per buffer, and which test the naive version breaks.

## [0.7.4] - 2026-07-29

### Fixed

- **`__version__` reported 0.7.1 (#TBD)**: the Python package hard-coded a version literal that was never bumped, so `websocket_rs.__version__` read `0.7.1` through the 0.7.2 and 0.7.3 releases while the installed distribution said otherwise. It now derives from the compiled module's `CARGO_PKG_VERSION`, leaving `Cargo.toml` as the single place a version is written by hand (`pyproject.toml` still mirrors it for the build backend).

- **`eof_received` on the native protocol (#TBD)**: `NativeClient` implements asyncio's protocol callbacks by hand — it is a pyclass and inherits nothing from `asyncio.Protocol`, so the base class's default was never available — and `eof_received` was missing. When the peer half-closes, the transport calls it unguarded and the resulting `AttributeError` was raised inside asyncio's own callback: CPython routes it into `_fatal_error` ("Fatal error: protocol.eof_received() call failed."), so it surfaced as a connection error rather than a warning.

  Affects every combination except plain TCP under uvloop, whose `_on_eof` is the only call site that guards with `try/except AttributeError`. Both TLS stacks (CPython `sslproto._call_eof_received`, uvloop `sslproto.pyx` `_call_eof_received`) call it directly, so `wss://` was affected under both event loops. Client-closes-first tests never reach this path, which is why it went unnoticed.

  `NativeClientBuffered` inherits the fix. The regression test lives in `tests/test_timeout_and_errors.py` (one of the two files CI actually runs) and covers the loop × transport matrix; CI now installs uvloop everywhere it has wheels, so `uvloop + wss` is exercised there too. After the peer closes, a second `recv()` must fail immediately rather than hang to the timeout, which is what proves `connection_lost` still fails pending receives. The TLS cases mint a self-signed cert into a pytest temp dir.

## [0.7.3] - 2026-07-24

### Performance

- **Sync receive without the intermediate copy (#29)**: tungstenite's owned `Bytes` payload now moves through the synchronous receive path instead of being copied into an intermediate `Vec<u8>`. Measured medians on an idle host: plain 1 MiB +9.1%, TLS 1 MiB +27.9%.

- **Sync send borrows exact `bytes` (#30)**: exact CPython `bytes` payloads are sent through an owner-backed `Bytes` with no extraction copy; `bytearray`, `memoryview`, other buffer-protocol objects, and `bytes` subclasses keep the existing copy path. Pooled 7-round median: plain 1 MiB +7.2%.

- **Interned Future-delivery identifiers (#31)**: `done` / `set_result` lookups in the hot async receive path use interned strings. Merged under the documented micro-optimization exception — the +2% performance gate was **not** met (~+1% on TLS 256 B, 7/7 positive rounds); this entry must not be cited as a perf-gate pass.

### Fixed

- **Interrupted socket I/O in the sync client (#34)**: blocking read/write/flush now follow PEP 475 — on `EINTR` the client checks pending Python signals first (so Ctrl-C still raises `KeyboardInterrupt`) and then retries, and an overall receive deadline is enforced across retries so `receive_timeout` is never extended by signal storms. Previously any signal delivered during a blocking `recv()` surfaced `RuntimeError: Interrupted system call`.

### Internal

- **Owner invariants compiler-enforced (#32)**: the sync-send `PyBytes` owner moved into a private module with a single constructor, making its safety invariants unconstructible-wrong; mutate-after-send tripwire tests added for mutable input types.

- **Peer-close unified behind one seam (#33)**: close parsing and effects flow through `parse_close_payload` + `begin_peer_close` / `apply_peer_close` on every path (slow-path event sink, all three fast-path adapters, `connection_lost`, client `close()`). The remaining future-failure and transport calls made under an active `RefCell` borrow were eliminated — the same reentrancy discipline the 0.7.2 event sink introduced — and `handle_close_frame` / `fail_all_pending` were deleted outright.

## [0.7.2] - 2026-07-24

### Fixed

- **Async client `connect()` timeout kwargs (#24)**: `connect_timeout` / `receive_timeout` passed to the pure-Python async client's `connect()` were silently dropped; they are now honored.

- **Async client reserved-header filtering (#24)**: user-supplied extra headers that collide with reserved WebSocket handshake headers are now filtered by the async client, matching the native client's behavior.

- **Native SOCKS5 proxy short reads (#24)**: proxy handshake replies are now read with an exact-length loop, fixing connection failures when the reply arrived split across TCP segments.

- **Slow-path Ping panic under backpressure (#27)**: receiving a Ping on the buffered slow path while the transport applies write backpressure (`pause_writing` reentry) no longer panics. Protocol decisions and transport effects are now decoupled — all writes and future completions run after internal state borrows are released, with a regression test pinning the invariant.

### Changed

- **Native `connect_timeout` default (#24)**: `connect_timeout=None` now applies the 10-second default (aligned across all three clients) instead of allowing an indefinite connect hang. `receive_timeout=None` remains unlimited by design — idle connections that legitimately wait minutes between messages are unaffected.

### Removed

- **Leaked internal methods (#25)**: `parse_recv_data`, `data_received_inner`, `data_received_inner_pybytes`, `flush_pending_callbacks`, and `build_merged_frame` are no longer exposed as Python methods on the native client. They were undocumented implementation details.

### Internal

- **Receive-path characterization suite (#23)**: slow/fast-path protocol behavior — including known quirks — is pinned as regression tests before any restructuring.

- **Unified frame scanner (#26)**: the three native fast-path frame walks now share one monomorphized visitor (`walk_frames`); each adapter keeps its payload-ownership strategy. Benchmarks held or improved (all 16 parity cells ≥ baseline, several +3–11%).

- **`ProtocolCore` extraction (#27)**: the buffered handshake/frame state machine is now a pure bytes-in/events-out core emitting through a one-event-at-a-time sink (no event `Vec`, no `Box`, no `dyn`). Protocol edge cases are unit-testable without a live socket. Performance gates held (per-cell ≥98.77%, family geomeans ≥99.7%).

## [0.7.1] - 2026-05-14

### Fixed

- **Protocol edge cases in `NativeClient` (#21)**:

  - **Hashing consistency**: Aligned `WSMessage` hashing logic with Python `bytes` equality. This ensures reliable behavior when messages are used as keys in dictionaries or stored in sets.

  - **Connection teardown**: Centralized close-frame handling to ensure `close_code` and `close_reason` are correctly preserved during fast-path processing.

  - **Malformed sequence defense**: Added strict validation for invalid fragmented-frame sequences; connections now terminate with a `ProtocolError` instead of potentially entering an inconsistent state.

- **URI Parsing**: Migrated to the Rust `url` crate for robust WebSocket URI handling. Now correctly supports IPv6 literals, query parameters, and handles default ports automatically per scheme (`ws`/`wss`).



### Changed

- **Test Infrastructure**: Enabled `pytest-asyncio` integration. Refactored legacy server/async tests into standard pytest fixtures, allowing for a unified `uv run pytest` entry point with proper async collection.

- **Dependency Refresh**: Updated Rust (`Cargo.lock`) and Python dev-dependencies (`uv.lock`) to the latest compatible versions for consistent benchmark and development environments.


## [0.7.0] - 2026-04-14

### Performance

- **asyncio.BufferedProtocol + ring-buffer recv path**: kernel writes
  directly into a reusable buffer; partial frames stay in place across
  `get_buffer` calls. 64 KB pipelined: ws-rs 0.91 ms vs picows 1.07 ms
  mean (+15%), p99 tied.
- **Split `NativeClient` / `NativeClientBuffered` by scheme**: `ws://`
  uses the BufferedProtocol subclass (wins plain-TCP pipelined);
  `wss://` uses the base class (asyncio's SSLProtocol interacts poorly
  with BufferedProtocol's ≤16 KB record callbacks).
- **Lazy buffer allocation**: per-connection memory footprint dropped
  from ~130 KB to **8.9 KB** (−93%). 10K idle connections: 1.3 GB → 90 MB.
  Warm-path throughput unchanged.
- **Defer-parse gate** (`next_frame_needed`): short-circuits wasted
  parse passes when a TLS large frame is split across many records.

### Changed

- **TLS backend: `native-tls` → `rustls`** (pure-Rust, no OpenSSL).
  Eliminates the process-wide OpenSSL global-state conflict that caused
  picows to segfault when loaded alongside websocket-rs. Simpler
  cross-platform wheel builds. `SSL_CERT_FILE` env var still honored
  for users with private CAs.
- `tls-certs` Makefile target now produces end-entity certs
  (`basicConstraints=CA:FALSE`, `extendedKeyUsage=serverAuth`); rustls
  strictly rejects CA-flagged certs used as leaf, OpenSSL was lenient.

### Added

- `on_message` callback API on `native_client.connect()` — synchronous
  callback delivery for users who want to bypass the `await ws.recv()`
  per-frame Future overhead.
- `tests/benchmark_picows_parity.py`: plain-TCP RPS matrix (6 clients
  × 3 server architectures × 4 sizes).
- `tests/benchmark_tls_parity.py` + `ws_echo_server_tls` binary: TLS
  RPS matrix against a tokio-tungstenite+rustls echo server.
- `make tls-certs` target: generates self-signed cert for the TLS
  benchmark (gitignored).

### Fixed

- `next_frame_needed` threshold off-by-`off` (over-conservative defer
  gate). Previously stored `off + hdr + plen` but `recv_pos` is
  compacted by `consumed` before the next check — correct value is
  `hdr + plen`.
- `data_received` non-PyBytes path now rejects non-contiguous or
  multi-dimensional `PyBuffer` inputs that `from_raw_parts` would
  mis-read (e.g. strided NumPy slices). Returns `TypeError`.
- `pybytes_zero_copy_slice` + `PyBytesOwner`: added SAFETY docs and
  `debug_assert!` bounds checks around the raw pointer arithmetic.

### Removed

- `tests/benchmark_micro.py` — superseded by the two new benchmark
  harnesses.

### Benchmarks (v0.7.0, tokio-tungstenite server, 10s RPS)

Plain TCP: ws-rs wins or ties **12/12 cells** across 3 server
architectures. Sync wins 256 B–100 KB; async ties picows at 1 MB.

TLS (wss://): ws-rs sync wins 256 B–100 KB by 18–65%; at 1 MB picows
edges ws-rs by ~7%, all within 2σ noise at this throughput level.

Idle connection footprint: **8.9 KB/conn** (was ~130 KB in v0.6).

---

## [0.5.0] - 2026-03-19

### Added
- SOCKS5 proxy support via `proxy="socks5://host:port"` keyword argument
- Custom HTTP headers support via `headers={"key": "value"}` keyword argument
- Proxy scheme validation (only socks5:// accepted, clear error for others)
- Header name/value validation at construction time

### Changed
- `headers`, `proxy`, `connect_timeout`, `receive_timeout` are now keyword-only parameters
- Refactored background WebSocket task into generic `start_ws_task()` supporting both direct and proxy streams
- Simplified `Arc<RwLock>` usage for one-time-use fields (headers, proxy)

### Fixed
- GIL safety: ensure all `Py<T>` objects are dropped under GIL in error/timeout paths
- Restore error logging for future completion failures (previously silently swallowed)

## [0.4.1] - 2025-11-26

### Fixed
- Version number sync across all package files (__init__.py, pyproject.toml, Cargo.toml)
- Cleaned up obsolete python/ directory structure
- Improved .gitignore rules for better precision

## [0.4.0] - 2025-11-26

### Added
- Pure synchronous client implementation (websocket_rs.sync.client)
- High-performance sync API with blocking I/O
- Comprehensive benchmark suite with server timestamp validation

### Performance
- Sync client: ~50% faster than websockets.sync.client
- Request-Response: 194.32ms for 1000 messages (vs 287.71ms)
- Pipelined: 115.13ms for 1000 messages (vs 152.24ms)
- Throughput: 8,685 msgs/sec (vs 5,806 msgs/sec)

### Changed
- Project structure reorganization
- Enhanced documentation with performance benchmarks

## [0.3.1] - 2025-11-25

### Added
- Per-connection event loop cache to reduce Python C API calls
- Event loop cache write-back for non-context-manager usage (25% improvement)
- ReadyFuture error path optimization
- Performance testing suite

### Changed
- Updated `get_event_loop()` to `get_running_loop()` for Python 3.10+ compatibility
- Event loop now cached on first access regardless of usage pattern
- Error handling performance improved by ~50x
- Stability improved (standard deviation <1-2%)

### Performance
- Error path: 0.1μs per error (10M+ errors/sec)
- Request-Response: 232.64ms for 1000 messages
- Pipelined: 105.39ms for 1000 messages
- Mixed scenario: 200.03ms for 1000 messages

## [0.3.0] - 2025-11-25

### Added
- Security fixes with parking_lot::RwLock
- Error logging for all operations
- 10-second timeout on close() function
- Channel buffer optimization (256 → 64)

### Changed
- Migrated from std::sync::RwLock to parking_lot::RwLock
- Improved error handling with explicit logging
- Enhanced close() reliability

### Removed
- Unused flume dependency
- Deprecated monkeypatch files

### Fixed
- Potential panic issues across FFI boundary
- Silent error ignoring
- Missing timeout on connection close

## [0.2.0] - 2025-11-21

### Added
- Initial async and sync client implementations
- Actor pattern architecture
- PyO3 bindings
- Basic documentation

### Performance
- Async RR: 0.222ms
- Pipelined (100 msgs): 5.715ms
- Sync: 0.137ms
