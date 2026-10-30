#!/bin/bash

# Exit immediately if any command fails
set -e

PROJECT_DIR="$HOME/BrenSupp/BRUTE"
cd "$PROJECT_DIR"

echo "================================================="
echo "          BRUTE Release Automation Tool          "
echo "================================================="

# 1. Fetch latest changes from GitHub to check state
echo "[1/6] Checking remote repository status..."
git fetch origin main

# Check if local branch is behind remote
LOCAL=$(git rev-parse HEAD)
REMOTE=$(git rev-parse origin/main)
BASE=$(git merge-base HEAD origin/main)

if [ "$LOCAL" != "$REMOTE" ]; then
    if [ "$LOCAL" = "$BASE" ]; then
        echo "[!] Error: Your local branch is behind remote. Please pull changes first."
        exit 1
    elif [ "$REMOTE" = "$BASE" ]; then
        echo "[!] Warning: Your local branch has unpushed commits."
    else
        echo "[!] Conflict Error: Local and remote branches have diverged. Aborting to avoid conflicts."
        exit 1
    fi
fi

# 2. Check for uncommitted local changes
if ! git diff-index --quiet HEAD --; then
    echo "[!] Error: You have uncommitted changes in your workspace. Please commit or stash them first."
    exit 1
fi

echo "[+] Workspace is clean and up to date."

# 3. Read current version from Cargo.toml and calculate suggested next version
CURRENT_VERSION=$(grep '^version = ' Cargo.toml | head -n 1 | cut -d'"' -f2)
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT_VERSION"
SUGGESTED_VERSION="$MAJOR.$MINOR.$((PATCH + 1))"

echo ""
echo "Current version in Cargo.toml is: $CURRENT_VERSION"
read -p "Enter the new version number [$SUGGESTED_VERSION]: " NEW_VERSION </dev/tty
NEW_VERSION=${NEW_VERSION:-$SUGGESTED_VERSION}

TAG_NAME="v$NEW_VERSION"

echo "[2/6] Updating version across all 4 project locations to $NEW_VERSION..."

# Location 1: Root Cargo.toml
sed -i "s/^version = \"[^\"]*\"/version = \"$NEW_VERSION\"/" Cargo.toml

# Location 2: src-tauri/Cargo.toml
sed -i "s/^version = \"[^\"]*\"/version = \"$NEW_VERSION\"/" src-tauri/Cargo.toml

# Location 3: src-tauri/tauri.conf.json
node -e "
const fs = require('fs');
const config = JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json', 'utf8'));
config.version = '$NEW_VERSION';
fs.writeFileSync('src-tauri/tauri.conf.json', JSON.stringify(config, null, 2) + '\n');
"

# Location 4: src/app.rs (Terminal startup banner version text)
sed -i -E "s/\[BRUTE v[0-9]+\.[0-9]+\.[0-9]+(-alpha)?\]/[BRUTE v$NEW_VERSION]/g" src/app.rs

# 4. Prompt for custom commit message (forced to /dev/tty)
echo ""
read -p "Enter git commit message [Bump version to $TAG_NAME]: " COMMIT_MSG </dev/tty
COMMIT_MSG=${COMMIT_MSG:-"Bump version to $TAG_NAME"}

# 5. Stage and commit all version updates
echo "[3/6] Committing version updates..."
git add Cargo.toml src-tauri/Cargo.toml src-tauri/tauri.conf.json src/app.rs
git commit -m "$COMMIT_MSG"

# 6. Push changes to GitHub main
echo "[4/6] Pushing changes to origin main..."
git push origin main

# 7. Create and push the Git tag
echo "[5/6] Creating git tag $TAG_NAME..."
git tag "$TAG_NAME"
git push origin "$TAG_NAME"

echo ""
echo "=============================================="
echo " [6/6] Successfully released $TAG_NAME!"
echo " Head over to GitHub Actions to watch the build."
echo "=============================================="