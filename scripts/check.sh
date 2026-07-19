#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

cd "$PROJECT_ROOT"
bash -n scripts/*.sh
PYTHONDONTWRITEBYTECODE=1 python3 scripts/prepare-assets.py --check
cargo fmt --all -- --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked
