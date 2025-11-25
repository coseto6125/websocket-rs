# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.1] - 2025-11-25

### Added
- Per-connection event loop cache to reduce Python C API calls
- ReadyFuture error path optimization
- Performance testing suite

### Changed
- Updated `get_event_loop()` to `get_running_loop()` for Python 3.10+ compatibility
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

## [0.2.0] - 2024-XX-XX

### Added
- Initial async and sync client implementations
- Actor pattern architecture
- PyO3 bindings
- Basic documentation

### Performance
- Async RR: 0.222ms
- Pipelined (100 msgs): 5.715ms
- Sync: 0.137ms
