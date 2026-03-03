#!/bin/bash
# release.sh - Interactive release script for Linkly AI CLI
# Updates version in Cargo.toml, commits, tags, and pushes to trigger CI/CD.
# Usage: ./scripts/release.sh

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'

# State
CURRENT_VERSION=""
NEW_VERSION=""

# ============================================================================
# Helper Functions
# ============================================================================

print_success() { echo -e "${GREEN}[✓]${NC} $1"; }
print_error() { echo -e "${RED}[✗]${NC} $1"; }

# ============================================================================
# Main Functions
# ============================================================================

check_workdir() {
  echo -e "${BOLD}Step 1: Check${NC}"

  if [[ -n $(git status -s) ]]; then
    print_error "Working directory has uncommitted changes"
    git status -s
    exit 1
  fi
  print_success "Working directory clean"

  # Read version from Cargo.toml [package] section
  CURRENT_VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/version = "\(.*\)"/\1/')
  print_success "Current version: $CURRENT_VERSION"
  echo ""
}

select_version() {
  echo -e "${BOLD}Step 2: Select Version${NC}"

  IFS='.' read -r major minor patch <<< "$CURRENT_VERSION"
  local patch_ver="$major.$minor.$((patch + 1))"
  local minor_ver="$major.$((minor + 1)).0"
  local major_ver="$((major + 1)).0.0"

  echo "  1) patch  -> $patch_ver"
  echo "  2) minor  -> $minor_ver"
  echo "  3) major  -> $major_ver"
  echo ""
  read -r -p "Select [1-3]: " choice

  case "$choice" in
    1) NEW_VERSION="$patch_ver" ;;
    2) NEW_VERSION="$minor_ver" ;;
    3) NEW_VERSION="$major_ver" ;;
    *) print_error "Invalid choice"; exit 1 ;;
  esac

  print_success "Selected: $CURRENT_VERSION -> $NEW_VERSION"
  echo ""
}

generate_release_notes() {
  echo -e "${BOLD}Step 3: Release Notes${NC}"

  local last_tag notes
  last_tag=$(git describe --tags --abbrev=0 2>/dev/null || echo "")

  if [ -n "$last_tag" ]; then
    notes=$(git log "${last_tag}..HEAD" --pretty=format:"- %s" --no-merges)
    echo "Since $last_tag:"
  else
    notes=$(git log --pretty=format:"- %s" --no-merges -10)
    echo "Recent commits:"
  fi

  if [ -z "$notes" ]; then
    echo -e "  ${DIM}(no commits since last tag)${NC}"
  else
    echo "$notes" | head -15
  fi
  echo ""

  echo "  1) Continue"
  echo "  2) Abort"
  read -r -p "Select [1-2]: " choice
  if [[ "$choice" == "2" ]]; then
    echo "Aborted."
    exit 0
  fi
  echo ""
}

update_cargo_version() {
  # Update version in Cargo.toml — replace only the first 'version = "..."' (under [package])
  # Uses awk for cross-platform compatibility (BSD sed doesn't support 0,/pattern/ ranges)
  awk -v new="$NEW_VERSION" '
    done==0 && /^version = ".*"/ { sub(/"[^"]*"/, "\"" new "\""); done=1 }
    { print }
  ' Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml

  # Regenerate Cargo.lock
  cargo generate-lockfile --quiet 2>/dev/null || true
}

confirm_and_execute() {
  echo -e "${BOLD}Step 4: Confirm${NC}"
  echo -e "  Version: $CURRENT_VERSION -> ${BOLD}$NEW_VERSION${NC}"
  echo "  Tag:     v$NEW_VERSION"
  echo "  Files:   Cargo.toml, Cargo.lock"
  echo "  Action:  Commit, tag, push to origin -> triggers CI/CD"
  echo ""

  read -r -p "Type 'yes' to release: " response
  if [[ "$response" != "yes" ]]; then
    echo "Cancelled."
    exit 0
  fi

  echo ""
  echo -n "Updating version... "
  update_cargo_version
  echo -e "${GREEN}OK${NC}"

  echo -n "Committing and tagging... "
  git add Cargo.toml Cargo.lock
  git commit -m "chore: release v$NEW_VERSION" > /dev/null
  git tag "v$NEW_VERSION"
  echo -e "${GREEN}OK${NC}"

  local branch
  branch=$(git rev-parse --abbrev-ref HEAD)

  echo -n "Pushing branch ($branch)... "
  if ! git push origin "$branch" 2>&1; then
    echo -e "${RED}FAILED${NC}"
    echo ""
    echo "Manual recovery:"
    echo "  git push origin $branch"
    echo "  git push origin v$NEW_VERSION"
    exit 1
  fi
  echo -e "${GREEN}OK${NC}"

  echo -n "Pushing tag (triggers CI/CD)... "
  if ! git push origin "v$NEW_VERSION" 2>&1; then
    echo -e "${RED}FAILED${NC}"
    echo ""
    echo "Manual recovery:"
    echo "  git push origin v$NEW_VERSION"
    exit 1
  fi
  echo -e "${GREEN}OK${NC}"

  echo ""
  echo -e "${GREEN}Released v$NEW_VERSION${NC}"
  echo -e "${DIM}CI/CD will build and publish binaries automatically.${NC}"
}

# ============================================================================
# Main
# ============================================================================

echo ""
echo -e "${BOLD}Linkly AI CLI Release${NC}"
echo "─────────────────────"
echo ""

check_workdir
select_version
generate_release_notes
confirm_and_execute
