#!/bin/bash
# Script to prepare a new release

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}WebSocket-RS Release Preparation${NC}"
echo "===================================="

# Check if version argument is provided
if [ -z "$1" ]; then
    echo -e "${RED}Error: Version number required${NC}"
    echo "Usage: ./scripts/prepare-release.sh <version>"
    echo "Example: ./scripts/prepare-release.sh 0.3.0"
    exit 1
fi

VERSION=$1
BRANCH_NAME="release/v${VERSION}"

echo -e "${YELLOW}Preparing release v${VERSION}${NC}"

# Ensure we're on main and up to date
echo "1. Checking git status..."
git fetch origin
CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD)

if [ "$CURRENT_BRANCH" != "main" ]; then
    echo -e "${YELLOW}Warning: Not on main branch. Current branch: ${CURRENT_BRANCH}${NC}"
    read -p "Switch to main branch? (y/n) " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        git checkout main
        git pull origin main
    else
        echo -e "${RED}Aborted. Please switch to main branch first.${NC}"
        exit 1
    fi
fi

# Create release branch
echo "2. Creating release branch..."
git checkout -b "${BRANCH_NAME}"

# Update version in Cargo.toml
echo "3. Updating Cargo.toml..."
sed -i "s/^version = \".*\"/version = \"${VERSION}\"/" Cargo.toml

# Update version in pyproject.toml
echo "4. Updating pyproject.toml..."
sed -i "s/^version = \".*\"/version = \"${VERSION}\"/" pyproject.toml

# Update version in __init__.py
echo "5. Updating __init__.py..."
sed -i "s/__version__ = \".*\"/__version__ = \"${VERSION}\"/" websocket_rs/__init__.py

# Update version references in README
echo "6. Updating README.md..."
OLD_VERSION=$(grep -oP 'v\d+\.\d+\.\d+' README.md | head -1 | sed 's/v//')
sed -i "s/${OLD_VERSION}/${VERSION}/g" README.md
sed -i "s/websocket_rs-${OLD_VERSION}/websocket_rs-${VERSION}/g" README.md

# Show changes
echo -e "\n${GREEN}Version updated to ${VERSION} in:${NC}"
echo "  - Cargo.toml"
echo "  - pyproject.toml"
echo "  - websocket_rs/__init__.py"
echo "  - README.md"

# Run tests
echo -e "\n7. Running tests..."
if command -v uv &> /dev/null; then
    echo "Using uv to run tests..."
    uv venv || true
    source .venv/bin/activate
    uv pip install maturin pytest websockets
else
    echo "Using pip to run tests..."
    pip install maturin pytest websockets
fi

maturin develop --release
python tests/test_compatibility.py
python tests/test_monkeypatch.py

echo -e "\n${GREEN}✅ Tests passed!${NC}"

# Commit changes
echo -e "\n8. Committing changes..."
git add Cargo.toml pyproject.toml websocket_rs/__init__.py README.md
git commit -m "chore: prepare release v${VERSION}"

# Push branch
echo -e "\n9. Pushing release branch..."
git push origin "${BRANCH_NAME}"

echo -e "\n${GREEN}🎉 Release preparation complete!${NC}"
echo ""
echo "Next steps:"
echo "1. Go to https://github.com/coseto6125/websocket-rs"
echo "2. Create a Pull Request from '${BRANCH_NAME}' to 'main'"
echo "3. Wait for CI checks to pass"
echo "4. Get approval and merge"
echo "5. After merge, run: ./scripts/tag-release.sh ${VERSION}"