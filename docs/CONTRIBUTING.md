# Contributing to WebSocket-RS

Thank you for your interest in contributing to WebSocket-RS! This document provides guidelines and workflows for contributing.

## Development Workflow

### 1. Branch Strategy

We use a feature branch workflow:

- `main` - Production-ready code
- `develop` - Integration branch for features
- `feature/*` - New features
- `fix/*` - Bug fixes
- `release/*` - Release preparation

### 2. Creating a Feature

```bash
# Start from main or develop
git checkout main
git pull origin main

# Create feature branch
git checkout -b feature/your-feature-name

# Make your changes
# ... edit files ...

# Test locally
make test

# Commit changes
git add .
git commit -m "feat: add your feature description"
```

### 3. Commit Message Convention

We follow [Conventional Commits](https://www.conventionalcommits.org/):

- `feat:` - New feature
- `fix:` - Bug fix
- `perf:` - Performance improvement
- `docs:` - Documentation only
- `style:` - Code style (formatting, missing semicolons, etc.)
- `refactor:` - Code refactoring
- `test:` - Adding or updating tests
- `chore:` - Build process or auxiliary tool changes

Examples:
```
feat: add batch operations for WebSocket messages
fix: resolve connection timeout issue
perf: optimize message serialization using zero-copy
docs: update README with monkeypatch examples
```

### 4. Testing

Before submitting a PR, ensure:

```bash
# Install dependencies
make install

# Run tests
make test

# Run benchmarks (optional)
make bench

# Check code quality
cargo clippy -- -D warnings
cargo fmt -- --check
```

### 5. Creating a Pull Request

1. Push your branch:
```bash
git push origin feature/your-feature-name
```

2. Create PR on GitHub:
   - Base: `main` (for hotfixes) or `develop` (for features)
   - Fill out the PR template completely
   - Link related issues

3. Wait for CI checks to pass

4. Request review from maintainers

## Release Process

### Version Release Workflow

1. **Create Release Branch**
```bash
git checkout develop
git pull origin develop
git checkout -b release/v0.3.0
```

2. **Update Version Numbers**
```bash
# Update versions in:
# - Cargo.toml
# - pyproject.toml
# - websocket_rs/__init__.py
```

3. **Update Documentation**
```bash
# Update README.md with new version
# Update CHANGELOG.md (if exists)
```

4. **Create PR to main**
```bash
git add .
git commit -m "chore: prepare release v0.3.0"
git push origin release/v0.3.0
```

5. **After PR Approval and Merge**
```bash
# On main branch after merge
git checkout main
git pull origin main
git tag -a v0.3.0 -m "Release v0.3.0: Description"
git push origin v0.3.0
```

6. **GitHub Actions will automatically**:
   - Build wheels for all platforms
   - Run tests
   - Create GitHub Release
   - Upload wheels to release

### Hotfix Process

For urgent fixes to production:

```bash
# Create hotfix from main
git checkout main
git pull origin main
git checkout -b fix/critical-bug

# Make fixes, update version (e.g., 0.2.0 -> 0.2.1)
# Create PR directly to main
# After merge, tag and release
```

## Development Environment Setup

### Using uv (Recommended)

```bash
# Install uv
curl -LsSf https://astral.sh/uv/install.sh | sh

# Setup environment
make install

# Build
make dev  # Development build
make build  # Release build
```

### Manual Setup

```bash
# Create virtual environment
python -m venv .venv
source .venv/bin/activate

# Install dependencies
pip install maturin pytest websockets

# Build
maturin develop --release
```

## Code Style Guidelines

### Python
- Follow PEP 8
- Use type hints where appropriate
- Maximum line length: 120 characters
- Use ruff for formatting and linting

### Rust
- Follow standard Rust conventions
- Use `cargo fmt` for formatting
- Use `cargo clippy` for linting
- Write tests for new functionality

## Getting Help

- Create an issue for bugs or feature requests
- Join discussions in GitHub Discussions
- Check existing issues before creating new ones

## License

By contributing to WebSocket-RS, you agree that your contributions will be licensed under the MIT License.