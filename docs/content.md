# コンテンツ

記事は `content/` 以下のMarkdownファイルです。先頭に `+++` で囲んだTOMLフロントマターを書きます。

```markdown
+++
title = "最初の記事"
template = "page.html"
created_at = 2026-09-17 09:00
updated_at = 2026-09-22 00:00
description = "記事で扱う内容を簡潔に説明します。"
tags = ["react", "web-security"]
+++

ここに本文を書きます。
```

## フロントマター

| 項目 | 必須 | 説明 |
| --- | --- | --- |
| `created_at` | 必須 | 作成日時。 |
| `updated_at` | 必須 | 最終更新日時。更新していない記事では `created_at` と同じ値にします。`created_at` より前にはできません。 |
| `description` | 必須 | meta description、Open Graph、JSON-LD、RSSフィードに使う概要。空にはできません。 |
| `title` | 任意 | 記事のタイトル。省略時はファイル名。 |
| `template` | 任意 | `templates/` 以下のテンプレート。省略時は `page.html`。 |
| `tags` | 任意 | タグの配列。[タグ](#タグ)を参照。 |

未知のフィールドはエラーになります。`title` と `description` には制御文字（タブ・改行を除く）を使えません。RSSフィードやHTMLを壊さないためです。

### 日時

`created_at` と `updated_at` は、引用符なしのTOMLローカル日時を分単位で書きます（`YYYY-MM-DD HH:MM`）。日付と時刻の間は `T` でも構いません。日付のみの値や秒付きの値は受け付けません。日時は `config.toml` の `timezone` の地域の時刻として扱います。初期テンプレートの画面には日付だけを表示します。

`genbit new` が作るサンプル記事の日時は固定の例です。記事を使う場合は実際の日時へ書き換えてください。

### description

`description` は本文から自動生成せず、文字数で切り詰めることもしません。検索結果に表示される文章は検索エンジン側で決まります。

## タグ

- タグ名は、英小文字・数字からなる語をハイフンでつないだ形式に限ります（例: `react-19`）。
- 同じ記事の中で同じタグは重複できません。
- `untagged` は予約語です。

`templates/tags.html` から `/tags/` を、`templates/tag.html` からタグごとの `/tags/{tag}/` を生成し、sitemapにも追加します。タグのない記事は `/tags/untagged/` に表示し、テンプレートに渡す `tags` は `["untagged"]` になります。タグページの記事の並びはトップページと同じです。

## パスとURL

`content/` 以下のディレクトリ構造を保ったまま、`.md` を `.html` に替えて出力します。

| 入力 | 出力 | URL |
| --- | --- | --- |
| `content/entries/hello-world.md` | `dist/entries/hello-world.html` | `/entries/hello-world` |

- パスの各要素に使えるのは、ASCII英数字・`-`・`_` だけです。
- `content/index.md` は使えません。トップページは `config.toml` と `templates/root.html` から生成します。
- 記事は作成日時の新しい順に並べます。同じ日時の記事はURL順です。

記事URLには拡張子がありません。公開先で `.html` ファイルに対応づける必要があります。`genbit dev` はこの対応づけを行います。

## Markdownの処理

### 記事間のリンク

`.md` ファイルへの相対リンクは拡張子を取り除きます。`[次の記事](next.md#section)` は `next#section` になります。クエリとアンカーは保持します。画像、外部URL、`/` で始まるリンクは変換しません。

### サイト内リンクの検証

ビルド時に、記事のリンクと画像の参照先がサイト内に存在するかを確かめます。存在しなければビルドは失敗し、`dist/` を更新しません。エラーには記事のパス、リンク、解決後のパスを表示します。

```text
broken internal link in content/entries/a.md: missing#x resolves to /entries/missing, which is not generated
```

- 参照先として扱うのは、記事・トップページ・タグページ・404ページ・`feed.xml`・`sitemap.xml`・`robots.txt`・`static/` のファイルが配信されるURLです。記事は拡張子なしのURLと `.html` のURLのどちらでも一致します。
- 相対リンクは、ブラウザーと同じく記事のURLを基準に解決します。`content/entries/a.md` の `[次](next.md)` は `/entries/next`、`![図](../img/a.png)` は `/img/a.png` を確かめます。`.` と `..` も解決し、ルートより上を指す `..` はルートで止まります。
- `?` 以降のクエリと `#` 以降のアンカーは除いて確かめます。見出しの `id` が存在するかは確かめません。
- `%E7%94%BB` のようなパーセントエンコードはデコードしてから比べます。不正なエンコードはエラーになります。
- 大文字と小文字は区別します。`/Entries/A` は `/entries/a` と一致しません。
- `https:`・`mailto:`・`tel:` などスキームのあるURL、`//` で始まるURL、`#section` のような同じページ内へのリンクは確かめません。ネットワークにはアクセスしません。
- Markdownに直接書いたHTML（`<a href>` や `<img src>`）、テンプレートに書いたリンク、見出しの中に書いて取り除かれたリンクは確かめません。

### 外部リンク

`http://`・`https://`・`//` で始まるリンクは、`target="_blank"` と `rel="noopener noreferrer"` を付けて別タブで開きます。相対リンクとページ内リンクは同じタブで開きます。テンプレートに直接書いたリンクには適用しません。

### 見出し

`h2` と `h3` には見出しの文字列から一意な `id` を付け、見出し全体をその見出しへのリンクにします。英字は小文字にし、空白はハイフンに置き換え、日本語などASCII以外の文字はそのまま使います。同じ `id` になる見出しには `-2`、`-3` のような連番を付けます。見出しの中に書いたリンクはテキストだけを残します。JavaScriptは使いません。

### コードブロック

言語指定付きのコードブロックには、指定した言語名を左上にそのまま表示します（例: `tsx`）。言語指定のないコードブロックには表示しません。シンタックスハイライトは行いません。

### 表

GitHub Flavored Markdownの表の記法を使えます。区切り行の `:---`・`:---:`・`---:` による揃えの指定は、各セルの `style="text-align: ..."` として出力します。初期CSSは表に罫線を引き、本文の幅に収まらない表は横にスクロールできるようにしています。

```markdown
| ページ | 転送サイズ |
| --- | ---: |
| トップページ | 1,098 |
```

### 画像

Markdownの画像には `loading="lazy"` と `decoding="async"` を付けます。

genbitは画像ファイルを変換しません。`static/` に置いた画像は、形式・寸法・ファイル名を変えずに `dist/` へコピーします。転送量を減らしたい場合は、`static/` に置く前に外部ツールで事前に処理してください。処理の例を次に挙げます。

- 表示する幅より大きな画像の縮小
- 写真のWebP・AVIFへの変換
- 図やスクリーンショットのPNGの可逆圧縮

ツールには `cwebp`、`avifenc`、`oxipng`、ImageMagickなどがあります。変換前の画像は `static/` の外に保管してください。`static/` 以下のファイルはすべて公開されます。

形式を出し分けたい場合は、Markdownに `<picture>` 要素を直接書けます。直接書いたHTMLはそのまま出力され、`loading` と `decoding` は付きません。
