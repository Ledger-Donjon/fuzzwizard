#!/usr/bin/env bash
# SPDX-FileCopyrightText: © 2026 LEDGER SAS
# SPDX-License-Identifier: MIT
#
# Creates a scheduler for a given project using a config file:
# - scheduler_<project_name> directory under the path you provide
# - scheduler config JSON
# - daily fuzzer update script via GitHub App (workflow artifacts)
#
# Run without arguments: prompts for existing config or generates an example.
# Run with config path: ./install.sh /path/to/config_install

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALL_ROOT="$(cd "$SCRIPT_DIR" && pwd)"

# Default config path (can be overridden by argument)
CONFIG_FILE=""

usage() {
    echo "Usage: $0 [config_file]"
    echo ""
    echo "Creates a scheduler for the project defined in config_install:"
    echo "  - scheduler_<project_name>/ under the path set in the config"
    echo "  - config.json (scheduler config)"
    echo "  - update_fuzzers_from_github.sh (daily update via GitHub App)"
    echo ""
    echo "If no config_file is given, the script will ask whether you already"
    echo "have a config; if not, it generates an example and asks where to save it."
    echo ""
    echo "Example: $0"
    echo "Example: $0 ./my_config_install"
    exit 1
}

# Parse optional config path
if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
    usage
fi
if [ -n "${1:-}" ]; then
    CONFIG_FILE="$1"
fi

# Ask for config or generate example
if [ -z "$CONFIG_FILE" ]; then
    echo ""
    read -r -p "Do you already have a config file? (y/n) [n]: " HAS_CONFIG
    HAS_CONFIG="${HAS_CONFIG:-n}"
    if [[ "$HAS_CONFIG" =~ ^[yY] ]]; then
        read -r -p "Enter the path to your config file: " CONFIG_FILE
        if [ -z "$CONFIG_FILE" ]; then
            echo "Error: config file path is required."
            exit 1
        fi
    else
        # Generate example config content
        EXAMPLE_CONFIG="# Fuzzwizard scheduler install config
# Copy this file, fill in the values, and run: $0 <path_to_this_file>

# Project name (alphanumeric, underscores, hyphens)
project_name=my_project

# Directory where the scheduler will be created (scheduler_<project_name> will be created inside)
scheduler_base_dir=/path/to/schedulers

# Use Docker/Podman to run fuzzers? (y/n)
use_docker=n

# Container image name (only used if use_docker=y)
container_image=fuzzing-image:latest

# Generate coverage? (y/n)
has_coverage=n

# Add this scheduler to the main config.json for FuzzTUI? (y/n)
add_to_main_config=n

# Path to main config.json (optional; default: config.json in install directory)
# main_config_path=$INSTALL_ROOT/config.json

# --- GitHub (for update_fuzzers_from_github.sh via GitHub App) ---
# GitHub App ID
github_app_id=
# Path to the App private key .pem file
github_app_key_file=
# Installation ID (optional; inferred from repo if not set)
github_installation_id=
# Repository owner/repo (e.g. my-org/my-repo)
github_repo=
# Workflow name or filename to target (optional)
github_workflow_name=
# Comma-separated artifact names to download (optional; empty = all artifacts)
# github_artifact_names=my-fuzzers,coverage
"
        echo ""
        echo "Example config (save this to a file, then run $0 <path_to_file>):"
        echo "----------------------------------------"
        echo "$EXAMPLE_CONFIG"
        echo "----------------------------------------"
        read -r -p "Where should we save this example config? (path, or press Enter to skip): " SAVE_PATH
        if [ -n "$SAVE_PATH" ]; then
            echo "$EXAMPLE_CONFIG" > "$SAVE_PATH"
            echo "Saved to $SAVE_PATH. Edit the file, then run: $0 $SAVE_PATH"
        else
            echo "Skipped. Save the example above to a file, then run: $0 <path_to_config>"
        fi
        exit 0
    fi
fi

# Resolve config path
CONFIG_FILE="$(cd "$(dirname "$CONFIG_FILE")" && pwd)/$(basename "$CONFIG_FILE")"
if [ ! -f "$CONFIG_FILE" ]; then
    echo "Error: config file not found: $CONFIG_FILE"
    exit 1
fi

