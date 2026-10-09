# ビュー

ページのHTMLとCSSは `views/` に置きます。HTMLは[bitview](https://github.com/nnenn0/bitview)のテンプレートで組み立てます。bitviewは、HTMLを文字列ではなく値として、関数で組み立てる小さい言語です。`genbit new` が作ったファイルは自由に編集できます。

## 構成

```
views/
  pages/        ページを描画する関数。ページごとに1ファイル
    home.bv  home.css
    entry.bv  entry.css
    ...
  components/   ページが使う部品
    document.bv  document.css
    entry-list.bv  entry-list.css
    ...
```

- `views/` には `pages/` と `components/` だけを置けます。それぞれに置けるのは `.bv` と `.css` だけで、サブディレクトリやほかのファイルがあるとビルドエラーになります。名前が `.` で始まるファイルとディレクトリ（macOSのFinderが作る `.DS_Store` など）は読み飛ばします。
- `.bv` ファイルはすべて1つのプログラムとして読み込みます。`defn` で定義した関数は、ほかのファイルからも呼べ、名前はファイルをまたいで1つずつしか定義できません。`defn-` で定義した関数は、そのファイルの中からだけ呼べ、別のファイルの `defn-` の関数と同じ名前を付けられます。
- `X.css` は、`defn` で定義した関数 `X` と同じディレクトリに置きます（[CSS](#css)）。

### ページ

genbitは、ページごとに次の関数を呼びます。ページ `X` の関数は `views/pages/X.bv` に `defn` で定義します。どれも必須で、表の型の引数を1つ（以下 `ctx`）取り、`(html ...)` 要素を返します。`<!doctype html>` はgenbitが付けます。

| ファイル | 関数 | 引数の型 | 描画するもの |
| --- | --- | --- | --- |
| `views/pages/home.bv` | `home` | `HomePage` | トップページ `/` |
| `views/pages/entry.bv` | `entry` | `EntryPage` | 記事 |
| `views/pages/tags.bv` | `tags` | `TagsPage` | タグ一覧 `/tags/` |
| `views/pages/tag.bv` | `tag` | `TagPage` | タグ別ページ `/tags/{tag}/` |
| `views/pages/not-found.bv` | `not-found` | `NotFoundPage` | `dist/404.html` |

`views/pages/` にほかの `.bv` ファイルを置いたり、ページの関数を別のファイルで定義したりすると、ビルドエラーになります。ページの中だけで使う関数（`entry.bv` の `tag-link` など）は、そのページのファイルに `defn-` で定義して構いません。

## 書き方

```clojure
(defn entry [ctx EntryPage]
  (layout ctx (concat ctx.entry.title " | " ctx.site.title) ctx.entry.description "article"
    (home-link ctx.site)
    (article (h1 ctx.entry.title) ctx.content)))

; 記事のタグへのリンク
(defn- tag-link [tag TagLink]
  (a {:href tag.url} tag.name))
```

- 関数は `(defn 名前 [引数 型 …] 本体)` と書き、呼び出しは `(関数 引数 …)` と書きます。引数には必ず型を書きます（[型](#型)）。
- HTMLの要素は関数として書きます。最初の引数がレコード（`{:href "/"}`）なら属性で、残りの引数は子です。
- HTMLの値は、HTMLの断片（ノードの並び）です。文字列は1つのテキストの断片として、断片のリストはそれらを順に並べた断片として、HTMLの代わりに子に渡せます。
- 値は文字列、真偽値、リスト、レコード、HTMLの5種類です。数値はありません。
- `ctx.entry.title` のように、引数のレコードの項目を読みます。
- `(if 条件 値 値)` の条件は真偽値に限り、両側は型を合わせられる値にします。空のリスト `[]` は空の断片でもあるので、何も出さない側は `[]` と書きます（`(if ctx.entry.draft (span "draft") [])`）。
- `(concat …)` は文字列を連結し、`(map リスト 関数名)` はリストの各要素に関数を適用します。
- 名前は小文字と数字を `-` でつないだものです（`entry-list`）。`;` から行末まではコメントです。
- 再帰、ファイルの読み込み、テンプレートの継承やインクルードの構文はありません。共通の部分は関数にして呼びます。

言語の詳細はbitviewのREADMEを参照してください。

### HTMLの安全性

- 文字列は必ずエスケープして出力します。記事のタイトルなどに `<` や `"` が含まれていても、タグや属性にはなりません。
- ビューから、文字列をそのままHTMLとして入れる手段はありません。記事の本文（`ctx.content`）、CSS（`ctx.style`）、JSON-LD（`ctx.search.json-ld`）は、genbitがHTMLとして渡します。
- `script`・`style` 要素と、`onclick` などの `on*` 属性、`style` 属性は書けません。JavaScriptは使えず、CSSは `.css` ファイルに書きます。
- `href`・`src`・`cite` と、URLを並べる `srcset`・`ping` に書けるURLは、相対URLと `http:`・`https:`・`mailto:` だけです。
- 使える要素は、ページの骨組み（`header`・`footer`・`main`・`nav`・`section`・`article`・`aside` など）と、記事の本文で使うものです。
- 書ける属性は、全要素に共通の属性（`id`・`class`・`title`・`lang` など）、要素ごとに決まった属性（`a` の `href`・`target`・`rel`、`img` の `src`・`alt` など）、`data-*`、`aria-*` だけです。`herf` のような綴りの誤りはビルドエラーになります。
- 要素は、HTMLで置ける場所にだけ置けます。`p` の中の `div`、`ul` の直下の `p`、`head` の中の `p`、`a` の中の `a` のように、ブラウザーが組み替えてしまう入れ子はビルドエラーになります。`a` は中身と同じ扱いで、ブロックを包んだ `a` はブロックを置ける場所に置けます。

## 型

関数の引数には、名前の後に型を書きます。型は、`String`（文字列）、`Bool`（真偽値）、HTMLの `Flow`（`<body>` の中に置けるもの）・`Phrasing`（文中に置けるもの）・`Metadata`（`<head>` の中に置けるもの）、リスト `[String]`、レコード `{:title String :draft Bool}` と、genbitが名前を付けた型（[変数](#変数)）です。

```clojure
(defn draft-badge [entry {:draft Bool}]
  (if entry.draft (span {:class "draft-badge"} "draft") []))
```

- レコードの型は、その関数が読む項目の一覧です。それより多くの項目を持つレコードも渡せるので、`draft-badge` には記事（`Entry`）も、`draft` を持つほかのレコードも渡せます。
- 関数の中から読めるのは、型に書いた項目だけです。書いていない項目を読むと、ビルドエラーになります。
- 呼び出す側では、渡す値が引数の型に合うかを検査します。

genbitは、ビューを読み込むとき、どのページを描画するよりも前に、すべての関数をそれぞれの引数の型で検査します。`if` の両側と `map` の中も検査するので、下書きのときにしか通らない部分の誤りも、ビルドの最初に見つかります。ない項目を読む、真偽値を子にする、といった誤りは、誤りのある関数の位置と、そのレコードにある項目の一覧を付けてエラーになります。どのページからも呼ばれない関数も検査します。URLのスキームのように値そのものに依存する誤りは、そのページの描画で見つかります。

```text
views/components/draft-badge.bv:2:54: unknown field "titel" (fields: draft, title)
  in draft-badge
```

## 変数

ページの関数の引数（`ctx`）に渡す値の型と、その中で使う型には、genbitが名前を付けています。`genbit types` で、すべての型を、ビューで型を書くときと同じ構文で表示できます。VS Codeのbitview拡張機能は、このコマンドを使って、型の名前から宣言へ移動したり、ホバーで中身を表示したりします。

### すべてのページ

| 項目 | 型 | 内容 |
| --- | --- | --- |
| `site` | `Site` | `title`・`description`（`config.toml` の値）、`url`（正規化した `site_url`）、`og-image`（絶対URLに解決した `og_image`） |
| `style` | `Metadata` | そのページのCSSを入れた `<style>` 要素。[CSS](#css)を参照。 |

### `not-found` 以外のページ

| 項目 | 型 | 内容 |
| --- | --- | --- |
| `search` | `Search` | 検索エンジンが読むもの。`url`（ページの絶対URL）と `json-ld`（構造化データの `<script type="application/ld+json">` 要素。記事は `BlogPosting`、それ以外はサイトの `WebSite`） |

### ページごと

| 型 | 項目 |
| --- | --- |
| `HomePage` | `entries`（`[Entry]`）: 全記事（作成日時の新しい順） |
| `EntryPage` | `entry`（`Entry`）: 記事、`content`（`Flow`）: MarkdownをHTMLに変換した本文 |
| `TagsPage` | `tags`（`[TagCount]`）: タグのリスト。各要素は `name`、`url`、`count`（記事数を表す文字列） |
| `TagPage` | `tag`（`String`）: タグ名、`entries`（`[Entry]`）: そのタグの記事（作成日時の新しい順） |
| `NotFoundPage` | なし |

記事（`Entry`）は次の項目を持ちます。

| 項目 | 内容 |
| --- | --- |
| `title`、`description` | フロントマターの値 |
| `url` | `/entries/hello-world/` のように `/` で終わる記事のURL |
| `created-at`、`updated-at` | `datetime`（`config.toml` の `timezone` の時差を付けたRFC 3339の日時。例: `2026-09-17T09:00:00+09:00`）と `date`（`YYYY-MM-DD`）を持つレコード（`Timestamp`） |
| `tags` | `name` と `url`（`/tags/react/` など）を持つレコード（`TagLink`）のリスト。タグのない記事は `untagged` の1つ |
| `draft` | `genbit dev` で `drafts/` から読んだ[下書き](content.md#下書き)なら `true`。`genbit build` では常に `false` |

`<time>` 要素の `datetime` 属性には `datetime`、画面の表示には `date` を使います（例: `(time {:datetime stamp.datetime} stamp.date)`）。

## CSS

`X.css` は `defn` で定義した関数 `X` のCSSで、`X` を定義した `.bv` と同じディレクトリに置きます。`defn-` の関数は、ほかのファイルの `defn-` の関数と名前が重なりうるので、CSSファイルを持てません。`defn-` の関数が出す要素のCSSは、同じファイルの `defn` の関数のCSSに書きます（`entry-list.bv` の `entry-item` が出す `li` は `entry-list.css` に書く）。部品のHTMLとCSSが並ぶので、まとめて読み、直し、消せます。対応する関数が同じディレクトリにないCSSはビルドエラーになるので、関数の名前を変えたり別のファイルへ移したりしたときに、CSSが取り残されることはありません。どのCSSファイルも任意です。

ページの `style` には、そのページの関数から呼ばれうる `defn` の関数のCSSが入ります。`defn-` の関数を通って呼ばれる関数も数えます。`if` は両方の側を数えるので、ページに入るCSSは記事の内容によらず、関数ごとに決まります。初期のビューでは、記事ページの `style` に次の順でCSSが入ります。

```
document.css → layout.css → home-link.css → draft-badge.css → timestamp.css → entry.css
```

（存在するファイルだけが入ります。初期のCSSでは `document.css`、`draft-badge.css`、`entry.css` の3つです。）

- 呼ばれる関数のCSSが、呼ぶ関数のCSSより先に入ります。全ページの骨組みのCSS（`document.css`）が最初、ページの関数のCSS（`entry.css` など）が最後になり、後のCSSが前のCSSを上書きできます。
- 互いに呼び合わない関数のCSSは、ビューで呼んでいる順に入ります。呼び出しの順を入れ替えても見た目が変わらないように、部品のCSSは自分のクラスの中だけを指定してください（`entry-list.css` なら `.entry-list` の中）。
- CSSを置く位置はビューが決めます。初期のビューは `<head>` に `ctx.style` を置きます。
- CSSに `</style`・`</script`・`<!--` を書くと、ビルドエラーになります。

CSSは各HTMLへインライン展開し、HTMLとともに圧縮します。

初期テーマは落ち着いた淡色の背景、狭めの本文幅、広めの行間を使い、`prefers-color-scheme` でOSのダークモード設定に追従します。色は `views/components/document.css` の `:root` でCSS変数として定義し、ダークモードの値もそこでまとめて切り替えます。手動の切り替えやJavaScriptはありません。

本文のローカルPNG・JPEG・GIF・WebPには `width`・`height` 属性が付きます（[画像](content.md#画像)）。初期の `document.css` は `img { display: block; max-width: 100%; height: auto; margin-inline: auto; }` で縦横比を保ち、1枚ずつ縦に中央寄せで配置します。独自のCSSでも縦横比を保つために `height: auto` を指定してください。

## 初期のビュー

部品は、主な関数と同じ名前のファイルに置いています。`views/components/` の分け方は自由に変えて構いません。

- `components/document.bv` は全ページの骨組みで、タイトル、favicon、RSSの自動検出用リンク、`style` を出力します。`<html lang="ja">` もここで指定します。
- `components/document.bv` と `components/layout.bv` は、ページの値を `[page {:site Site :style Metadata}]` のように、自分が読む部分だけを型に書いて受け取ります。どのページの `ctx` もそのまま渡せます。
- `components/layout.bv` は、`document` にmeta description、canonicalリンク、Open Graph、JSON-LDを加えた骨組みです。`home`・`entry`・`tags`・`tag` が使います。これらの要素を出力する `seo` も同じファイルに `defn-` で置きます。説明文は各ページが渡します（トップページは `config.toml` の `description`、記事は記事の `description`、タグ一覧とタグ別ページは固定の文）。
- `components/home-link.bv` は、ヘッダーに置くトップページへのリンクです。
- `components/entry-list.bv` は記事の一覧で、トップページとタグ別ページが使います。
- `components/timestamp.bv` は日時を `<time>` 要素で表示します。
- `components/draft-badge.bv` は下書きに「draft」の印を付けます。記事一覧では日付の後に、記事ページではタイトルの後に付けます。
- `pages/not-found.bv` は `layout` ではなく `document` を使い、`noindex` を指定します。canonical・Open Graph・JSON-LDは出しません。
- `pages/entry.bv` は記事タイトルを `h1` で表示し、`created_at`・`updated_at`・`tags` を `キー: 値` の形で表示します。日時は日付だけを表示し、`<time>` 要素の `datetime` 属性に時刻と時差を含めます。
- `pages/home.bv` は記事一覧の後に、タグ一覧とRSSフィードへのリンクを表示します。
- faviconはPNGを先に指定し、SVG対応ブラウザー向けのSVGも併記します。

## 作成済みのサイト

genbitを更新しても、作成済みのサイトのビューは変更しません。新しい初期のビューの機能を使う場合は、自分でサイトへ反映してください。

### bitview 0.3 への移行

ビューの言語は、bitview 0.2からbitview 0.3に変わりました。bitview 0.2を使う版のgenbitで作ったサイトのビューは、そのままでは読み込めないので、次のように書き換えます。初期のビューを使っているなら、新しく `genbit new` で作ったサイトの `views/` に置き換え、自分で変えた部分を移すのが確実です。

- 構文を書き換えます。`fn badge(entry) => span(entry.title)` は `(defn badge [entry Entry] (span entry.title))`、`{href: "/"}` は `{:href "/"}`、`if c then x else y` は `(if c x y)`、`-- コメント` は `; コメント` です。リストとレコードの区切りの `,` は書きません。
- すべての引数に型を書きます（[型](#型)）。
- `views/pages/root.bv` を `home.bv`（関数 `home`）、`views/pages/page.bv` を `entry.bv`（関数 `entry`）に名前を変えます。CSSファイルも `home.css`・`entry.css` に変えます。
- `ctx.article` を `ctx.entry`、`ctx.canonical-url` を `ctx.search.url`、`ctx.json-ld` を `ctx.search.json-ld` に変えます。
- ほかのファイルから呼ばない関数は、`defn-` で定義できます。どのページからも呼ばれない関数がエラーになることはなくなりました。
