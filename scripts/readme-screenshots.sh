#!/bin/sh
# README のスクリーンショット（docs/assets/screenshot-{light,dark}.png）を撮り直す。
# scripts/readme-screenshots/ の設定と記事で new したサイトを Docker Compose でビルドし、
# frame.html で記事一覧と記事ページを並べてホストの Chrome で撮影する。
# 初期テンプレートや CSS の見た目を変えたら実行する。
# 使い方: scripts/readme-screenshots.sh
# Chrome の場所は環境変数 CHROME で変えられる。
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
materials=$root/scripts/readme-screenshots
chrome=${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# macOS の Docker Desktop では bind mount 越しの削除がコンテナに届かないことがあるため、
# 初期記事の差し替えもビルドと同じコンテナ内で行う。
docker compose --project-directory "$root" run --rm -T \
    --volume "$work:/work" --workdir /work --entrypoint sh cli -euc '
        genbit() { cargo run --quiet --locked --manifest-path /workspace/Cargo.toml -- "$@"; }
        materials=/workspace/scripts/readme-screenshots
        genbit new site
        rm -r site/content
        cp -R "$materials/content" site/content
        cp "$materials/config.toml" site/config.toml
        cd site
        genbit build
    '
cp "$materials/frame.html" "$work/frame.html"

# 本文の iframe は OS の配色設定に従うため、ライト・ダークの両方で配色を明示する。
for scheme in light dark; do
    case $scheme in
        light) preference=1 ;;
        dark) preference=0 ;;
    esac
    "$chrome" --headless --disable-gpu --hide-scrollbars \
        --default-background-color=00000000 --force-device-scale-factor=2 \
        --window-size=1198,652 --blink-settings=preferredColorScheme="$preference" \
        --screenshot="$root/docs/assets/screenshot-$scheme.png" \
        "file://$work/frame.html#$scheme" 2>/dev/null
done
