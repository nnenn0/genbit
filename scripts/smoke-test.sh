#!/bin/sh
# 配布用バイナリで new と build を実行し、主要な成果物ができることを確認する。
# 使い方: scripts/smoke-test.sh <genbit のパス> [期待する版]
set -eu

bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
if [ "$#" -ge 2 ]; then
    actual=$("$bin" --version)
    if [ "$actual" != "genbit $2" ]; then
        echo "unexpected version: $actual (expected genbit $2)" >&2
        exit 1
    fi
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"
"$bin" new site
cd site
"$bin" build
for file in dist/index.html dist/entries/hello-world/index.html dist/feed.xml; do
    if [ ! -s "$file" ]; then
        echo "missing or empty $file" >&2
        exit 1
    fi
done
