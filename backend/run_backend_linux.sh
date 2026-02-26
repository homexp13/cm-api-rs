#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "[INFO] Starting cm-api-rs backend..."
echo "[INFO] Directory: $PWD"

if ! command -v cargo >/dev/null 2>&1; then
  echo "[ERROR] cargo not found in PATH."
  exit 1
fi

cargo run