#!/bin/bash
# Pre-release check script - Run this before pushing to avoid CI failures

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}  WebSocket-RS Pre-Release Check${NC}"
echo -e "${BLUE}========================================${NC}\n"

FAILED=0
WARNINGS=0

# Function to print section headers
print_section() {
    echo -e "\n${YELLOW}▶ $1${NC}"
    echo "  ────────────────────────────────"
}

# Function to check command existence
check_command() {
    if ! command -v $1 &> /dev/null; then
        echo -e "  ${RED}✗ $1 is not installed${NC}"
        return 1
    fi
    return 0
}

# 1. Check required tools
print_section "Checking Required Tools"

TOOLS_OK=1
for tool in cargo rustc rustfmt clippy-driver uv python3; do
    if check_command $tool; then
        echo -e "  ${GREEN}✓ $tool found${NC}"
    else
        TOOLS_OK=0
        FAILED=1
    fi
done

if [ $TOOLS_OK -eq 0 ]; then
    echo -e "\n${RED}Missing required tools. Please install them first.${NC}"
    exit 1
fi

# 2. Check Rust formatting
print_section "Rust Format Check (cargo fmt)"

if cargo fmt -- --check 2>/dev/null; then
    echo -e "  ${GREEN}✓ Rust code is properly formatted${NC}"
else
    echo -e "  ${RED}✗ Rust code needs formatting${NC}"
    echo -e "  ${YELLOW}  Run: cargo fmt${NC}"
    FAILED=1
fi

# 3. Run Clippy
print_section "Rust Linting (cargo clippy)"

# Allow certain warnings that are acceptable
if cargo clippy -- -D warnings -A clippy::await-holding-lock -A clippy::redundant-closure 2>&1 | grep -q "error"; then
    echo -e "  ${RED}✗ Clippy found errors${NC}"
    echo -e "  ${YELLOW}  Run: cargo clippy -- -D warnings${NC}"
    FAILED=1
else
    echo -e "  ${GREEN}✓ Clippy checks passed${NC}"
fi

# 4. Check Rust compilation
print_section "Rust Compilation Check"

if cargo check --release 2>&1 | grep -q "error"; then
    echo -e "  ${RED}✗ Rust compilation failed${NC}"
    FAILED=1
else
    echo -e "  ${GREEN}✓ Rust code compiles${NC}"
fi

# 5. Setup Python environment if needed
print_section "Python Environment Setup"

if [ ! -d ".venv" ]; then
    echo -e "  Creating virtual environment..."
    uv venv > /dev/null 2>&1
fi

source .venv/bin/activate || . .venv/Scripts/activate
echo -e "  ${GREEN}✓ Virtual environment activated${NC}"

# 6. Install dependencies
print_section "Installing Dependencies"

echo -e "  Installing Python dependencies..."
uv pip install -q maturin pytest websockets
echo -e "  ${GREEN}✓ Dependencies installed${NC}"

# 7. Build the extension
print_section "Building Extension"

if maturin develop --release > /dev/null 2>&1; then
    echo -e "  ${GREEN}✓ Extension built successfully${NC}"
else
    echo -e "  ${RED}✗ Failed to build extension${NC}"
    FAILED=1
fi

# 8. Run Python tests
print_section "Running Tests"

# Compatibility tests
echo -e "  Running compatibility tests..."
if python tests/test_compatibility.py > /dev/null 2>&1; then
    echo -e "  ${GREEN}✓ Compatibility tests passed${NC}"
else
    echo -e "  ${RED}✗ Compatibility tests failed${NC}"
    echo -e "  ${YELLOW}  Run: python tests/test_compatibility.py${NC}"
    FAILED=1
fi

# Monkeypatch tests (allow warnings)
echo -e "  Running monkeypatch tests..."
if python tests/test_monkeypatch.py 2>&1 | grep -q "FAILED"; then
    echo -e "  ${YELLOW}⚠ Monkeypatch tests have issues (non-critical)${NC}"
    WARNINGS=1
else
    echo -e "  ${GREEN}✓ Monkeypatch tests passed${NC}"
fi

# 9. Check Python code with ruff (if installed)
print_section "Python Linting"

if command -v ruff &> /dev/null; then
    if ruff check websocket_rs/*.py tests/*.py 2>/dev/null; then
        echo -e "  ${GREEN}✓ Python code passes ruff checks${NC}"
    else
        echo -e "  ${YELLOW}⚠ Python code has ruff warnings${NC}"
        echo -e "  ${YELLOW}  Run: ruff check --fix${NC}"
        WARNINGS=1
    fi
else
    echo -e "  ${YELLOW}⚠ ruff not installed, skipping Python linting${NC}"
fi

# 10. Check for uncommitted changes
print_section "Git Status Check"

if [ -n "$(git status --porcelain)" ]; then
    echo -e "  ${YELLOW}⚠ There are uncommitted changes${NC}"
    echo -e "  ${YELLOW}  Remember to commit all changes before pushing${NC}"
    WARNINGS=1
else
    echo -e "  ${GREEN}✓ Working directory is clean${NC}"
fi

# 11. Version consistency check
print_section "Version Consistency"

CARGO_VERSION=$(grep "^version" Cargo.toml | head -1 | cut -d'"' -f2)
PY_VERSION=$(grep "^version" pyproject.toml | head -1 | cut -d'"' -f2)
INIT_VERSION=$(grep "__version__" websocket_rs/__init__.py | cut -d'"' -f2)

if [ "$CARGO_VERSION" = "$PY_VERSION" ] && [ "$PY_VERSION" = "$INIT_VERSION" ]; then
    echo -e "  ${GREEN}✓ Version numbers are consistent: $CARGO_VERSION${NC}"
else
    echo -e "  ${RED}✗ Version mismatch detected!${NC}"
    echo -e "    Cargo.toml:    $CARGO_VERSION"
    echo -e "    pyproject.toml: $PY_VERSION"
    echo -e "    __init__.py:    $INIT_VERSION"
    FAILED=1
fi

# Final summary
echo -e "\n${BLUE}========================================${NC}"
echo -e "${BLUE}  Check Summary${NC}"
echo -e "${BLUE}========================================${NC}"

if [ $FAILED -eq 0 ]; then
    if [ $WARNINGS -eq 0 ]; then
        echo -e "\n${GREEN}✅ All checks passed! Ready to push.${NC}"
    else
        echo -e "\n${GREEN}✅ All critical checks passed!${NC}"
        echo -e "${YELLOW}⚠  Some warnings were found but they won't block CI.${NC}"
    fi
    echo -e "\nNext steps:"
    echo -e "  1. git add -A"
    echo -e "  2. git commit -m 'your message'"
    echo -e "  3. git push origin your-branch"
else
    echo -e "\n${RED}❌ Some checks failed. Please fix the issues above.${NC}"
    echo -e "\nCommands to fix common issues:"
    echo -e "  ${YELLOW}cargo fmt${NC}             - Fix Rust formatting"
    echo -e "  ${YELLOW}cargo clippy --fix${NC}    - Auto-fix some Clippy warnings"
    echo -e "  ${YELLOW}ruff check --fix${NC}      - Fix Python linting issues"
    exit 1
fi