# genbit: Agent 作業ガイド

## 最初に読むもの

1. この `AGENTS.md` で構成、守る設計、検証方法を把握する。
2. 既知の課題は GitHub Issues で管理する。課題は起票時点の記録なので、着手時には必ず関連する最新コードとテストで再確認する。
3. 利用者向けの概要とインストールは `README.md`、設定・記事・テンプレート・出力の仕様は `docs/` で確認する。利用者から見える挙動は `docs/` を正とし、この文書には書かない。

## Project Overview

- genbit は Rust 製の静的サイトジェネレーター CLI。Markdown 記事と bitview テンプレートから HTML を生成する。bitview は同じ作者の別リポジトリ（`https://github.com/nnenn0/bitview`）で、Git 依存として `rev` で固定する。単一のバイナリで、DB・API サーバー・フロントエンドのビルドシステムはない。
- `new` がサイトの初期ファイルを作り、`build` が `dist/` を生成し、`dev` が再ビルド付きのローカル配信を行う。生成サイトはこのリポジトリとは別ディレクトリで運用する。
- ツールチェーンは `rust-toolchain.toml` で固定する。`Dockerfile` のイメージと `Cargo.toml` の `rust-version` は同じ版にそろえ、CI が一致を検査する。依存は `Cargo.toml` と `Cargo.lock` を見る。

## Repository Structure

| 場所 | 役割と編集上の注意 |
| --- | --- |
| `src/main.rs` | clap のエントリーポイント。`build` と `dev` はカレントディレクトリをサイトルートにする。 |
| `src/scaffold.rs`、`scaffold/` | `new` が書き込む初期サイト。`include_str!` で同梱する。既存の利用者サイトは自動更新されない。 |
| `src/content.rs`、`src/markdown.rs` | `content/`・`drafts/` のページと素材の振り分け、フロントマターの検証、Markdown から `bitview::Html` への変換。 |
| `src/images.rs`、`src/content_hash.rs` | 本文の画像の寸法・表示方向と、画像URLに付ける内容のハッシュ。 |
| `src/route.rs` | 記事URLと出力先の対応、相対パスの解決、配信URLの集合と予約領域の検査。 |
| `src/config.rs`、`src/text.rs` | `config.toml` の検証と公開URLの組み立て。出力に書く `title`・`description` の検査は `text.rs` を通す。HTML のエスケープは bitview、RSS と sitemap の XML のエスケープは `metadata.rs` が行う。 |
| `src/views.rs` | `views/` の読み込みと構成の検査（ページのファイル、CSS の置き場所）、ページごとの CSS の組み立て。 |
| `src/render.rs` | ビューへ渡す値の組み立て、bitview での描画、HTML圧縮。 |
| `src/tags.rs` | タグの検証と記事のタグ分け。タグに関わる出力はすべてここを通す。 |
| `src/metadata.rs` | JSON-LD、sitemap、RSSフィード、robots。 |
| `src/input.rs` | サイト入力のパス・ファイル種別の検査、読み込み、静的ファイルのコピー。 |
| `src/build.rs` | 入力の読み込みから公開までをまとめる。 |
| `src/output.rs` | 出力計画の衝突検査、ステージング、`dist/` の入れ替え。データ保護に関わるため、変更前にテストを読む。 |
| `src/dev.rs` | 開発サーバー、監視、再ビルド、SSE リロード。`build` と同じ生成経路を使う。 |
| `tests/cli.rs` | 実行バイナリを一時ディレクトリで起動する E2E テスト。入出力形式や保護動作の変更時の主な確認先。 |
| `tests/license_lists.rs` | `deny.toml` の `allow` と `about.toml` の `accepted` の一致を検査する。 |
| `.github/`、`scripts/`、`deny.toml`、`about.toml`、`about.hbs`、`Dockerfile`、`compose.yaml` | CI、依存の検査、リリース、smoke test、ライセンス一覧の生成、README のスクリーンショットの撮影、開発用コンテナ。 |

## 守る設計

コードだけでは理由が読み取りにくい設計を挙げる。変える場合は理由ごと見直す。

