# ビュー

ページのHTMLとCSSは `views/` に置きます。HTMLは[bitview](https://github.com/nnenn0/bitview)のテンプレートで組み立てます。bitviewは、HTMLを文字列ではなく値として、関数で組み立てる小さい言語です。`genbit new` が作ったファイルは自由に編集できます。

## 構成

```
views/
  pages/        ページを描画する関数。ページごとに1ファイル
    root.bv  root.css
    page.bv  page.css
    ...
  components/   ページが使う部品
    document.bv  document.css
    entry-list.bv  entry-list.css
    ...
```

- `views/` には `pages/` と `components/` だけを置けます。それぞれに置けるのは `.bv` と `.css` だけで、サブディレクトリやほかのファイルがあるとビルドエラーになります。名前が `.` で始まるファイルとディレクトリ（macOSのFinderが作る `.DS_Store` など）は読み飛ばします。
- `.bv` ファイルはすべて1つのプログラムとして読み込みます。どのファイルで定義した関数も、ほかのファイルから呼べます。同じ名前の関数を2つ定義するとビルドエラーになります。
- `X.css` は、関数 `X` を定義した `.bv` と同じディレクトリに置きます（[CSS](#css)）。

### ページ

genbitは、ページごとに次の関数を呼びます。ページ `X` の関数は `views/pages/X.bv` に定義します。どれも必須で、引数を1つ（以下 `ctx`）取り、`html(...)` 要素を返します。`<!doctype html>` はgenbitが付けます。

| ファイル | 関数 | 描画するもの |
| --- | --- | --- |
| `views/pages/root.bv` | `root` | トップページ `/` |
| `views/pages/page.bv` | `page` | 記事 |
| `views/pages/tags.bv` | `tags` | タグ一覧 `/tags/` |
| `views/pages/tag.bv` | `tag` | タグ別ページ `/tags/{tag}/` |
| `views/pages/not-found.bv` | `not-found` | `dist/404.html` |

`views/pages/` にほかの `.bv` ファイルを置いたり、ページの関数を別のファイルで定義したりすると、ビルドエラーになります。ページの中だけで使う関数（`page.bv` の `tag-link` など）は、そのページのファイルに定義して構いません。

## 書き方

```
fn page(ctx) =>
  layout(ctx, concat(ctx.article.title, " | ", ctx.site.title), ctx.article.description, "article",
    home-link(ctx),
    article(h1(ctx.article.title), ctx.content)
  )

-- 記事のタグへのリンク
fn tag-link(tag) => a({href: tag.url}, tag.name)
```

- HTMLの要素は関数として書きます。最初の引数がレコード（`{href: "/"}`）なら属性で、残りの引数は子です。
- HTMLの値は、HTMLの断片（ノードの並び）です。文字列は1つのテキストの断片として、断片のリストはそれらを順に並べた断片として、HTMLの代わりに子に渡せます。
- 値は文字列、真偽値、リスト、レコード、HTMLの5種類です。数値はありません。
- `ctx.article.title` のようにレコードの項目を読みます。
- `if 条件 then 値 else 値` の条件は真偽値に限り、両側は型を合わせられる値にします。空のリスト `[]` は空の断片でもあるので、何も出さない側は `else []` と書きます（`if ctx.article.draft then span("draft") else []`）。
- `concat(...)` は文字列を連結し、`map(リスト, 関数名)` はリストの各要素に関数を適用します。
- 名前は小文字と数字を `-` でつないだものです（`entry-list`）。`--` から行末まではコメントです。
- 再帰、ファイルの読み込み、テンプレートの継承やインクルードの構文はありません。共通の部分は関数にして呼びます。

言語の詳細はbitviewのREADMEを参照してください。

### HTMLの安全性

- 文字列は必ずエスケープして出力します。記事のタイトルなどに `<` や `"` が含まれていても、タグや属性にはなりません。
- ビューから、文字列をそのままHTMLとして入れる手段はありません。記事の本文（`ctx.content`）、CSS（`ctx.style`）、JSON-LD（`ctx.json-ld`）は、genbitがHTMLとして渡します。
- `script`・`style` 要素と、`onclick` などの `on*` 属性、`style` 属性は書けません。JavaScriptは使えず、CSSは `.css` ファイルに書きます。
- `href`・`src`・`cite` と、URLを並べる `srcset`・`ping` に書けるURLは、相対URLと `http:`・`https:`・`mailto:` だけです。
- 使える要素は、ページの骨組み（`header`・`footer`・`main`・`nav`・`section`・`article`・`aside` など）と、記事の本文で使うものです。
- 書ける属性は、全要素に共通の属性（`id`・`class`・`title`・`lang` など）、要素ごとに決まった属性（`a` の `href`・`target`・`rel`、`img` の `src`・`alt` など）、`data-*`、`aria-*` だけです。`herf` のような綴りの誤りはビルドエラーになります。
- 要素は、HTMLで置ける場所にだけ置けます。`p` の中の `div`、`ul` の直下の `p`、`head` の中の `p`、`a` の中の `a` のように、ブラウザーが組み替えてしまう入れ子はビルドエラーになります。`a` は中身と同じ扱いで、ブロックを包んだ `a` はブロックを置ける場所に置けます。

## 変数

### すべてのページ

| 変数 | 内容 |
| --- | --- |
| `site.title` | `config.toml` の `title` |
| `site.description` | `config.toml` の `description` |
| `site.url` | 正規化した `site_url` |
| `site.og-image` | 絶対URLに解決した `og_image` |
| `style` | そのページのCSSを入れた `<style>` 要素。[CSS](#css)を参照。 |

### `not-found` 以外のページ

| 変数 | 内容 |
| --- | --- |
| `canonical-url` | ページの絶対URL |
| `json-ld` | 構造化データの `<script type="application/ld+json">` 要素。記事は `BlogPosting`、それ以外はサイトの `WebSite` |

### ページごと

| 関数 | 変数 |
| --- | --- |
| `root` | `entries`: 全記事（作成日時の新しい順） |
| `page` | `article`: 記事、`content`: MarkdownをHTMLに変換した本文 |
| `tags` | `tags`: タグのリスト。各要素は `name`、`url`、`count`（記事数を表す文字列） |
| `tag` | `tag`: タグ名、`entries`: そのタグの記事（作成日時の新しい順） |
| `not-found` | なし |

`entries` の各要素と `article` は次の項目を持ちます。

| 項目 | 内容 |
| --- | --- |
| `title`、`description` | フロントマターの値 |
| `url` | `/entries/hello-world/` のように `/` で終わる記事のURL |
| `created-at`、`updated-at` | `datetime`（`config.toml` の `timezone` の時差を付けたRFC 3339の日時。例: `2026-09-17T09:00:00+09:00`）と `date`（`YYYY-MM-DD`）を持つレコード |
| `tags` | `name` と `url`（`/tags/react/` など）を持つレコードのリスト。タグのない記事は `untagged` の1つ |
| `draft` | `genbit dev` で `drafts/` から読んだ[下書き](content.md#下書き)なら `true`。`genbit build` では常に `false` |

`<time>` 要素の `datetime` 属性には `datetime`、画面の表示には `date` を使います（例: `time({datetime: ctx.article.created-at.datetime}, ctx.article.created-at.date)`）。

genbitはビューを読み込んだ直後、どのページを描画するよりも前に、各ページの関数をこれらの変数の型で検査します。`if` の両側と `map` の中も検査するので、下書きのときにしか通らない部分の誤りも、ビルドの最初に見つかります。ない項目を読む、真偽値を子にする、といった誤りは、ビューの位置と、そのレコードにある項目の一覧を付けてエラーになります。URLのスキームのように値そのものに依存する誤りは、そのページの描画で見つかります。

関数は、ページの関数からの呼び出しを通して検査します。どのページの関数からも呼ばれない関数は検査できないので、ビルドエラーになります。使わなくなった関数は削除してください。

```text
views/components/home-link.bv:3:1: function site-name is not called from root, page, tags, tag, not-found, so it cannot be checked; call it or remove it
```

## CSS

`X.css` は関数 `X` のCSSで、`X` を定義した `.bv` と同じディレクトリに置きます。部品のHTMLとCSSが並ぶので、まとめて読み、直し、消せます。対応する関数が同じディレクトリにないCSSはビルドエラーになるので、関数の名前を変えたり別のファイルへ移したりしたときに、CSSが取り残されることはありません。どのCSSファイルも任意です。

ページの `style` には、そのページの関数から呼ばれうる関数のCSSが入ります。`if` は両方の側を数えるので、ページに入るCSSは記事の内容によらず、関数ごとに決まります。初期のビューでは、記事ページの `style` に次の順でCSSが入ります。

```
document.css → seo.css → layout.css → home-link.css → draft-badge.css → timestamp.css → tag-link.css → page.css
```

（存在するファイルだけが入ります。初期のCSSでは `document.css`、`draft-badge.css`、`page.css` の3つです。）

- 呼ばれる関数のCSSが、呼ぶ関数のCSSより先に入ります。全ページの骨組みのCSS（`document.css`）が最初、ページの関数のCSS（`page.css` など）が最後になり、後のCSSが前のCSSを上書きできます。
- 互いに呼び合わない関数のCSSは、ビューで呼んでいる順に入ります。呼び出しの順を入れ替えても見た目が変わらないように、部品のCSSは自分のクラスの中だけを指定してください（`entry-list.css` なら `.entry-list` の中）。
- CSSを置く位置はビューが決めます。初期のビューは `<head>` に `ctx.style` を置きます。
- CSSに `</style`・`</script`・`<!--` を書くと、ビルドエラーになります。

CSSは各HTMLへインライン展開し、HTMLとともに圧縮します。

初期テーマは落ち着いた淡色の背景、狭めの本文幅、広めの行間を使い、`prefers-color-scheme` でOSのダークモード設定に追従します。色は `views/components/document.css` の `:root` でCSS変数として定義し、ダークモードの値もそこでまとめて切り替えます。手動の切り替えやJavaScriptはありません。

本文のローカルPNG・JPEG・GIF・WebPには `width`・`height` 属性が付きます（[画像](content.md#画像)）。初期の `document.css` は `img { display: block; max-width: 100%; height: auto; margin-inline: auto; }` で縦横比を保ち、1枚ずつ縦に中央寄せで配置します。独自のCSSでも縦横比を保つために `height: auto` を指定してください。

## 初期のビュー

部品は、主な関数と同じ名前のファイルに置いています。`views/components/` の分け方は自由に変えて構いません。

- `components/document.bv` は全ページの骨組みで、タイトル、favicon、RSSの自動検出用リンク、`style` を出力します。`<html lang="ja">` もここで指定します。
- `components/layout.bv` は、`document` にmeta description、canonicalリンク、Open Graph、JSON-LDを加えた骨組みです。`root`・`page`・`tags`・`tag` が使います。これらの要素を出力する `seo` も同じファイルに置きます。説明文は各ページが渡します（トップページは `config.toml` の `description`、記事は記事の `description`、タグ一覧とタグ別ページは固定の文）。
- `components/home-link.bv` は、ヘッダーに置くトップページへのリンクです。
- `components/entry-list.bv` は記事の一覧で、トップページとタグ別ページが使います。
- `components/timestamp.bv` は日時を `<time>` 要素で表示します。
- `components/draft-badge.bv` は下書きに「draft」の印を付けます。記事一覧では日付の後に、記事ページではタイトルの後に付けます。
- `pages/not-found.bv` は `layout` ではなく `document` を使い、`noindex` を指定します。canonical・Open Graph・JSON-LDは出しません。
- `pages/page.bv` は記事タイトルを `h1` で表示し、`created_at`・`updated_at`・`tags` を `キー: 値` の形で表示します。日時は日付だけを表示し、`<time>` 要素の `datetime` 属性に時刻と時差を含めます。
- `pages/root.bv` は記事一覧の後に、タグ一覧とRSSフィードへのリンクを表示します。
- faviconはPNGを先に指定し、SVG対応ブラウザー向けのSVGも併記します。

## 作成済みのサイト

genbitを更新しても、作成済みのサイトのビューは変更しません。新しい初期のビューの機能を使う場合は、自分でサイトへ反映してください。
