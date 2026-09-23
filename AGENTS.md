# genbit: Agent 作業ガイド

## 最初に読むもの

1. この `AGENTS.md` で現行の構成、入出力、検証方法を把握する。
2. 現在認識している課題は `PROJECT_ISSUES.md` を参照する。課題は調査時点の記録なので、着手時には必ず関連する最新コードとテストで再確認する。
3. 利用者向けの操作とサイト形式は `README.md` でも確認する。

## Project Overview

- genbit は Rust 製の静的サイトジェネレーター CLI（`Cargo.toml` の版は `0.1.0`）。Markdown 記事と Tera テンプレートから HTML を生成する。
- 主な利用者は、CLI でサイトを新規作成し、自分のサイトの `config.toml`、記事、テンプレート、CSS、静的ファイルを編集・公開する人。サイト作成・ビルド・ローカルプレビューに Node.js は不要。
- `new` がサイトの初期ファイルをコピーし、`build` が `dist/` を生成し、`dev` が再ビルド付きローカル配信を行う。生成サイトはこのリポジトリとは別ディレクトリで運用する。
- 現状は基本的な生成・プレビュー機能がある初期段階。`README.md` は画像圧縮、コードのシンタックスハイライト、ダークモードを未実装と明記している。公開サービスや管理画面はこのリポジトリにない。

## Tech Stack

- Rust 2024 edition、`rust-version = 1.98.1`。`Dockerfile` と GitHub Actions も Rust 1.98.1 を指定。依存関係は `Cargo.toml`、解決済み版は `Cargo.lock`。
- CLI: clap 4。エラー: anyhow。設定・データ: serde 1、toml 1、gray_matter 0.3。
- 生成: pulldown-cmark 0.13、Tera 2、minify-html 0.18。開発サーバー: axum 0.8、Tokio 1、tower-http 0.7、notify 8、tokio-stream 0.1。出力の一時領域: tempfile 3。
- DB、マイグレーション、フロントエンドのビルドシステムはない。ブラウザー用コードは生成 HTML と `dev` 専用の小さなリロードスクリプト。
- 整形・静的解析・テスト: rustfmt、Clippy、Cargo test。`Cargo.toml` は Rust 警告と Clippy の `all`/`pedantic` 等を deny にしている。

## Repository Structure

| 場所 | 役割と編集上の注意 |
| --- | --- |
| `src/main.rs` | clap の `new` / `build` / `dev` エントリーポイント。`build` と `dev` はカレントディレクトリをサイトルートにする。 |
| `src/scaffold.rs` と `scaffold/` | `new` のサイト作成処理と同梱する初期テンプレート・CSS・記事・favicon。`include_str!` でビルド時に同梱するため、初期サイトを変えるときは生成サイトの契約と統合テストも確認する。既存の利用者サイトは自動更新されない。 |
| `src/content.rs`、`src/markdown.rs` | TOML フロントマターの検証、記事の URL/出力先決定、Markdown→HTML。記事 URL と日付形式の変更時は生成結果を確認する。 |
| `src/build.rs` | 設定・素材の読込、記事一覧の作成、Tera 描画、CSS インライン化、HTML 圧縮、成果物の収集。利用者サイトのディレクトリを読む。 |
| `src/output.rs` | 出力パス衝突の検査、ステージングと `dist/` の入れ替え。データ保護に関わるため、既存 `dist/` の扱いを変える前に統合テストを読む。 |
| `src/dev.rs` | 初回ビルド、ファイル変更監視、再ビルド、静的配信、SSE リロード。`build` と同じ生成経路を使う。 |
| `tests/cli.rs` | 一時ディレクトリで実行バイナリを起動する E2E テスト。入出力形式や保護動作を変更するときの主な確認先。 |
| `README.md`、`.github/workflows/ci.yml`、`Dockerfile`、`compose.yaml` | 利用者向け契約、CI、開発用コンテナ設定。`target/` は生成物で Git 管理外。 |

## Architecture and Data Flow

