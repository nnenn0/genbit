#!/bin/sh
# 配布アーカイブに同梱する依存クレートのライセンス一覧を cargo-about で生成する。
# 使い方: scripts/third-party-licenses.sh <出力ファイル>
set -eu

version=0.9.2
case "$(uname -s)-$(uname -m)" in
    Linux-x86_64)
        target=x86_64-unknown-linux-musl
        sha256=9099a59e820c38a68b9d65f300662a567d56562f9a10f6aa4c7e86c17c2566af
        ;;
    Linux-aarch64)
        target=aarch64-unknown-linux-musl
        sha256=af5169282fb6f84e13471493f405437e43ac517744c9ae12fbe2cdf0a6f0e5a8
        ;;
    *)
        echo "unsupported platform: $(uname -s)-$(uname -m)" >&2
        exit 1
        ;;
esac

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
archive=cargo-about-$version-$target.tar.gz
curl -sSfL -o "$work/$archive" \
    "https://github.com/EmbarkStudios/cargo-about/releases/download/$version/$archive"
echo "$sha256  $work/$archive" | sha256sum -c -
tar -xzf "$work/$archive" -C "$work" --strip-components=1
"$work/cargo-about" generate --locked --fail --output-file "$1" about.hbs
