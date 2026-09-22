# genbit

Rust製の静的サイトジェネレーター。現在は段階1（CLIとサイト雛形）のみ実装済みです。
`build` と `dev` は未実装で、説明付きのエラーを返します。

## Dockerで実行

ホストへのRust/Cargoのインストールは不要です。Docker Composeを使用します。
このREADMEのあるディレクトリで実行してください。

```sh
docker compose run --rm cli run -- --help
docker compose run --rm cli run -- new test-blog
```

生成されるサイト:

```text
test-blog/
  config.toml
  content/index.md
  templates/base.html
  templates/page.html
  styles/main.css
  static/.gitkeep
  .gitignore
```

サイト名は英数字で始まり、英数字・ハイフン・アンダースコアのみ使用できます。
既存ファイルやディレクトリは上書きしません。途中でI/Oエラーになった場合は、
調査できるよう作成途中のファイルを残し、失敗したパスを表示します。

フロントマターは `+++` で囲むTOML形式です。テンプレートの変数は
`site.title`、`page.title`、`content`（生成済み本文HTML）、`css`（CSS本文）を予定しています。
CSSの読み込みとHTML生成は段階2で実装します。

## 検証

```sh
docker compose run --rm cli fmt --check
docker compose run --rm cli test --locked
docker compose run --rm cli clippy --locked --all-targets -- -D warnings
```

依存キャッシュとビルド成果物はDockerの名前付きボリュームに保存します。
サイトの雛形はバイナリに同梱するため、実行時にこのリポジトリを探しません。
