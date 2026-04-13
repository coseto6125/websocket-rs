# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
