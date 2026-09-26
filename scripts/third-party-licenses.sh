#!/bin/sh
# 配布アーカイブに同梱する依存クレートのライセンス一覧を cargo-about で生成する。
# 対象と書式は about.toml と about.hbs で決まる。CI とリリースでは
# taiki-e/install-action で cargo-about を入れてから実行する。
# 使い方: scripts/third-party-licenses.sh <出力ファイル>
set -eu

cargo about generate --locked --fail --output-file "$1" about.hbs
