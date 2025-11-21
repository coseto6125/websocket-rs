#!/bin/bash
# Script to create release tag after PR is merged

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}WebSocket-RS Release Tagging${NC}"
echo "============================="

# Check if version argument is provided
if [ -z "$1" ]; then
    echo -e "${RED}Error: Version number required${NC}"
    echo "Usage: ./scripts/tag-release.sh <version>"
    echo "Example: ./scripts/tag-release.sh 0.3.0"
    exit 1
fi

VERSION=$1
TAG_NAME="v${VERSION}"

echo -e "${YELLOW}Creating release tag ${TAG_NAME}${NC}"

# Ensure we're on main and up to date
echo "1. Checking git status..."
CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD)

if [ "$CURRENT_BRANCH" != "main" ]; then
    echo -e "${RED}Error: Must be on main branch to create release tag${NC}"
    echo "Current branch: ${CURRENT_BRANCH}"
    echo "Run: git checkout main && git pull origin main"
    exit 1
fi

# Pull latest changes
echo "2. Pulling latest changes..."
git pull origin main

# Verify version numbers match
echo "3. Verifying version numbers..."
CARGO_VERSION=$(grep "^version" Cargo.toml | head -1 | cut -d'"' -f2)
PY_VERSION=$(grep "^version" pyproject.toml | head -1 | cut -d'"' -f2)
INIT_VERSION=$(grep "__version__" websocket_rs/__init__.py | cut -d'"' -f2)

if [ "$CARGO_VERSION" != "$VERSION" ] || [ "$PY_VERSION" != "$VERSION" ] || [ "$INIT_VERSION" != "$VERSION" ]; then
    echo -e "${RED}Error: Version mismatch!${NC}"
    echo "  Cargo.toml: ${CARGO_VERSION}"
    echo "  pyproject.toml: ${PY_VERSION}"
    echo "  __init__.py: ${INIT_VERSION}"
    echo "  Expected: ${VERSION}"
    exit 1
fi

# Create and push tag
echo "4. Creating tag..."
git tag -a "${TAG_NAME}" -m "Release ${TAG_NAME}: See CHANGELOG for details"

echo "5. Pushing tag..."
git push origin "${TAG_NAME}"

echo -e "\n${GREEN}✅ Release tag ${TAG_NAME} created and pushed!${NC}"
echo ""
echo "GitHub Actions will now:"
echo "  - Build wheels for all platforms"
echo "  - Run tests"
echo "  - Create GitHub Release"
echo "  - Upload wheels to the release"
echo ""
echo "Monitor progress at:"
echo "https://github.com/coseto6125/websocket-rs/actions"
echo ""
echo "Once complete, users can install with:"
echo "  uv pip install git+https://github.com/coseto6125/websocket-rs.git@${TAG_NAME}"