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
uv pip install maturin pytest websockets

# Build the Rust extension
echo "🔨 Building websocket-rs..."
maturin develop --release

# Run tests
echo "🧪 Running tests..."
echo ""

# API Compatibility tests
echo "=== Running API Compatibility Tests ==="
python tests/test_compatibility.py
echo ""

# Monkeypatch tests
echo "=== Running Monkeypatch Tests ==="
python tests/test_monkeypatch.py
echo ""

# Performance benchmarks (optional)
read -p "Run performance benchmarks? (y/n) " -n 1 -r
echo ""
if [[ $REPLY =~ ^[Yy]$ ]]; then
    echo "=== Running Performance Benchmarks ==="
    python tests/benchmark_optimized.py
    echo ""

    echo "=== Running Latency Tests ==="
    python tests/benchmark_latency.py
fi

echo "✅ All tests completed!"