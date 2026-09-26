"""deny.toml の allow と about.toml の accepted が同じライセンスの一覧か検査する。

cargo-deny と cargo-about は別々の設定ファイルで許可するライセンスを持つため、
片方だけを更新したずれを CI で止める。
"""

import sys
import tomllib

with open("deny.toml", "rb") as file:
    deny = set(tomllib.load(file)["licenses"]["allow"])
with open("about.toml", "rb") as file:
    about = set(tomllib.load(file)["accepted"])

if not deny or deny != about:
    print(f"only in deny.toml: {sorted(deny - about)}", file=sys.stderr)
    print(f"only in about.toml: {sorted(about - deny)}", file=sys.stderr)
    sys.exit(1)
