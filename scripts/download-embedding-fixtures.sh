#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="$ROOT/tests/fixtures/embedding"
REVISION="5c38ec7c405ec4b44b94cc5a9bb96e735b38267a"
BASE_URL="https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/${REVISION}"

declare -A EXPECTED_SHA256=(
  ["model.onnx"]="828e1496d7fabb79cfa4dcd84fa38625c0d3d21da474a00f08db0f559940cf35"
  ["tokenizer.json"]="d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66"
)

declare -A REMOTE_PATHS=(
  ["model.onnx"]="onnx/model.onnx"
  ["tokenizer.json"]="tokenizer.json"
)

verify_sha256() {
  local file="$1"
  local expected="$2"
  local actual
  actual="$(sha256sum "$file" | awk '{print $1}')"
  if [[ "$actual" != "$expected" ]]; then
    echo "SHA-256 mismatch for $(basename "$file"): expected $expected, got $actual" >&2
    return 1
  fi
}

download_fixture() {
  local filename="$1"
  local destination="$DEST/$filename"
  local expected="${EXPECTED_SHA256[$filename]}"

  if [[ -f "$destination" ]] && verify_sha256 "$destination" "$expected"; then
    return 0
  fi

  rm -f "$destination"
  curl --http1.1 -fsSL -o "$destination" "${BASE_URL}/${REMOTE_PATHS[$filename]}"
  verify_sha256 "$destination" "$expected"
}

mkdir -p "$DEST"
download_fixture "model.onnx"
download_fixture "tokenizer.json"
