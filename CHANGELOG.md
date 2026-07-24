# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
