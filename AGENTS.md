# genbit: Agent 作業ガイド

## 最初に読むもの

1. この `AGENTS.md` で現行の構成、入出力、検証方法を把握する。
2. 既知の課題は GitHub Issues で管理する。課題は起票時点の記録なので、着手時には必ず関連する最新コードとテストで再確認する。
3. 利用者向けの概要とインストールは `README.md`、設定・記事・テンプレート・出力の仕様は `docs/` で確認する。

## Project Overview

- genbit は Rust 製の静的サイトジェネレーター CLI。Markdown 記事と Tera テンプレートから HTML を生成する。
- 主な利用者は、CLI でサイトを新規作成し、自分のサイトの `config.toml`、記事、テンプレート、CSS、静的ファイルを編集・公開する人。サイト作成・ビルド・ローカルプレビューに Node.js は不要。
- `new` がサイトの初期ファイルをコピーし、`build` が `dist/` を生成し、`dev` が再ビルド付きローカル配信を行う。生成サイトはこのリポジトリとは別ディレクトリで運用する。
- 現状は基本的な生成・プレビュー機能がある初期段階。`README.md` は画像の最適化とシンタックスハイライトを未実装と明記している。初期CSSはOSの暗色設定に追従するが、手動切り替えはない。公開サービスや管理画面はこのリポジトリにない。

## Tech Stack

- Rust 2024 edition、`rust-version = 1.98.1`。ツールチェーンは `rust-toolchain.toml` で固定し、GitHub Actions はこれを使う。`Dockerfile` のイメージと `Cargo.toml` の `rust-version` は同じ版にそろえ、CI が一致を検査する。依存関係は `Cargo.toml`、解決済み版は `Cargo.lock`。
- CLI: clap 4。エラー: anyhow。設定・データ: serde 1、toml 1。日時・タイムゾーン: jiff 0.2（タイムゾーンのデータを同梱）。
- 生成: pulldown-cmark 0.13、Tera 2、minify-html 0.18。開発サーバー: axum 0.8、Tokio 1、tower-http 0.7、notify 8、tokio-stream 0.1。出力の一時領域: tempfile 3。画像URLのハッシュ: sha2 0.11。
- DB、マイグレーション、フロントエンドのビルドシステムはない。ブラウザー用コードは生成 HTML と `dev` 専用の小さなリロードスクリプト。
- 整形・静的解析・テスト: rustfmt、Clippy、Cargo test。`Cargo.toml` は Rust 警告と Clippy の `all`/`pedantic` 等を deny にしている。

## Repository Structure

