# genbit

genbitはRust製の静的サイトジェネレーターです。MarkdownとTeraテンプレートから静的なHTMLを生成します。サイトの作成・ビルド・ローカルプレビューにNode.jsは不要です。

現在は作者の[blog](https://memo.nnenn0.com/)での利用を中心に開発しています。ソースから試すことはできますが、crates.ioやビルド済みバイナリでの配布、対応OS・CPUの範囲はまだ定めていません。公開先はドメインのルートに配置し、記事の拡張子なしURLを配信できる必要があります。

## 使い方

Rust 1.98.1のCargoが使える環境では、このリポジトリからCLIをインストールできます。以下の手順はDocker内のLinux環境で確認済みです。

```sh
cargo install --path .
genbit new my-blog
cd my-blog
genbit dev
```

`dev` は起動時にビルドし、既定では `http://127.0.0.1:3000` で配信します。ポートが使用中ならビルドせずに終了します。`content/`、`templates/`、`styles/`、`static/`、`config.toml` の変更を検知すると再ビルドし、成功時だけSSEでブラウザーを再読み込みします。再ビルドに失敗しても最後に成功したサイトを配信します。成功した再ビルドで `dist/` を切り替える瞬間には、独立したHTTPリクエストが一時的に404になる場合があります。アドレスとポートは `genbit dev --host 127.0.0.1 --port 3000` で変更できます。
`templates/404.html` は必須です。`dist/404.html` を生成し、`dev` は存在しないURLにこのページをHTTP 404で返します。`new` は404用テンプレートを用意します。公開先で同じ動作をさせるには、ホスティング側にも `404.html` をHTTP 404で配信する設定が必要です。

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
│   ├── root.html
│   ├── tags.html
│   └── tag.html
├── styles/
│   ├── common.css
│   ├── page.css
│   ├── root.css
│   ├── tags.css
│   └── tag.css
└── static/
    └── assets/img/
        ├── favicon.png
        ├── favicon.svg
        └── ogp.png
```

`config.toml` の必須項目はサイトの `title`、`description`、`site_url`、`og_image` です。`description` にはトップページの説明文を指定します。`new` が入れる説明文は仮の文章なので、公開前にサイトの内容に合わせて書き換えてください。トップページは `templates/root.html` から自動生成され、ブログタイトル・記事一覧・タグ一覧へのリンクを表示します。`content/root.md` は不要です。

`site_url` も必須です。`new` はローカルプレビュー用の `http://127.0.0.1:3000/` を設定するため、公開前に正式なサイトURL（例: `https://example.com/`）へ変更してください。サイトをドメインのルートに置くHTTP(S)のURLだけを受け付けます。トップページと全記事に絶対URLの canonical リンクを出し、同じURLを列挙した `dist/sitemap.xml` と、その Sitemap を案内する `dist/robots.txt` を生成します。sitemapの記事URLには、`updated_at` の日付を `<lastmod>` として出力します。記事URLは拡張子なしの `/entries/hello-world` を使うため、公開先もそのURLを配信する必要があります。サブパス配信にはまだ対応していません。

`og_image` にはトップページと全記事で共通して使うOGP画像を指定します。`new` は1200×630ピクセルの `static/assets/img/ogp.png` と、それを指すルート相対URL `/assets/img/ogp.png` を用意します。ルート相対URLはビルド時に `site_url` と結合されます。外部サービスに置いた画像を使う場合は、HTTP(S)の絶対URLも指定できます。生成ページにはOpen Graphメタデータを出力し、トップページには `WebSite`、記事ページには `BlogPosting` のJSON-LD構造化データを出力します。記事単位のOGP画像と著者情報には対応していません。作成済みのサイトは自動更新されないため、`config.toml` への `og_image` 追加とテンプレートへのメタデータ追加が必要です。

`new` はfaviconとして96×96ピクセルのPNGとSVGを用意します。初期テンプレートではPNGを先に指定し、SVG対応ブラウザー向けの候補も併記します。作成済みのサイトは自動更新されないため、必要に応じてPNGの追加と `templates/base.html` の変更を行ってください。

記事のMarkdownにはTOMLフロントマターで `created_at` と `updated_at` を必ず指定します。更新していない記事では `updated_at` に `created_at` と同じ日時を指定します。どちらも引用符なしのTOMLローカル日時（`YYYY-MM-DD HH:MM`）を使い、日付のみや秒付きの値は受け付けません。日付と時刻の間は `T` でも構いません。時差変換はせず、`updated_at` は `created_at` 以降の日時を指定します。画面とsitemapには日付だけを出力します。自分で記事を追加するときは執筆日と時刻を入力してください。`new` が作るサンプル記事の日時は固定の例なので、記事を使う場合は実際の日時へ書き換えてください。

```markdown
+++
title = "最初の記事"
template = "page.html"
created_at = 2026-09-17 09:00
updated_at = 2026-09-22 00:00
description = "記事で扱う内容を簡潔に説明します。"
tags = ["react", "web-security"]
+++

# はじめに
```

記事の `description` は必須です。記事を書いた後、その内容を説明する文章をフロントマターに指定してください。本文からの自動生成や文字数による切り詰めは行いません。検索結果に表示される文章は検索エンジン側で決まります。

`tags` は省略可能な文字列の配列です。タグ名は英小文字・数字からなる語をハイフンでつないだ形式（例: `react-19`）に限り、同じ記事内で重複できません。`untagged` は予約語です。`templates/tags.html` と `templates/tag.html` から `/tags/` と `/tags/{tag}/` を生成し、sitemapにも追加します。タグのない記事は `/tags/untagged/` に表示し、テンプレートに渡す `tags` は `["untagged"]` になります。タグページの記事はトップページと同じ作成日時順です。

`content/entries/hello-world.md` は `dist/entries/hello-world.html` になり、記事URLは `/entries/hello-world` です。コンテンツのディレクトリ構造を保ったまま、Markdownの拡張子を `.html` へ替えて出力します。拡張子なしのURLで配信するには、ホスティングサービス側が `.html` ファイルを対応づける必要があります。`genbit dev` では拡張子なしのURLを配信します。トップページには記事の作成日とタイトルを作成日時の新しい順に並べます。同じ日時の記事はURL順です。`static/` のファイルはそのままコピーされます。初期テンプレートは記事ページに `created_at`・`updated_at`・`tags` を `キー: 値` の形で表示します。生成サイトのテンプレートとCSSは自由に編集できます。

記事と静的ファイルが同じ配信URLを使う構成はビルド時に拒否します。たとえば `content/foo.md` と `static/foo` はどちらも `/foo` を使い、`static/foo/index.html` も `/foo` からのディレクトリアクセスと競合します。`.html` の直接URLと、開発サーバーで使う拡張子なしURLも検査します。`/__genbit` とその配下は開発サーバー用の予約URLなので、記事や静的ファイルには使えません。衝突時は旧 `dist/` を更新しません。

全テンプレートで `site`（`title`、`description`、`site_url`、絶対URLに解決済みの `og_image`）と `css` を参照できます。トップページ・記事・タグページのテンプレートには `description`、`canonical_url`、`json_ld` も渡されます。トップページの `root.html` には作成日時順の `entries` が渡され、各要素に `title`、`description`、`url`、`created_at`、`updated_at`、`tags` が入ります。記事テンプレートには同じ項目を持つ `article` と、MarkdownをHTMLに変換した `content` が渡されます。タグ一覧の `tags.html` には `tags`（各要素に `name`、`url`、`count`）が、タグ別の `tag.html` には `tag` と作成日時順の `entries` が渡されます。404用テンプレートには `site` と `css` だけを渡します。初期テンプレートは `base.html` の `metadata` ブロックを上書きして `noindex` を指定し、canonical・OGP・JSON-LDを出しません。テンプレートに渡す `created_at` と `updated_at` は日付（`YYYY-MM-DD`）なので、表示形式は変わりません。

記事中の相対リンクで `.md` ファイルを指定すると、生成HTMLでは `.md` を取り除きます。例えば同じディレクトリの記事への `[次の記事](next.md#section)` は `next#section` になります。クエリとアンカーは保持します。画像、外部URL、`/` で始まるリンクは変換しません。

初期の記事テンプレートはタイトル全体を記事URLへのリンクにします。Markdown本文の `h2` と `h3` には見出しの文字列から一意な `id` を生成し、見出し全体をその見出しへのリンクにします。見出し内に書いたリンクはテキストだけを残します。英字は小文字にし、空白はハイフンに置き換え、日本語はそのまま使います。同じ `id` になる見出しには `-2`、`-3` のような連番を付けます。JavaScriptは使いません。

Markdownの言語指定付きコードブロックには、指定した言語名を左上に表示します。例えば `tsx` はそのまま `tsx` と表示し、表示名への変換はしません。言語指定のないコードブロックには表示しません。シンタックスハイライトは行いません。

記事中の `http://`・`https://`・`//` で始まるリンクは別タブで開きます。生成されるリンクには `target="_blank"` と `rel="noopener noreferrer"` が付きます。相対リンクとページ内リンクは同じタブで開きます。テンプレートに直接書いたリンクにはこの処理は適用されません。

CSSは各HTMLへインライン展開され、HTMLとともに圧縮されます。`styles/common.css` は全ページへ適用する共通CSSです。任意でテンプレートと同じ相対パス・拡張子を `.css` に替えたCSS（例: `templates/page.html` に対する `styles/page.css`）を置くと、そのテンプレートを使うページだけに追加でインライン展開されます。初期CSSは落ち着いた淡色の背景、狭めの本文幅、広めの行間を使い、OSのダークモード設定には `prefers-color-scheme` で追従します。手動切り替え用のJavaScriptはありません。`new` で作成済みのサイトのCSSやテンプレートは自動更新されません。Markdown画像には `loading="lazy"` と `decoding="async"` を付けます。現段階では画像ファイル自体の圧縮とコードのシンタックスハイライトは実装していません。生成HTMLのサイズは記事・テンプレート・CSSの内容によって変わります。

`config.toml` と `content/`・`templates/`・`styles/`・`static/` 以下の入力ファイルにはシンボリックリンクを使えません。CSSファイル自体だけでなく、その親ディレクトリがシンボリックリンクの場合もビルドを拒否します。テンプレート別の任意CSSがない場合は、そのままビルドできます。

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

## ライセンス

genbitのコード、同梱する初期テンプレート・CSS・サンプル記事・favicon・OGP画像は[MITライセンス](LICENSE)です。`new` で作ったサイトへコピーされた初期素材を再配布するときも、著作権表示とライセンス文を残してください。利用者が自分で書いた記事や追加した素材には、このライセンスを適用しません。外部のRust依存関係には、それぞれのライセンスが適用されます。