- 単一のバイナリ。API サーバーや DB 層はない。`dev` の HTTP エンドポイントは静的ファイル配信と `GET /__genbit/reload`（SSE）のみ。
- `new <name>` は名前を ASCII 英数字・`-`・`_` に検証し、新しいディレクトリへ `config.toml` と `scaffold/` の素材を書き込む。既存のパスは上書きしない。
- `build` はサイト直下の `config.toml`（必須の `title`）を読み、`templates/**/*.html` を Tera に登録し、`content/**/*.md` を `Article` に変換する。`content/index.md` は使えず、トップページは記事とは別に `root.html` と設定から生成する。
- 記事の TOML フロントマターには引用符なしのローカル日付またはローカル日時 `created_at = YYYY-MM-DD HH:MM`（秒も任意で指定可）が必須。日付と時刻の区切りは `T` も可。時差変換はせず、日付のみは `00:00:00` として並べる。`updated_at` は任意のローカル日付で作成日以降。`title` がない場合はファイル名、`template` がない場合は `page.html`。未知のフィールドはエラー。
- 記事の相対パスはそのまま保ち、`.md` を `.html` に置き換えて出力する。例: `content/entries/a.md` → `dist/entries/a.html`、記事URLは `/entries/a`。パス要素は ASCII 英数字・`-`・`_` に制限される。
- Markdown の相対 `.md` リンクはイベント処理で拡張子なしのURLに変換する。クエリとアンカーは保持し、外部 URL・ルート相対 URL・画像の参照先は変えない。
- 全記事を作成日時降順、同時刻なら URL 順で並べる。テンプレートに渡す `created_at` は日付だけ。テンプレートには共通の `site` と `css`、トップページ専用の `entries`、記事ページ専用の `article` と `content` を渡す。Markdown 画像はイベント処理で `loading="lazy"` と `decoding="async"` を付ける。`styles/common.css` は必須で、使用テンプレートと同名の CSS は任意。CSS はテンプレートごとに組み立て、HTML ごとに埋め込み圧縮する。
- `static/` の通常ファイルは出力ルートへコピーする。出力パスの重複、大小文字だけ異なる衝突、ファイルとディレクトリの衝突を拒否する。`dist/` は `.genbit-output` マーカーを持つ既存ディレクトリだけ入れ替える。入力ディレクトリ内のシンボリックリンクは拒否する。
- `dev` は起動時にビルドし、既定の `127.0.0.1:3000` で `dist/` を配信する。記事の拡張子なしURLも対応する `.html` から配信する。`config.toml` と `content/`・`templates/`・`styles/`・`static/` の変更イベント後に再ビルドし、成功時だけ SSE を送る。開発用スクリプトは `build` の出力には入らない。

## Development Commands

Rust 関連の開発・検証は `compose.yaml` の `cli` サービスで実行する。初回または `Dockerfile` 変更後はイメージをビルドする。

```sh
docker compose build cli
docker compose run --rm cli fmt --check
docker compose run --rm cli clippy --locked --all-targets --all-features -- -D warnings
docker compose run --rm cli test --locked --all-targets --all-features
```

CLI を試すときは、生成サイトのディレクトリで `genbit new <name>`、`genbit build`、`genbit dev [--host <IP>] [--port <番号>]` を使う。Docker から利用者サイトを動かす bind mount 例は `README.md` を参照。公開は生成された `dist/` を静的ホストへ配置する手順のみが記載され、自動デプロイ設定はない。

## Coding Conventions and Testing

- Rust ファイル・関数は snake_case、型は PascalCase。モジュールは `main.rs` から内部で宣言し、内部共有は必要な範囲で `pub(crate)` を使う。
- 失敗し得る処理は `anyhow::Result`、`?`、`Context` / `with_context`、`ensure!` / `bail!` を使い、入力・出力パスをエラーに含める。外部入力は型へのデシリアライズと明示検証を行う。
- テンプレートは Tera、本文は Markdown の生成 HTML を `safe` で挿入する。`safe` の扱いを変える際は、既存サイトのテンプレートと信頼する記事入力の範囲を確認する。
- ユニットテストは各 `src/*.rs` の `#[cfg(test)]` 内、CLI の結合テストは `tests/cli.rs`。テスト名は挙動を説明する snake_case。一時サイトは `tempfile::TempDir` で作り、CLI の終了状態・エラー・生成内容・旧出力の保護を検証している。現時点でユニット 11 件、結合 12 件。網羅率の計測設定はない。
- `.github/workflows/ci.yml` は全ブランチの push と pull request で rustfmt、Clippy、テストを実行する。独立した型チェックコマンドは定義されていない。

## Environment and Configuration

- CLI 実行に必須の環境変数や外部サービスはコード上ない。`README.md` の `SITE_DIR` は Docker 実行例で使うシェル変数で、genbit の設定ではない。
- 利用者サイトには `config.toml`、`content/`、`templates/`、`styles/common.css`、`static/` が必要。`new` がこれらを用意する。CLI の設定はサイト側から読み、リポジトリ側の `scaffold/` は初期値だけを提供する。
- `dev` はローカル HTTP サーバー。外部公開や認証付き配信の仕組みは確認できない。生成 HTML はルート相対 URL を使うため、サブパス配信は課題ファイルを参照。

## Agent Guidelines

- 作業前に対象モジュール、呼び出し元、関連する `tests/cli.rs` と `scaffold/` を読む。変更後はドキュメント上の説明と生成結果を照合する。
- 入力サイトの構造、テンプレート変数、フロントマター、URL、`dist/` の保護は利用者向けの仕様として扱う。変更時は README と初期素材を更新し、エラーとデータ保護のテストを追加する。
- 必要な範囲だけ変更し、既存の `Result` とパス付きエラー、厳格な lint を維持する。新規依存は用途と既存依存で代替できない理由を確認する。
- `scaffold/` は公開してよい初期値だけを置く。利用者サイトの実データ、秘密情報、実際の `.env` をこのリポジトリへ追加しない。`Cargo.lock` を管理し、依存更新は意図して行う。
- Rust コード・依存・ビルド設定を変更したら、関連テストを整備して上記の fmt、Clippy、test を `--locked` で実行する。文書だけの変更なら内容と差分を確認する。実行できなかった検証はそのまま報告する。
- 課題の説明を実装の事実と混同しない。`PROJECT_ISSUES.md` にある仮説・改善案は着手時に再検証し、解決した項目は記録を更新する。
