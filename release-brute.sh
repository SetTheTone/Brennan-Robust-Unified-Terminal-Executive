#!/usr/bin/env bash
set -uo pipefail

# --- Configuration ---
REPO_ROOT="/home/milton/BrenSupp"
APP_DIR="$REPO_ROOT/BRUTE"

FILE_UI_CARGO="$APP_DIR/Cargo.toml"
FILE_TAURI_CARGO="$APP_DIR/src-tauri/Cargo.toml"
FILE_TAURI_CONF="$APP_DIR/src-tauri/tauri.conf.json"
FILE_APP_RS="$APP_DIR/src/app.rs"

# Colours
CYAN='\033[0;36m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

# Error handler helper
fail_with_reason() {
    local step="$1"
    local reason="$2"
    echo -e "\n${RED}[ERROR] Failed during: ${step}${NC}" >&2
    echo -e "${RED}[REASON] ${reason}${NC}\n" >&2
    exit 1
}

cd "$REPO_ROOT" || fail_with_reason "Directory Change" "Could not cd into $REPO_ROOT"

# 1. Verify all 4 target files exist
for f in "$FILE_UI_CARGO" "$FILE_TAURI_CARGO" "$FILE_TAURI_CONF" "$FILE_APP_RS"; do
    if [[ ! -f "$f" ]]; then
        fail_with_reason "File Check" "Required file not found: $f"
    fi
done

echo -e "${CYAN}=== BRUTE Release & Version Manager ===${NC}"

# 2. Fetch latest tags from GitHub
echo "Fetching latest tags from GitHub origin..."
err_out=$(git fetch --tags origin 2>&1) || fail_with_reason "Git Fetch" "$err_out"

# 3. Read present build version from src-tauri/tauri.conf.json
PRESENT_VER=$(python3 -c "import json; print(json.load(open('$FILE_TAURI_CONF'))['version'])" 2>&1) \
    || fail_with_reason "Read Present Version" "$PRESENT_VER"

# 4. Get the highest version tag from Git (stripping leading 'v')
LATEST_TAG=$(git tag -l "v[0-9]*.[0-9]*.[0-9]*" | sort -V | tail -n 1)
LATEST_REL_VER="${LATEST_TAG#v}"

if [[ -z "$LATEST_REL_VER" ]]; then
    LATEST_REL_VER="0.0.0"
fi

echo -e "Present local build version : ${YELLOW}v${PRESENT_VER}${NC}"
echo -e "Latest GitHub release tag   : ${GREEN}v${LATEST_REL_VER}${NC}"

# 5. Compare versions using sort -V
HIGHEST_VER=$(printf "%s\n%s\n" "$PRESENT_VER" "$LATEST_REL_VER" | sort -V | tail -n 1)

if [[ "$LATEST_REL_VER" != "$PRESENT_VER" && "$HIGHEST_VER" == "$LATEST_REL_VER" ]]; then
    echo -e "${YELLOW}Notice: The latest release tag (v${LATEST_REL_VER}) is newer than the present build (v${PRESENT_VER}).${NC}"
elif [[ "$LATEST_REL_VER" == "$PRESENT_VER" ]]; then
    echo -e "${YELLOW}Notice: Present build (v${PRESENT_VER}) matches the latest release tag (v${LATEST_REL_VER}).${NC}"
fi

# 6. Ask for the new build number
echo ""
read -rp "Enter the new build number (e.g. 0.0.17 or v0.0.17): " RAW_NEW_VER
NEW_VER="${RAW_NEW_VER#v}"
NEW_VER="$(echo "$NEW_VER" | xargs)" # trim whitespace

