#!/bin/bash
# Test script using uv for dependency management

set -e

echo "🔧 Setting up test environment with uv..."

# Create virtual environment if not exists
if [ ! -d ".venv" ]; then
    echo "Creating virtual environment..."
    uv venv
fi

# Activate virtual environment
source .venv/bin/activate || . .venv/Scripts/activate

# Install dependencies
echo "📦 Installing dependencies..."
uv pip install --upgrade pip
uv pip install maturin pytest pytest-asyncio websockets

# Build the Rust extension
echo "🔨 Building websocket-rs..."
maturin develop --release

# Run the same suite CI runs (pytest discovers everything under tests/).
# Benchmarks are separate: make bench (tests/bench_ab.py).
echo "🧪 Running tests..."
python -m pytest tests/ "$@"

echo ""
echo "✅ All tests completed!"