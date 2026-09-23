# genbit

genbitはRust製の静的サイトジェネレーターです。MarkdownとTeraテンプレートから静的なHTMLを生成します。サイトの作成・ビルド・ローカルプレビューにNode.jsは不要です。

## 使い方

Cargoが使える環境では、このリポジトリからCLIをインストールできます。

```sh
cargo install --path .
genbit new my-blog
cd my-blog
genbit dev
```

`dev` は起動時にビルドし、既定では `http://127.0.0.1:3000` で配信します。`content/`、`templates/`、`styles/`、`static/`、`config.toml` の変更を検知すると再ビルドし、成功時だけSSEでブラウザーを再読み込みします。アドレスとポートは `genbit dev --host 127.0.0.1 --port 3000` で変更できます。

公開用ファイルはサイトのルートで生成します。

```sh
genbit build
```

`dist/` の中身を静的ホスティングサービスへ配置してください。`build` の出力には開発用の再読み込みスクリプトを含めません。既存の `dist/` はgenbitが生成したことを確認できる場合にだけ更新します。

## 生成されるサイト

```text
my-blog/
├── config.toml
├── content/
│   └── entries/
│       └── hello-world.md
├── templates/
│   ├── base.html
│   ├── page.html
│   └── root.html
├── styles/
│   ├── common.css
│   ├── page.css
│   └── root.css
└── static/
```

`config.toml` の必須項目はサイトの `title` です。トップページは `templates/root.html` から自動生成され、ブログタイトルと記事一覧だけを表示します。`content/root.md` は不要です。

記事のMarkdownにはTOMLフロントマターで `created_at` を必ず指定します。時刻まで指定する場合は、引用符なしのTOMLローカル日時（`YYYY-MM-DD HH:MM`、秒を指定するなら `YYYY-MM-DD HH:MM:SS`）を使います。日付と時刻の間は `T` でも構いません。時差変換はしないため、サイトで使う現地時刻を記入してください。従来のローカル日付（`YYYY-MM-DD`）も使え、その場合は並び替え時にその日の `00:00:00` として扱います。`updated_at` は任意のローカル日付で、作成日以降を指定します。どちらも表示は日付だけです。自分で記事を追加するときは執筆日と時刻を入力してください。`new` が作るサンプル記事の日時は固定の例なので、記事を使う場合は実際の日時へ書き換えてください。

```markdown
+++
title = "最初の記事"
template = "page.html"
created_at = 2026-09-17 09:00
updated_at = 2026-09-22
+++

# はじめに
```

`content/entries/hello-world.md` は `dist/entries/hello-world.html` になり、記事URLは `/entries/hello-world` です。コンテンツのディレクトリ構造を保ったまま、Markdownの拡張子を `.html` へ替えて出力します。拡張子なしのURLで配信するには、ホスティングサービス側が `.html` ファイルを対応づける必要があります。`genbit dev` では拡張子なしのURLを配信します。トップページには記事の作成日とタイトルを作成日時の新しい順に並べます。同じ日時の記事はURL順です。`static/` のファイルはそのままコピーされます。生成サイトのテンプレートとCSSは自由に編集できます。

全テンプレートで `site`（`title`）と `css` を参照できます。トップページの `root.html` には作成日時順の `entries` が渡され、各要素に `title`、`url`、`created_at`、`updated_at` が入ります。記事テンプレートには同じ項目を持つ `article` と、MarkdownをHTMLに変換した `content` が渡されます。テンプレートに渡す `created_at` は日付（`YYYY-MM-DD`）なので、表示形式は変わりません。

記事中の相対リンクで `.md` ファイルを指定すると、生成HTMLでは `.md` を取り除きます。例えば同じディレクトリの記事への `[次の記事](next.md#section)` は `next#section` になります。クエリとアンカーは保持します。画像、外部URL、`/` で始まるリンクは変換しません。

CSSは各HTMLへインライン展開され、HTMLとともに圧縮されます。`styles/common.css` は全ページへ適用する共通CSSです。任意でテンプレートと同じ相対パス・拡張子を `.css` に替えたCSS（例: `templates/page.html` に対する `styles/page.css`）を置くと、そのテンプレートを使うページだけに追加でインライン展開されます。Markdown画像には `loading="lazy"` と `decoding="async"` を付けます。現段階では画像ファイル自体の圧縮、コードのシンタックスハイライト、ダークモードは実装していません。生成HTMLのサイズは記事・テンプレート・CSSの内容によって変わります。

## Dockerでの開発

genbitの開発・検証はDocker Compose内で行えます。ホストにRustツールチェーンを入れる必要はありません。

```sh
docker compose build cli
docker compose run --rm cli fmt --check
docker compose run --rm cli clippy --locked --all-targets --all-features -- -D warnings
docker compose run --rm cli test --locked --all-targets --all-features
```

作成済みサイトをDocker内の `dev` でプレビューする例です。`SITE_DIR` にはサイトの絶対パスを指定してください。

```sh
SITE_DIR=/absolute/path/to/my-blog
docker compose run --rm \
  --publish 127.0.0.1:3000:3000 \
  --volume "$SITE_DIR:/site" \
  --workdir /site \
  cli run --locked --manifest-path /workspace/Cargo.toml -- dev --host 0.0.0.0
```

ホストのブラウザーでは `http://127.0.0.1:3000` を開きます。macOSのDocker Desktopでは、bind mount越しのファイル削除イベントがコンテナに届かない場合があります（[Dockerの報告](https://github.com/docker/for-mac/issues/7246)）。削除後にページが残った場合は、別のコンテナで次を実行し、ブラウザーを再読み込みしてください。

```sh
docker compose run --rm \
  --volume "$SITE_DIR:/site" \
  --workdir /site \
  cli run --locked --manifest-path /workspace/Cargo.toml -- build
```

ファイルの作成・編集とSSE通知はこの環境で確認済みです。監視にはファイル変更イベントを使い、ポーリングは行いません。