if [[ ! "$NEW_VER" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    fail_with_reason "Version Validation" "Invalid version format '$RAW_NEW_VER'. Must follow X.Y.Z (e.g. 0.0.17)."
fi

if git rev-parse "v${NEW_VER}" >/dev/null 2>&1; then
    fail_with_reason "Tag Check" "Git tag 'v${NEW_VER}' already exists. Please choose a newer version number."
fi

# 7. Ask for the build comment
read -rp "Enter a build/commit comment: " BUILD_COMMENT
BUILD_COMMENT="$(echo "$BUILD_COMMENT" | xargs)"

if [[ -z "$BUILD_COMMENT" ]]; then
    fail_with_reason "Comment Validation" "Build comment cannot be empty."
fi

# 8. Edit the 4 files with the new build number
echo -e "\n${CYAN}Updating build number to v${NEW_VER} across 4 files...${NC}"

update_out=$(python3 - "$NEW_VER" "$FILE_UI_CARGO" "$FILE_TAURI_CARGO" "$FILE_TAURI_CONF" "$FILE_APP_RS" 2>&1 << 'PYEOF'
import sys, re, json
from pathlib import Path

new_ver = sys.argv[1]
ui_cargo, tauri_cargo, tauri_conf, app_rs = [Path(p) for p in sys.argv[2:6]]

# 1 & 2: Update version in both Cargo.toml files (only under [package])
for cargo_file in (ui_cargo, tauri_cargo):
    text = cargo_file.read_text()
    new_text, count = re.subn(
        r'(^\[package\][\s\S]*?^version\s*=\s*")[^"]+(")',
        rf'\g<1>{new_ver}\2',
        text,
        count=1,
        flags=re.MULTILINE
    )
    if count == 0:
        raise RuntimeError(f"Could not locate package version in {cargo_file}")
    cargo_file.write_text(new_text)
    print(f"  [OK] Updated {cargo_file} -> {new_ver}")

# 3: Update version in src-tauri/tauri.conf.json
conf_text = tauri_conf.read_text()
new_conf, count = re.subn(
    r'("version"\s*:\s*")[^"]+(")',
    rf'\g<1>{new_ver}\2',
    conf_text,
    count=1
)
if count == 0:
    raise RuntimeError(f"Could not locate \"version\" key in {tauri_conf}")
tauri_conf.write_text(new_conf)
print(f"  [OK] Updated {tauri_conf} -> {new_ver}")

# 4: Update [BRUTE vX.Y.Z] banner in src/app.rs
app_text = app_rs.read_text()
new_app, count = re.subn(
    r'(\[BRUTE\s+v)[0-9]+\.[0-9]+\.[0-9]+(\])',
    rf'\g<1>{new_ver}\2',
    app_text
)
if count == 0:
    raise RuntimeError(f"Could not locate '[BRUTE vX.Y.Z]' string in {app_rs}")
app_rs.write_text(new_app)
print(f"  [OK] Updated {app_rs} -> [BRUTE v{new_ver}]")
PYEOF
) || fail_with_reason "Updating 4 Version Files" "$update_out"

echo "$update_out"

# 9. Verify local compile metadata & update Cargo.lock before pushing
echo -e "\n${CYAN}Verifying local Rust workspace metadata...${NC}"
cargo_out=$(cargo check --manifest-path "$FILE_TAURI_CARGO" 2>&1) \
    || fail_with_reason "Local Cargo Check" "$cargo_out"
echo -e "  ${GREEN}[OK] Local Rust check passed.${NC}"

# 10. Commit, Tag, and Push to trigger the GitHub Actions Release Build
echo -e "\n${CYAN}Committing and pushing v${NEW_VER} to trigger build...${NC}"

git_add_out=$(git add -A 2>&1) || fail_with_reason "git add" "$git_add_out"

git_commit_out=$(git commit -m "v${NEW_VER}: ${BUILD_COMMENT}" 2>&1) \
    || fail_with_reason "git commit" "$git_commit_out"

git_push_out=$(git push origin main 2>&1) \
    || fail_with_reason "git push origin main" "$git_push_out"

git_tag_out=$(git tag "v${NEW_VER}" 2>&1) \
    || fail_with_reason "git tag v${NEW_VER}" "$git_tag_out"

git_push_tag_out=$(git push origin "v${NEW_VER}" 2>&1) \
    || fail_with_reason "git push origin v${NEW_VER}" "$git_push_tag_out"

echo -e "\n${GREEN}Success! Version v${NEW_VER} committed and pushed.${NC}"
echo -e "GitHub Actions is now building BRUTE v${NEW_VER}: https://github.com/SetTheTone/Brennan-Robust-Unified-Terminal-Executive/actions"
