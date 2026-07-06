#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="$ROOT/tests/fixtures/embedding"

mkdir -p "$DEST"

if [[ ! -f "$DEST/model.onnx" ]]; then
  curl -fsSL -o "$DEST/model.onnx" \
    "https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/onnx/model.onnx"
fi

if [[ ! -f "$DEST/tokenizer.json" ]]; then
  curl -fsSL -o "$DEST/tokenizer.json" \
    "https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/tokenizer.json"
fi
