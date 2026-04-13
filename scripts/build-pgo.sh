#!/usr/bin/env bash
# Two-stage PGO (Profile-Guided Optimization) build.
#
# Stage 1 builds an instrumented wheel. Running the benchmark suite against
# it emits .profraw files that record which code paths are hot. Stage 2
# rebuilds using those samples so rustc can specialise branch layout / inline
# decisions for the real workload.
#
# Typical win on callback-heavy small-message pipelined workloads: 10-25 %.
# Flat on larger-message paths.
#
# Usage:
#   scripts/build-pgo.sh        # full cycle
#   scripts/build-pgo.sh clean  # remove profile data
set -euo pipefail

PROFDIR="${PROFDIR:-/tmp/websocket-rs-pgo}"
PROFDATA="${PROFDIR}/merged.profdata"
LLVM_PROFDATA="$(rustc --print sysroot)/lib/rustlib/$(rustc -Vv | awk '/host:/ {print $2}')/bin/llvm-profdata"
TARGET_FEATURE="${TARGET_FEATURE:-+avx2,+bmi2}"

if [[ "${1:-}" == "clean" ]]; then
  rm -rf "$PROFDIR"
  echo "Removed $PROFDIR"
  exit 0
fi

if [[ ! -x "$LLVM_PROFDATA" ]]; then
  echo "llvm-profdata missing — run: rustup component add llvm-tools-preview"
  exit 1
fi

mkdir -p "$PROFDIR"
rm -f "$PROFDIR"/*.profraw

echo "==> Stage 1: building instrumented wheel"
RUSTFLAGS="-Cprofile-generate=$PROFDIR -Ctarget-feature=$TARGET_FEATURE" \
  maturin develop --release

echo "==> Collecting profile data (benchmark + feature tests)"
python -m pytest tests/test_native_features.py -q
python tests/benchmark_three_servers.py >/dev/null || true

echo "==> Merging profile data"
"$LLVM_PROFDATA" merge -o "$PROFDATA" "$PROFDIR"/*.profraw

echo "==> Stage 2: optimised build"
RUSTFLAGS="-Cprofile-use=$PROFDATA -Ctarget-feature=$TARGET_FEATURE" \
  maturin develop --release

echo
echo "PGO build complete. Profile data: $PROFDATA"
echo "Re-run benchmarks to confirm the uplift."