# Parse config file (key=value, one per line; # comments and empty lines ignored)
parse_config() {
    local key val
    while IFS= read -r line || [ -n "$line" ]; do
        line="${line%%#*}"
        line="${line%"${line##*[![:space:]]}"}"
        line="${line#${line%%[![:space:]]*}}"
        [ -z "$line" ] && continue
        key="${line%%=*}"; key="${key%"${key##*[![:space:]]}"}"
        val="${line#*=}"; val="${val#"${val%%[![:space:]]*}}"}
        case "$key" in
            project_name) PROJECT_NAME="$val" ;;
            scheduler_base_dir) SCHEDULER_BASE_DIR="$val" ;;
            use_docker) USE_DOCKER="$val" ;;
            container_image) CONTAINER_IMAGE="$val" ;;
            has_coverage) HAS_COVERAGE="$val" ;;
            add_to_main_config) ADD_TO_MAIN_CONFIG="$val" ;;
            main_config_path) MAIN_CONFIG_PATH="$val" ;;
            github_app_id) GITHUB_APP_ID="$val" ;;
            github_app_key_file) GITHUB_APP_KEY_FILE="$val" ;;
            github_installation_id) GITHUB_INSTALLATION_ID="$val" ;;
            github_repo) GITHUB_REPO="$val" ;;
            github_workflow_name) GITHUB_WORKFLOW_NAME="$val" ;;
            github_artifact_names) GITHUB_ARTIFACT_NAMES="$val" ;;
        esac
    done < "$CONFIG_FILE"
}

PROJECT_NAME=""
SCHEDULER_BASE_DIR=""
USE_DOCKER="n"
CONTAINER_IMAGE="fuzzing-image:latest"
HAS_COVERAGE="n"
ADD_TO_MAIN_CONFIG="n"
MAIN_CONFIG_PATH=""
GITHUB_APP_ID=""
GITHUB_APP_KEY_FILE=""
GITHUB_INSTALLATION_ID=""
GITHUB_REPO=""
GITHUB_WORKFLOW_NAME=""
GITHUB_ARTIFACT_NAMES=""

parse_config

if [ -z "$PROJECT_NAME" ]; then
    echo "Error: project_name is required in config file."
    exit 1
fi
if [ -z "$SCHEDULER_BASE_DIR" ]; then
    echo "Error: scheduler_base_dir is required in config file."
    exit 1
fi

# Warn if GitHub App settings are missing (update script will fail at run time without them)
if [ -z "${GITHUB_APP_ID:-}" ] || [ -z "${GITHUB_APP_KEY_FILE:-}" ] || [ -z "${GITHUB_REPO:-}" ]; then
    echo "Warning: github_app_id, github_app_key_file and github_repo are not all set in your config."
    echo "         The update script will be generated but will fail until you set them and re-run install."
fi

# Normalize project name (alphanumeric + underscore + hyphen)
SAFE_NAME="$(echo "$PROJECT_NAME" | tr -c '[:alnum:]_-' '_' | sed 's/__*/_/g;s/^_\|_$//g')"
if [ -z "$SAFE_NAME" ]; then
    echo "Error: invalid project name (use letters, digits, hyphens, underscores)."
    exit 1
fi

SCHEDULER_DIR="$SCHEDULER_BASE_DIR/scheduler_${SAFE_NAME}"
if [ -d "$SCHEDULER_DIR" ]; then
    echo "Error: directory already exists: $SCHEDULER_DIR"
    exit 1
fi

mkdir -p "$SCHEDULER_BASE_DIR"
mkdir -p "$SCHEDULER_DIR"

if [[ "$USE_DOCKER" =~ ^[oOyY] ]]; then
    CONTAINER_IMAGE_JSON="\"${CONTAINER_IMAGE}\""
    CONTAINER_OPTIONS_JSON='["--privileged"]'
else
    CONTAINER_IMAGE_JSON="null"
    CONTAINER_OPTIONS_JSON="null"
fi

if [[ "$HAS_COVERAGE" =~ ^[oOyY] ]]; then
    HAS_COVERAGE_JSON="true"
else
    HAS_COVERAGE_JSON="false"
fi

echo ""
echo "Creating scheduler for project: $SAFE_NAME"
echo "  Scheduler directory: $SCHEDULER_DIR"

# Absolute paths for the scheduler
TARGETS_DIR="$SCHEDULER_DIR/fuzzers"
CORPUS_DIR="$SCHEDULER_DIR/corpus"
CRASHES_DIR="$SCHEDULER_DIR/crashes"
COVERAGE_DIR="$SCHEDULER_DIR/coverage"
mkdir -p "$TARGETS_DIR" "$CORPUS_DIR" "$CRASHES_DIR" "$COVERAGE_DIR"

