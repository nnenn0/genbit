# テンプレートとCSS

ページは `templates/` の[Tera](https://keats.github.io/tera/)テンプレートで描画します。`genbit new` が作ったテンプレートとCSSは自由に編集できます。

## テンプレート

| テンプレート | 描画するもの | 必須 |
| --- | --- | --- |
| `root.html` | トップページ `/` | 必須 |
| `page.html` | 記事（フロントマターの `template` の既定値） | 全記事が別のテンプレートを指定していなければ必須 |
| `tags.html` | タグ一覧 `/tags/` | 必須 |
| `tag.html` | タグ別ページ `/tags/{tag}/` | 必須 |
| `404.html` | `dist/404.html` | 必須 |
| `base.html` | 他のテンプレートが継承する共通レイアウト | 初期テンプレートが使用 |
| `entry-list.html` | `root.html` と `tag.html` が読み込む記事一覧 | 初期テンプレートが使用 |

`templates/` 以下のサブディレクトリのテンプレートは、`/` で区切った名前（例: `entries/page.html`）で参照します。ファイル名・ディレクトリ名にバックスラッシュ（`\`）は使えず、ビルドエラーになります。

## 変数

全テンプレートに次の変数を渡します。

| 変数 | 内容 |
| --- | --- |
| `site.title` | `config.toml` の `title` |
| `site.description` | `config.toml` の `description` |
| `site.site_url` | 正規化した `site_url` |
| `site.og_image` | 絶対URLに解決した `og_image` |
| `css` | このテンプレート用のCSS。[CSS](#css)を参照。 |

トップページ・記事・タグ一覧・タグ別ページには、`description`、`canonical_url`、`json_ld` も渡します。タグ一覧とタグ別ページの `description` は固定の日本語の文（`記事のタグ一覧`、`{tag} の記事一覧`）で、`json_ld` はサイトの `WebSite` の構造化データです。

| テンプレート | 追加の変数 |
| --- | --- |
| `root.html` | `entries`: 全記事（作成日時の新しい順） |
| `page.html` などの記事テンプレート | `article`: 記事、`content`: MarkdownをHTMLに変換した本文 |
| `tags.html` | `tags`: `name`、`url`、`count` の配列 |
| `tag.html` | `tag`: タグ名、`entries`: そのタグの記事（作成日時の新しい順） |
| `404.html` | `site` と `css` のみ |

`entries` の各要素と `article` には、`title`、`description`、`url`、`created_at`、`updated_at`、`tags` が入ります。`created_at` と `updated_at` は次の値を持つオブジェクトです。

| 値 | 形式 | 例 |
| --- | --- | --- |
| `datetime` | `config.toml` の `timezone` の時差を付けたRFC 3339の日時 | `2026-09-17T09:00:00+09:00` |
| `date` | `YYYY-MM-DD` | `2026-09-17` |
| `time` | `HH:MM` | `09:00` |

`<time>` 要素の `datetime` 属性には `datetime`、画面の表示には `date` や `time` を使います（例: `<time datetime="{{ article.created_at.datetime }}">{{ article.created_at.date }}</time>`）。

本文のローカルPNG・JPEG・GIF・WebPには `width`・`height` 属性が付きます（[画像](content.md#画像)）。初期の `styles/common.css` は `img { display: block; max-width: 100%; height: auto; margin-inline: auto; }` で縦横比を保ち、1枚ずつ縦に中央寄せで配置します。独自のCSSでも縦横比を保つために `height: auto` を指定してください。

記事の本文は `{{ content | safe }}` で挿入します。本文は自分で書いたMarkdownから生成するので、信頼できる入力として扱います。

記事のMarkdownにはHTMLを直接書けません（[HTMLの禁止](content.md#htmlの禁止)）。`content` に含まれるHTMLは、genbitがMarkdownから生成したものだけです。テンプレートにはこの制限がなく、HTMLを自由に書けます。

## 初期テンプレート

- `base.html` はタイトル、meta description、canonicalリンク、Open Graph、JSON-LD、favicon、RSSの自動検出用リンクを出力します。子テンプレートはこのブロックを上書きします。
- `404.html` は `metadata` ブロックを上書きして `noindex` を指定し、canonical・Open Graph・JSON-LDを出しません。
- `page.html` は記事タイトルを `h1` で表示し、`created_at`・`updated_at`・`tags` を `キー: 値` の形で表示します。日時は日付だけを表示し、`<time>` 要素の `datetime` 属性に時刻と時差を含めます。
- `root.html` は記事一覧の後に、タグ一覧とRSSフィードへのリンクを表示します。
- `entry-list.html` は `entries` を描画します。マーカーを消して項目の間隔をそろえる `.unmarked-list` と、日付を表示する `.entry-list time` のスタイルは `common.css` にあります。`.unmarked-list` は `tags.html` のタグ一覧でも使います。
- faviconはPNGを先に指定し、SVG対応ブラウザー向けのSVGも併記します。

## CSS

CSSは各HTMLへインライン展開し、HTMLとともに圧縮します。

- `styles/common.css` は必須で、全ページに適用します。
- テンプレートと同じ相対パスで拡張子を `.css` に替えたファイルを置くと、そのテンプレートを使うページにだけ追加します。たとえば `styles/page.css` は `templates/page.html` で描画するページに適用します。これらのファイルは任意です。

初期テーマは落ち着いた淡色の背景、狭めの本文幅、広めの行間を使い、`prefers-color-scheme` でOSのダークモード設定に追従します。色は `common.css` の `:root` でCSS変数として定義し、ダークモードの値もそこでまとめて切り替えます。手動の切り替えやJavaScriptはありません。

## 作成済みのサイト

genbitを更新しても、作成済みのサイトのテンプレートとCSSは変更しません。新しい初期テンプレートの機能を使う場合は、自分でサイトへ反映してください。
