#!/usr/bin/env bash
set -euo pipefail

CRATE_LIB_NAME="extension"
OUTPUT="extension.wasm"
TARGET="wasm32-wasip1"

if ! command -v rustup &>/dev/null; then
    echo "rustup not found. Install from: https://rustup.rs/"
    exit 1
fi

if ! command -v cargo &>/dev/null; then
    echo "cargo not found. Re-install rustup."
    exit 1
fi

if [[ "${1:-}" == "--check" ]]; then
    echo "Toolchain OK (rustup + cargo found)"
    exit 0
fi

echo "Ensuring $TARGET target is installed..."
rustup target add "$TARGET"

echo "Building release binary..."
cargo build --release --target "$TARGET"

SRC="target/$TARGET/release/${CRATE_LIB_NAME}.wasm"

if [[ ! -f "$SRC" ]]; then
    echo "Expected binary not found at: $SRC"
    echo "Check [lib] name = \"$CRATE_LIB_NAME\" in Cargo.toml"
    exit 1
fi

cp "$SRC" "$OUTPUT"

SIZE_KB=$(( $(wc -c < "$OUTPUT") / 1024 ))
SHA=$(shasum -a 256 "$OUTPUT" | awk '{print $1}')

echo ""
echo "Built $OUTPUT (${SIZE_KB} KB)"
echo "SHA-256: $SHA"
echo ""
echo "Next steps:"
echo "  git add $OUTPUT"
echo "  git commit -m 'build: update extension.wasm'"
echo "  git push"