# Scheduler config (JSON)
CONFIG_JSON="$SCHEDULER_DIR/config.json"
cat > "$CONFIG_JSON" << EOF
{
  "scheduler_name": "scheduler_${SAFE_NAME}",
  "targets_directory": ["${TARGETS_DIR}"],
  "build_script": "${SCHEDULER_DIR}/update_fuzzers_from_github.sh",
  "backend_url": "http://127.0.0.1:3000",
  "corpus_directory": "${CORPUS_DIR}",
  "crashes_directory": "${CRASHES_DIR}",
  "coverage_directory": "${COVERAGE_DIR}",
  "has_coverage": ${HAS_COVERAGE_JSON},
  "container_image": ${CONTAINER_IMAGE_JSON},
  "container_options": ${CONTAINER_OPTIONS_JSON},
  "state_file": "${SCHEDULER_DIR}/state.json"
}
EOF

echo "  - $CONFIG_JSON"

# Fuzzer update script via GitHub App (artifacts), using gh CLI
# Values from install config are injected directly (no .env file).
UPDATE_SCRIPT="$SCHEDULER_DIR/update_fuzzers_from_github.sh"
{
cat << INJECTEOF
#!/usr/bin/env bash
# Updates fuzzer binaries by downloading artifacts from the latest run
# of a GitHub Actions workflow, via GitHub App (using gh CLI).
# Run daily (cron or systemd timer).
# GitHub settings are set by install.sh (no .env).

set -e

SCRIPT_DIR="\$(cd "\$(dirname "\${BASH_SOURCE[0]}")" && pwd)"
TARGETS_DIR="${TARGETS_DIR}"

# GitHub App config (injected by install.sh)
GITHUB_APP_ID="${GITHUB_APP_ID}"
GITHUB_APP_KEY_FILE="${GITHUB_APP_KEY_FILE}"
GITHUB_INSTALLATION_ID="${GITHUB_INSTALLATION_ID}"
GITHUB_REPO="${GITHUB_REPO}"
GITHUB_WORKFLOW_NAME="${GITHUB_WORKFLOW_NAME}"
GITHUB_ARTIFACT_NAMES="${GITHUB_ARTIFACT_NAMES}"

INJECTEOF
cat << 'UPDATEEOF'

for cmd in gh jq openssl; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "Error: $cmd is required for update_fuzzers_from_github.sh"
        exit 1
    fi
done

if [ -z "${GITHUB_APP_ID:-}" ] || [ -z "${GITHUB_APP_KEY_FILE:-}" ] || [ -z "${GITHUB_REPO:-}" ]; then
    echo "Missing GitHub config. Re-run install.sh with github_app_id, github_app_key_file and github_repo set in your config."
    exit 1
fi

if [ ! -f "${GITHUB_APP_KEY_FILE}" ]; then
    echo "GitHub App key file not found: $GITHUB_APP_KEY_FILE"
    exit 1
fi

mkdir -p "$TARGETS_DIR"

# Generate JWT for the app (valid 10 min)
get_app_jwt() {
    local now
    now=$(date +%s)
    local exp=$((now + 600))
    local header='{"alg":"RS256","typ":"JWT"}'
    local payload="{\"iat\":$now,\"exp\":$exp,\"iss\":\"$GITHUB_APP_ID\"}"
    local b64_header b64_payload
    b64_header=$(echo -n "$header" | base64 | tr -d '\n' | tr '+/' '-_' | tr -d '=')
    b64_payload=$(echo -n "$payload" | base64 | tr -d '\n' | tr '+/' '-_' | tr -d '=')
    local sig_input="${b64_header}.${b64_payload}"
    local sig
    sig=$(echo -n "$sig_input" | openssl dgst -sha256 -sign "$GITHUB_APP_KEY_FILE" | base64 | tr -d '\n' | tr '+/' '-_' | tr -d '=')
    echo "${sig_input}.${sig}"
}

# Get installation token using gh api
get_installation_token() {
    local jwt="$1"
    local install_id="${GITHUB_INSTALLATION_ID:-}"
    if [ -z "$install_id" ]; then
        install_id=$(GH_TOKEN="$jwt" gh api "repos/${GITHUB_REPO}/installation" -q '.id // empty')
        if [ -z "$install_id" ]; then
            echo "Could not get installation for $GITHUB_REPO" >&2
            exit 1
        fi
    fi
    GH_TOKEN="$jwt" gh api -X POST "app/installations/${install_id}/access_tokens" -q '.token // empty'
}

JWT=$(get_app_jwt)
INSTALL_TOKEN=$(get_installation_token "$JWT")
if [ -z "$INSTALL_TOKEN" ]; then
    echo "Failed to get GitHub installation token"
    exit 1
fi

# Latest completed run (optionally filtered by workflow name) using gh
GH_ARGS=(-R "$GITHUB_REPO" --limit 1 --status completed --json databaseId -q '.[0].databaseId')
[ -n "${GITHUB_WORKFLOW_NAME:-}" ] && GH_ARGS+=(--workflow "$GITHUB_WORKFLOW_NAME")

RUN_ID=$(export GH_TOKEN="$INSTALL_TOKEN"; gh run list "${GH_ARGS[@]}")
if [ -z "$RUN_ID" ] || [ "$RUN_ID" = "null" ]; then
    echo "No run found for $GITHUB_REPO"
    exit 0
fi

# Download artifacts: only those in GITHUB_ARTIFACT_NAMES (comma-separated), or all if empty
export GH_TOKEN="$INSTALL_TOKEN"
DOWNLOAD_ARGS=(-R "$GITHUB_REPO" -D "$TARGETS_DIR")
if [ -n "${GITHUB_ARTIFACT_NAMES:-}" ]; then
    IFS=',' read -ra NAMES <<< "$GITHUB_ARTIFACT_NAMES"
    for n in "${NAMES[@]}"; do
        n=$(echo "$n" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
        [ -n "$n" ] && DOWNLOAD_ARGS+=(-n "$n")
    done
fi
gh run download "$RUN_ID" "${DOWNLOAD_ARGS[@]}"

# Flatten: gh extracts each artifact into a subdir; move contents to TARGETS_DIR
for d in "$TARGETS_DIR"/*/; do
    [ -d "$d" ] || continue
    mv "$d"* "$TARGETS_DIR/" 2>/dev/null || true
    rmdir "$d" 2>/dev/null || true
done
chmod +x "$TARGETS_DIR"/* 2>/dev/null || true

echo "Fuzzers updated in $TARGETS_DIR (run $RUN_ID)"
UPDATEEOF
} > "$UPDATE_SCRIPT"

chmod +x "$UPDATE_SCRIPT"
echo "  - $UPDATE_SCRIPT"

# Optionally add to main config (for FuzzTUI)
MAIN_CONFIG="${MAIN_CONFIG_PATH:-$INSTALL_ROOT/config.json}"
ADDED_TO_CONFIG=false
if [[ "$ADD_TO_MAIN_CONFIG" =~ ^[oOyY] ]] && [ -f "$MAIN_CONFIG" ]; then
    SCHEDULER_CONFIG_ABS="$CONFIG_JSON"
    if command -v jq >/dev/null 2>&1; then
        if jq --arg name "$SAFE_NAME" --arg config "$SCHEDULER_CONFIG_ABS" \
            '.schedulers += [{ "name": $name, "config": $config }]' \
            "$MAIN_CONFIG" > "${MAIN_CONFIG}.tmp" && mv "${MAIN_CONFIG}.tmp" "$MAIN_CONFIG"; then
            echo "  - Scheduler added to $MAIN_CONFIG"
            ADDED_TO_CONFIG=true
        fi
    else
        echo "  - jq not found: add manually to $MAIN_CONFIG :"
        echo "    { \"name\": \"$SAFE_NAME\", \"config\": \"$SCHEDULER_CONFIG_ABS\" }"
    fi
fi

echo ""
echo "Scheduler created: $SCHEDULER_DIR"
echo ""
echo "Next steps:"
echo "  1. Ensure GitHub App settings (github_app_id, github_app_key_file, github_repo) are set in your install config so the update script can run."
echo "  2. Install gh CLI (https://cli.github.com/) if not already installed; the update script uses gh."
echo "  3. Schedule daily execution of update_fuzzers_from_github.sh, e.g.:"
echo "     crontab -e  →  0 2 * * * $UPDATE_SCRIPT"
if [ "$ADDED_TO_CONFIG" = false ]; then
    echo "  4. To run with FuzzTUI, add this scheduler to $MAIN_CONFIG or the launcher config:"
    echo "     { \"name\": \"$SAFE_NAME\", \"config\": \"$CONFIG_JSON\" }"
fi
echo ""
