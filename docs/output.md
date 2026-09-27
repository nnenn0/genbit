# 出力と公開

## 生成するファイル

`genbit build` はすべてを `dist/` に書き出します。

| パス | 内容 |
| --- | --- |
| `index.html` | `templates/root.html` から生成するトップページ |
| `entries/*.html` など | `content/` の構造どおりに、記事ごとに1ファイル |
| `tags/index.html`、`tags/{tag}/index.html` | タグ一覧とタグ別ページ |
| `404.html` | `templates/404.html` から生成する404ページ |
| `sitemap.xml` | トップページ・記事・タグページ。記事は `updated_at` を時差付きの日時で `<lastmod>` に使います。 |
| `robots.txt` | sitemapの場所を案内します |
| `feed.xml` | RSS 2.0フィード |
| `static/` 以下のすべて | そのままコピー |
| `.genbit-output` | `dist/` がgenbitの出力であることを示すマーカー |

`build` の出力には、開発サーバー用の再読み込みスクリプトを含めません。

### メタデータ

404ページ以外の全ページに、絶対URLのcanonicalリンクとOpen Graphを出力します。トップページとタグページには `WebSite`、記事には `BlogPosting` のJSON-LDを出力します。`BlogPosting` の `datePublished` と `dateModified` は、`created_at` と `updated_at` に `timezone` を付けた日時です。記事ごとの画像と著者情報には対応していません。

### RSSフィード

- チャンネルの `title` と `description` は `config.toml` の値、`link` は `site_url` です。
- 作成日時の新しい順に最新20件の記事を載せます。
- 各 `item` の `link` と `guid` には記事のcanonical URL、`description` には記事の `description`、`pubDate` には `created_at` を `timezone` の時差付きで使います。`updated_at` は使いません。
- フィードはsitemapに含めません。

## ビルドの確認（`--dry-run`）

`genbit build --dry-run` は、通常のビルドと同じ読み込み・検証・描画を最後まで行い、`dist/` を変更せずに終了します。成功すると `Checked N pages; dist/ was not changed` と表示します。

- 記事とテンプレートの描画、タグページ・404ページ・`feed.xml`・`sitemap.xml` の生成、出力パスとURLの衝突、[サイト内リンク](content.md#サイト内リンクの検証)を検査します。
- 既存の `dist/` が入れ替えられるか（`.genbit-output` マーカーがあるか）も確かめます。
- `dist/` を作らず、一時ディレクトリ（`.genbit-build-*`・`.genbit-backup-*`）も作りません。
- ファイルを書き込まないため、ディスク容量や書き込み権限の不足は確かめられません。`static/` のファイルは一覧を作る時点で検査しますが、コピー直前の再検査は行いません。

サイトのリポジトリで変更を確かめるCIには `genbit build --dry-run` が向いています。リンク切れや記事の誤りがあれば0以外の終了コードで終了し、成果物を残しません。公開用の `dist/` を作るCIでは `genbit build` を使ってください。同じ検査を行い、失敗すれば `dist/` を作らずに終了するため、前に `--dry-run` を実行する必要はありません。genbitの入れ方は[READMEのインストール](../README.md#インストール)と同じです。

## `dist/` の保護

- ビルドはサイト直下の一時ディレクトリで行い、全ファイルを書き終えてから `dist/` を入れ替えます。
- ビルドに失敗した場合、以前の `dist/` はそのまま残ります。
- 既存の `dist/` は `.genbit-output` マーカーがある場合だけ入れ替えます。genbitが作っていないディレクトリを消すことはありません。
- `config.toml` と、`content/`・`templates/`・`styles/`・`static/` 以下のシンボリックリンクは拒否します。CSSファイルの親ディレクトリも対象です。

## URLの衝突

2つの出力が同じURLで配信される構成は、ビルド時に拒否し、`dist/` を更新しません。たとえば `content/foo.md` と `static/foo` はどちらも `/foo` を使い、`static/foo/index.html` も `/foo` に応答します。`.html` の直接URLと、開発サーバーで使う拡張子なしURLの両方を検査します。`static/feed.xml` のように生成ファイルと衝突するパスも拒否します。

大文字と小文字だけが異なる出力も拒否します。ファイル名に加えてディレクトリ名も対象です。たとえば `content/Docs/post.md` と `static/docs/image.svg` は、`dist/Docs/` と `dist/docs/` を作ることになるので拒否します。macOSやWindowsの一般的なファイルシステムでは両者が1つのディレクトリにまとまり、公開先によってリンクが切れるためです。

`/__genbit` とその配下は開発サーバー用の予約URLです。

## 公開先の条件

- サイトをドメインのルートで配信すること。
- 記事URLを拡張子なしで配信すること。`/entries/hello-world` で `entries/hello-world.html` を返す必要があります。
- 存在しないURLには `404.html` をHTTP 404で返すこと。

## 開発サーバー

`genbit dev` はサイトをビルドし、`http://127.0.0.1:3000` で `dist/` を配信します。アドレスは `--host` と `--port` で変更できます。ポートが使用中なら、ビルドせずに終了します。

- 記事の拡張子なしURLは対応する `.html` から配信し（`/entries/%68ello-world` のようなパーセントエンコードはリンク検証と同じくデコードして扱います）、存在しないURLには `404.html` をHTTP 404で返します。
- `config.toml`、`content/`、`templates/`、`styles/`、`static/` の変更を検知すると、100 msの静穏期間の後、または最大500 msで再ビルドします。再ビルド中の変更は次の再ビルドで反映します。
- 再ビルドに成功するとSSEでブラウザーを再読み込みします。失敗した場合はエラーを表示し、最後に成功したサイトを配信し続けます。
- 変更の検知にはファイルシステムのイベントを使い、ポーリングはしません。
- Ctrl+C（SIGINT）またはSIGTERMで停止します。初回ビルドや再ビルドの途中なら、そのビルドが `dist/` を切り替え終えてから終了します。

## 既知の制限

- `dev` が再ビルド後に `dist/` を切り替える瞬間には、独立したHTTPリクエストが一時的に404になる場合があります。自動再読み込みは切り替え後に行うため、通常の編集では影響しません。
- `build` の実行中にCtrl+Cを押したり、`build` や `dev` をSIGKILLなどで強制終了したりすると、`dist/` の切り替え途中で止まる場合があります。このとき `dist/` がなくなり、旧出力はサイト直下の `.genbit-backup-*/dist` に残ります。自動復旧はしませんが、もう一度 `genbit build` すれば `dist/` を作り直せます。残った `.genbit-build-*` と `.genbit-backup-*` は削除して構いません。
