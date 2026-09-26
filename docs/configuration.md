# 設定

サイトのルートには `config.toml` を置きます。`genbit new` が作成します。

```toml
title = "My notes"
description = "小さなツール作りについてのメモです。"
site_url = "https://example.com/"
og_image = "/assets/img/ogp.png"
timezone = "+09:00"
```

未知のキーはエラーになります。書き間違いが無視されず、ビルドの失敗として分かります。

## 項目

| 項目 | 必須 | 説明 |
| --- | --- | --- |
| `title` | 必須 | サイトのタイトル。空にはできません。トップページ、各ページのタイトル、RSSのチャンネル名に使います。 |
| `description` | 必須 | トップページの説明文。空にはできません。RSSのチャンネルの説明にも使います。`genbit new` が入れる文章は仮のものなので、公開前に書き換えてください。 |
| `site_url` | 必須 | サイトのルートの公開URL（例: `https://example.com/`）。 |
| `og_image` | 必須 | トップページと全記事で共通のOGP画像。 |
| `timezone` | 任意 | 記事の日時のUTCオフセット（例: `+09:00`、`-05:00`）。省略時は `+00:00`。 |

### `site_url`

- ホストを持つ `http` か `https` の絶対URLを指定します。
- ドメインのルートを指す必要があります。パス・クエリ・フラグメント・認証情報（`user@host`）は受け付けません。サブパスでの配信には対応していません。
- 末尾の `/` がなければ補います。
- `genbit new` はローカルプレビュー用に `http://127.0.0.1:3000/` を設定します。公開前に正式なURLへ変更してください。

`site_url` は、canonicalリンク、`sitemap.xml`、`robots.txt`、`feed.xml`、Open Graph、JSON-LDの絶対URLを組み立てるのに使います。

### `og_image`

- `/assets/img/ogp.png` のようなルート相対パスは、ビルド時に `site_url` と結合します。
- `http` か `https` の絶対URLはそのまま使います。外部に置いた画像も指定できます。
- フラグメント（`#...`）は受け付けません。

`genbit new` は1200×630ピクセルの `static/assets/img/ogp.png` を用意し、`og_image` をそこへ向けます。記事ごとの画像には対応していません。

### `timezone`

記事の日時はオフセットなしのローカル時刻で書き、genbitは時差変換をしません。`timezone` のオフセットは、RSSフィードの `pubDate` に付けるだけです。

## 作成済みのサイト

`genbit new` は初期ファイルを一度コピーするだけです。genbitを更新しても、作成済みのサイトの `config.toml`・テンプレート・CSSは更新されません。必須項目やテンプレートの機能が追加された場合は、自分でサイトを更新してください。