- 記事本文の生 HTML は、パーサー直後の位置情報付きイベントで検査する。本文のリンクと画像のすべてに検証と属性付与を同じ規則で適用するための禁止なので、例外を設けない。
- 本文は pulldown-cmark のイベントから `bitview::Html` の要素を組み立てる。HTML を文字列で組み立てず、エスケープ・URL のスキーム・属性の検査は bitview に任せる。
- テンプレートへは `render.rs` が組み立てた `bitview::Value` だけを渡し、表示用に整形した値にする。テンプレート変数は利用者向けの仕様（`docs/views.md`）。
- テンプレート変数の型は、値を組み立てる関数の隣に定義する。読み込み時に `Program::check` でビューを検査し、描画時に `Type::validate` で値を照合するので、型と値がずれると必ずエラーになる。
- HTML は描画の最後に一度だけ文字列にする。`dev` のリロードスクリプトはサイトのものではないので、生成するファイルには入れず、開発サーバーが HTML の応答に足す。
- リンクの検証は、`markdown.rs` が出力した参照先を、`OutputPlan` が確定した配信URLの集合と照合する。`build --dry-run` も同じ経路を通り、公開だけを行わない。
- 画像URLのハッシュは描画時に計算し、コピー時に書き込んだバイト列のハッシュと照合する。食い違えば公開前に失敗する。
- `dist/` は `.genbit-output` マーカーを持つ既存ディレクトリだけを入れ替える。ステージングで全出力を作ってから入れ替え、失敗時は旧出力を残す。
- `dev` は `dist/` に触れず、OSの一時ディレクトリへ出力する。一時ディレクトリは Tokio ランタイムより先に作り、停止時に実行中のビルドを待ってから消す。

## Development Commands

Rust 関連の開発・検証は `compose.yaml` の `cli` サービスで実行する。初回または `Dockerfile` 変更後はイメージをビルドする。

```sh
docker compose build cli
docker compose run --rm cli fmt --check
docker compose run --rm cli clippy --locked --all-targets --all-features -- -D warnings
docker compose run --rm cli test --locked --all-targets --all-features
```

Docker から利用者サイトを動かす例は `docs/development.md` を参照。

## Coding Conventions and Testing

- モジュールは `main.rs` から内部で宣言し、内部共有は必要な範囲で `pub(crate)` にする。
- 失敗し得る処理は `anyhow::Result` と `Context` / `with_context`、`ensure!` / `bail!` を使い、入力・出力パスをエラーに含める。外部入力は型へのデシリアライズと明示検証を行う。
- ユニットテストは各 `src/*.rs` の `#[cfg(test)]`、CLI の結合テストは `tests/cli.rs`。テスト名は挙動を説明する snake_case。`tests/cli.rs` ではビルドの成否を `build_ok` / `build_err`、記事を `article_source` で組み立てる。

## CI とリリース

- `.github/workflows/ci.yml` のジョブ名は main の ruleset の必須チェック名。変えるときは ruleset も更新する。PR ブランチへの push だけでは CI は動かない。
- `.github/workflows/deny.yml` は `deny.toml` に従って cargo-deny を実行する。勧告を無視するときは `deny.toml` の `ignore` に理由を書く。許可リスト外のライセンスが入る場合は、`deny.toml` を変える前に配布への影響を確認する。
- リリースは `.github/workflows/release.yml` を `main` で手動実行して下書きの Release を作り、確認してから手動で公開する。`v*` タグはルールセットで削除・付け替えを禁じているため、ワークフローはタグを作らず、下書きの公開時に GitHub が作る。

## Agent Guidelines

- 利用者への説明では、独立した論点・設定・原因を一つにまとめず、それぞれ分けて説明する。各論点が何を決め、何に影響するかを明確にする。
- 作業前に対象モジュール、呼び出し元、関連する `tests/cli.rs` と `scaffold/` を読む。変更後はドキュメント上の説明と生成結果を照合する。
- 入力サイトの構造、テンプレート変数、フロントマター、URL、`dist/` の保護は利用者向けの仕様として扱う。変更時は `docs/`（必要なら README）と初期素材を更新し、エラーとデータ保護のテストを追加する。
- 必要な範囲だけ変更し、既存の `Result` とパス付きエラー、厳格な lint を維持する。新規依存は用途と既存依存で代替できない理由を確認する。
- `scaffold/` は公開してよい初期値だけを置く。利用者サイトの実データや秘密情報をこのリポジトリへ追加しない。依存更新は意図して行う。
- Rust コード・依存・ビルド設定を変更したら、関連テストを整備して上記の fmt、Clippy、test を実行する。文書だけの変更なら内容と差分を確認する。実行できなかった検証はそのまま報告する。
- 課題の説明を実装の事実と混同しない。Issue にある仮説・改善案は着手時に再検証し、解決したら経緯を残して Issue を閉じる。
