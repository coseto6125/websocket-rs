# Makefile for WebSocket-RS development

.PHONY: help install dev test bench clean build release

# Default target
help:
	@echo "WebSocket-RS Development Commands"
	@echo "================================="
	@echo "make install   - Install dependencies using uv"
	@echo "make dev       - Build in development mode"
	@echo "make test      - Run all tests"
	@echo "make bench     - Run benchmarks"
	@echo "make clean     - Clean build artifacts"
	@echo "make build     - Build release version"
	@echo "make release   - Build wheels for distribution"

# Install dependencies
install:
	@echo "📦 Installing dependencies with uv..."
	@command -v uv >/dev/null 2>&1 || (echo "❌ uv not found. Install from https://github.com/astral-sh/uv" && exit 1)
	uv venv
	. .venv/bin/activate && uv pip install -e ".[dev]"
	. .venv/bin/activate && uv pip install maturin

# Development build
dev:
	@echo "🔨 Building in development mode..."
	. .venv/bin/activate && maturin develop

# Release build
build:
	@echo "🚀 Building in release mode..."
	. .venv/bin/activate && maturin develop --release

# Run tests
test: build
	@echo "🧪 Running tests..."
	. .venv/bin/activate && python tests/test_compatibility.py
	. .venv/bin/activate && python tests/test_monkeypatch.py

# Run benchmarks
bench: build
	@echo "📊 Running benchmarks..."
	. .venv/bin/activate && python tests/benchmark_optimized.py
	@echo ""
	. .venv/bin/activate && python tests/benchmark_latency.py

# Clean build artifacts
clean:
	@echo "🧹 Cleaning build artifacts..."
	rm -rf target/
	rm -rf dist/
	rm -rf *.egg-info
	rm -rf .pytest_cache/
	rm -rf __pycache__/
	rm -rf **/__pycache__/
	find . -name "*.so" -delete
	find . -name "*.pyd" -delete

# Build distribution wheels
release:
	@echo "📦 Building distribution wheels..."
	. .venv/bin/activate && maturin build --release

# Quick test (no server needed)
quick-test: build
	@echo "🧪 Running quick tests (no server required)..."
	. .venv/bin/activate && python tests/test_monkeypatch.py