| 場所 | 役割と編集上の注意 |
| --- | --- |
| `src/main.rs` | clap の `new` / `build` / `dev` エントリーポイント。`build` と `dev` はカレントディレクトリをサイトルートにする。 |
| `src/scaffold.rs` と `scaffold/` | `new` のサイト作成処理と同梱する初期テンプレート・CSS・記事・favicon。`include_str!` でビルド時に同梱するため、初期サイトを変えるときは生成サイトの契約と統合テストも確認する。既存の利用者サイトは自動更新されない。 |
| `src/content.rs`、`src/markdown.rs` | TOML フロントマターの検証、Markdown→HTML。記事の日付形式の変更時は生成結果を確認する。 |
| `src/image_size.rs` | 記事本文で画像として参照する `static/` のファイルの内容ハッシュと、PNG・JPEG・GIF・WebPの寸法を取得し、EXIFの表示方向を反映する。対象形式の画像で寸法を取得できない場合はエラーにする。取得結果（画像でないファイルを含む）はビルド内でキャッシュする。 |
| `src/content_hash.rs` | 画像URLに付けるSHA-256のハッシュと、実際に書き込んだバイト列からハッシュを計算する書き込み。 |
| `src/route.rs` | 記事のURLと出力先の対応、サイト内の相対パスを `/` 区切りにする変換（名前の `\` は拒否）、開発サーバーの拡張子なしURL判定、配信URLの衝突・予約領域検査。URL形式の変更時は生成結果と配信を確認する。 |
| `src/config.rs` | `config.toml` の読み込み後の検証とサイトURL・OGP画像URLの正規化。公開URLの組み立てを担う。 |
| `src/render.rs` | テンプレート・CSSの読み込み、公開ビュー、Tera描画、HTML圧縮。記事の内部型を直接テンプレートへ渡さない。 |
| `src/tags.rs` | タグの書式・重複・予約名の検証、記事のタグ分けと `untagged` の規則。タグ一覧・タグページ・sitemap・テンプレートの `tags` はここを通す。 |
| `src/metadata.rs` | JSON-LD、sitemap、RSSフィード（`feed.xml`）、robotsの生成。 |
| `src/input.rs` | サイト入力のパス・ファイル種別を検査し、テキストの読み込みと静的ファイルのコピーを行う。 |
| `src/build.rs` | 設定・記事・静的素材を読み、描画とメタデータ生成を組み合わせて成果物を公開する。 |
| `src/output.rs` | 出力計画で衝突を検査し、生成ファイルの書き込みと静的ファイルのコピーをステージングしてから `dist/` を入れ替える。URLにハッシュを付けた画像は、コピーで書き込んだバイト列のハッシュを描画時の値と照合する。データ保護に関わるため、既存 `dist/` の扱いを変える前にテストを読む。 |
| `src/dev.rs` | ポート確保、監視登録、初回ビルド、単一の再ビルド処理、静的配信、SSE リロード。`build` と同じ生成経路を使う。 |
| `tests/cli.rs` | 一時ディレクトリで実行バイナリを起動する E2E テスト。入出力形式や保護動作を変更するときの主な確認先。 |
| `tests/license_lists.rs` | `deny.toml` の `allow` と `about.toml` の `accepted` が同じライセンスの一覧であることを検査する。 |
| `README.md`、`docs/` | 利用者向けの概要と仕様。README は概要・インストール・既知の制限、`docs/` は設定・コンテンツ・テンプレート・出力・開発の詳細。`docs/assets/` の画像は README 用。 |
| `.github/workflows/ci.yml`、`.github/workflows/deny.yml`、`.github/workflows/release.yml`、`scripts/`、`deny.toml`、`about.toml`、`about.hbs`、`Dockerfile`、`compose.yaml` | CI、依存の検査、バイナリのリリース、配布物の smoke test と依存のライセンス一覧の生成、README のスクリーンショットの撮影（`scripts/readme-screenshots.sh` と素材の `scripts/readme-screenshots/`）、開発用コンテナ設定。`target/` は生成物で Git 管理外。 |

## Architecture and Data Flow

- 単一のバイナリ。API サーバーや DB 層はない。`dev` の HTTP エンドポイントは静的ファイル配信と `GET /__genbit/reload`（SSE）のみ。
- `new <name>` は名前を ASCII 英数字・`-`・`_`（先頭は英数字）に検証し、新しいディレクトリへ `config.toml`、`/dist/` を除外する `.gitignore`、`scaffold/` の素材を書き込む。既存のパスは上書きしない。
- `build` はサイト直下の `config.toml`（必須の `title`、`description`、`site_url`、`og_image`、`timezone`）を読み、`templates/**/*.html` を Tera に登録し、`content/**/*.md` を `Article` に変換する。`content/index.md` は使えず、トップページは記事とは別に `root.html` と設定から生成する。
- 記事の TOML フロントマターには引用符なしのローカル日時 `created_at = YYYY-MM-DD HH:MM` が必須。`updated_at` も同形式で必須。日付のみ・秒付きは受け付けず、日付と時刻の区切りは `T` も可。日時は `timezone`（IANA 名のみ）の地域の時刻として jiff の `Zoned` に変換し、更新日時は作成日時以降にする。`description` は必須で空にできない。`title` がない場合はファイル名、`template` がない場合は `page.html`。未知のフィールドはエラー。
- 記事の相対パスはそのまま保ち、`.md` を `.html` に置き換えて出力する。例: `content/entries/a.md` → `dist/entries/a.html`、記事URLは `/entries/a`。パス要素は ASCII 英数字・`-`・`_` に制限される。記事と `dev` は共通の `Route` 規則を使う。
- 記事本文の生 HTML は禁止する。`markdown.rs` はパーサー直後の位置情報付きイベントで `Event::Html`・`Event::InlineHtml`（コメントを含む）を検査し、最初の箇所の本文内バイト位置をエラーで返す。見出し・画像などの加工は検査を通ったイベント列だけに行い、加工で genbit 自身が生成する `Event::Html` は検査しない。`content.rs` がその位置を記事ファイル全体の1始まりの行番号（BOM・フロントマター・CRLF を含む）に変換する。コードスパン・コードブロック・エスケープ・文字参照・オートリンクは許可する。
- Markdown の相対 `.md` リンクはイベント処理で拡張子なしのURLに変換する。クエリとアンカーは保持し、外部 URL・ルート相対 URL・画像の参照先は変えない。
- 全記事を作成日時降順、同時刻なら URL 順で並べる。この順序の先頭20件から `dist/feed.xml`（RSS 2.0、`pubDate` は `created_at` と `timezone`）を生成し、sitemap には含めない。テンプレートに渡す `created_at`・`updated_at` は `datetime`（時差付き RFC 3339）・`date`・`time` を持つオブジェクトで、JSON-LD と sitemap も同じ時差付き日時を使う。`render.rs` の専用ビューから共通の `site` と `css`、トップページ専用の `entries`、記事ページ専用の `article` と `content` を渡す。Markdown 画像はイベント処理で `loading="lazy"` と `decoding="async"` を付ける。`markdown.rs` は構文検査と描画を分け、描画時のコールバック経由で `image_size.rs` が参照先の寸法を取得する。ローカルPNG・JPEG・GIF・WebPには寸法を `width`・`height` として必ず付け、JPEGのEXIF Orientation 5〜8では幅と高さを交換する。内容がこの4形式の画像、または拡張子がこの4形式を示すファイルで、寸法・EXIFを解析できない場合、Orientationが1〜8でない場合、PNG・WebPのOrientationが5〜8の場合（ブラウザーで表示方向が一致しない）はビルドエラーにする。外部URLと、拡張子も内容も4形式でないファイル（SVG・AVIFなど）は属性なしとする。imagesizeとkamadak-exifを用い、ピクセルのデコードや変換は行わない。`static/` のファイルに解決される画像は形式を問わず、`src` のフラグメントの前にクエリ `v=<内容のSHA-256の先頭16桁>` を追加する。参照先のクエリにキー `v`（パーセントデコード後に比較）があればビルドエラー。リンク検証は元の参照先で行い、コピー時に書き込んだバイト列のSHA-256全体が描画時と異なれば公開前に失敗する。初期画像CSSは `styles/common.css` に置く。`styles/common.css` は必須で、使用テンプレートと同名の CSS は任意。CSS はテンプレートごとに組み立て、HTML ごとに埋め込み圧縮する。
- 記事のリンクと画像の参照先は `markdown.rs` が出力するとおりに集め、`OutputPlan` が確定した配信URLの集合と `route.rs` で照合する。相対パスは記事URLを基準にブラウザーと同じく解決し、スキーム付きURL・同一ページ内リンクは対象外。
- URLのスキームは先頭の構文で判定し、`/files/a:b.png` のようなパス内のコロンは内部参照として扱う。HTTP(S)の外部リンク属性はスキームの大文字・小文字を区別しない。パスは一度だけデコードし、`%2e` などのドット要素も解決する。パス内の `%2F`・`%5C` とバックスラッシュはビルドエラーにし、通常の `/` で区切ることを求める。クエリとアンカー内のエンコードはこの制限の対象外。
- `build --dry-run` は通常の `build` と同じ経路で `OutputPlan` の作成とリンク検証まで行い、`dist/` の所有チェックだけして公開（ステージング・入れ替え）をしない。
- `static/` の通常ファイルはステージング領域へ直接コピーし、コピー時にも入力パスとファイル種別を検査する。出力パスの重複、大小文字だけ異なる衝突、ファイルとディレクトリの衝突に加え、記事・静的ファイルの配信URL衝突と `/__genbit` 配下の使用を拒否する。`dist/` は `.genbit-output` マーカーを持つ既存ディレクトリだけ入れ替える。入力ディレクトリ内のシンボリックリンクは拒否する。
- `dev` はポート確保と監視登録の後に初回ビルドし、既定の `127.0.0.1:3000` で `dist/` を配信する。記事の拡張子なしURLも対応する `.html` から配信する。配信の応答には `Cache-Control: no-store` を付け、条件付きリクエストのヘッダーは削除してから配信する。`config.toml` と `content/`・`templates/`・`styles/`・`static/` の変更通知は容量1で保持し、100 msの静穏期間または500 msの最大待機後に単一の再ビルドを行う。ビルド中の変更は次回に処理し、成功時だけSSEを送る。SIGINT・SIGTERM では実行中のビルドの完了を待って正常終了する。開発用スクリプトは `build` の出力には入らない。

## Development Commands

Rust 関連の開発・検証は `compose.yaml` の `cli` サービスで実行する。初回または `Dockerfile` 変更後はイメージをビルドする。

```sh
docker compose build cli
docker compose run --rm cli fmt --check
docker compose run --rm cli clippy --locked --all-targets --all-features -- -D warnings
docker compose run --rm cli test --locked --all-targets --all-features
```

CLI を試すときは、生成サイトのディレクトリで `genbit new <name>`、`genbit build`、`genbit dev [--host <IP>] [--port <番号>]` を使う。Docker から利用者サイトを動かす bind mount 例は `docs/development.md` を参照。公開は生成された `dist/` を静的ホストへ配置する手順のみが記載され、自動デプロイ設定はない。

## Coding Conventions and Testing

- Rust ファイル・関数は snake_case、型は PascalCase。モジュールは `main.rs` から内部で宣言し、内部共有は必要な範囲で `pub(crate)` を使う。
- 失敗し得る処理は `anyhow::Result`、`?`、`Context` / `with_context`、`ensure!` / `bail!` を使い、入力・出力パスをエラーに含める。外部入力は型へのデシリアライズと明示検証を行う。
- テンプレートは Tera、本文は Markdown の生成 HTML を `safe` で挿入する。本文の生 HTML は禁止しているため、`content` の HTML は genbit が生成したものだけ。テンプレートには HTML を書ける。`safe` の扱いを変える際は、既存サイトのテンプレートと信頼する記事入力の範囲を確認する。
- ユニットテストは各 `src/*.rs` の `#[cfg(test)]` 内、CLI の結合テストは `tests/cli.rs`。テスト名は挙動を説明する snake_case。一時サイトは `tempfile::TempDir` で作り、CLI の終了状態・エラー・生成内容・旧出力の保護を検証している。`tests/cli.rs` ではビルドの成否を `build_ok` / `build_err`（失敗時の stderr を返す）で確認し、記事は既定のフロントマターに差分を重ねる `article_source` で組み立てる。網羅率は CI で計測するが、閾値はない。
- `.github/workflows/ci.yml` は pull request と `main` への push で、次のジョブを並列に実行する。`lint` は版の一致・`scripts/*.sh` の shellcheck・rustfmt・Clippy、`test-linux` は Ubuntu での cargo-llvm-cov によるカバレッジ計測付きテスト（要約をジョブの Summary に出し、閾値はない）、`test-macos` は macOS のテスト、`smoke-test` は Ubuntu の release ビルドでの `scripts/smoke-test.sh`、`third-party-licenses` は `scripts/third-party-licenses.sh`。ジョブ名は main の ruleset の必須チェック名なので、変えるときは ruleset も合わせて更新する。PR ブランチへの push だけでは動かない。PR では新しい push で古い実行を取り消す。依存のビルド結果は `Swatinem/rust-cache` でキャッシュし、保存は `main` の実行だけが行う。独立した型チェックコマンドは定義されていない。
- `.github/workflows/deny.yml` は pull request、`main` への push、週 1 回の定期実行で、`deny.toml` に従って cargo-deny を実行する。ジョブは勧告を見る `deny-advisories` と、ライセンス・取得元・禁止クレートを見る `deny-policy` に分かれ、必須チェックは `deny-policy` だけ。脆弱性・保守終了・yank の勧告、許可リスト外のライセンス、crates.io 以外の取得元を拒否する。勧告を無視するときは `deny.toml` の `ignore` に理由を書く。
- `.github/workflows/release.yml` は Actions の Run workflow から `main` で手動実行する。`Cargo.toml` の版から `v<版>` のタグ名を決め、タグが未作成であることの確認（`prepare`）、fmt・Clippy・テスト（`test`）、ライセンス一覧の生成（`licenses`）を並列に行う。`build` は一覧を待ってから `x86_64-unknown-linux-musl` と `aarch64-apple-darwin` をそれぞれのランナーでビルドし、アーカイブにしてから展開したバイナリを `scripts/smoke-test.sh` で確かめる。すべて成功したら `release` ジョブが `SHA256SUMS`・Artifact Attestations 付きの下書き Release を作る。ジョブ間で受け渡す artifact は1日で消える。アーカイブには `scripts/third-party-licenses.sh` が cargo-about で生成する依存クレートのライセンス一覧 `THIRD_PARTY_LICENSES.md` を同梱する。cargo-about は `taiki-e/install-action` で版を固定して入れる（チェックサムはアクションが検証する）。公開は下書きを確認してから手動で行い、そのときに GitHub がタグを作る。`v*` タグはルールセットで削除・付け替えを禁じているため、ワークフローはタグを作らない。

## Environment and Configuration

- CLI 実行に必須の環境変数や外部サービスはコード上ない。`docs/development.md` の `SITE_DIR` は Docker 実行例で使うシェル変数で、genbit の設定ではない。
- 利用者サイトには `config.toml`、`content/`、`templates/`、`styles/common.css`、`static/` が必要。`new` がこれらを用意する。CLI の設定はサイト側から読み、リポジトリ側の `scaffold/` は初期値だけを提供する。
- `dev` はローカル HTTP サーバー。外部公開や認証付き配信の仕組みは確認できない。生成 HTML はルート相対 URL を使うため、サブパス配信には対応していない。

## Agent Guidelines

- 利用者への説明では、独立した論点・設定・原因を一つにまとめず、それぞれ分けて説明する。各論点が何を決め、何に影響するかを明確にする。
- 作業前に対象モジュール、呼び出し元、関連する `tests/cli.rs` と `scaffold/` を読む。変更後はドキュメント上の説明と生成結果を照合する。
- 入力サイトの構造、テンプレート変数、フロントマター、URL、`dist/` の保護は利用者向けの仕様として扱う。変更時は `docs/`（必要なら README）と初期素材を更新し、エラーとデータ保護のテストを追加する。
- 必要な範囲だけ変更し、既存の `Result` とパス付きエラー、厳格な lint を維持する。新規依存は用途と既存依存で代替できない理由を確認する。
- `scaffold/` は公開してよい初期値だけを置く。利用者サイトの実データ、秘密情報、実際の `.env` をこのリポジトリへ追加しない。`Cargo.lock` を管理し、依存更新は意図して行う。依存を追加して許可リスト外のライセンスが入る場合は、`deny.toml` を変える前に配布への影響を確認する。
- Rust コード・依存・ビルド設定を変更したら、関連テストを整備して上記の fmt、Clippy、test を `--locked` で実行する。文書だけの変更なら内容と差分を確認する。実行できなかった検証はそのまま報告する。
- 課題の説明を実装の事実と混同しない。Issue にある仮説・改善案は着手時に再検証し、解決したら経緯を残して Issue を閉じる。
