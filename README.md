<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/logo-dark.svg">
    <img src="docs/assets/logo-light.svg" alt="genbit" width="300">
  </picture>
</p>

<p align="center">
  Markdownのブログを、そのまま配信できる静的ファイルにする小さなブログコンパイラ。<br>
  バイナリ1つで、<code>new</code>・<code>build</code>・<code>dev</code> だけ。
</p>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshot-dark.png">
    <img src="docs/assets/screenshot-light.png" alt="genbitで生成したサイトの記事一覧と記事ページ" width="860">
  </picture>
</p>

> [!NOTE]
> genbitは作者が自分の[ブログ](https://memo.nnenn0.com/)のために作っている個人用のツールです。設定・フロントマター・テンプレート変数・URL・生成物の構成は、どの版でも互換性なく変わる可能性があります。移行手順の提供やサポート、機能要望への対応はしません。使う場合はバージョンを固定してください。

## なぜgenbitか

genbitは、汎用の静的サイトジェネレーターを目指していません。ブログの記事を書き、手元で確かめ、静的ファイルとして公開するのに要るものだけを持ち、その範囲で誤りを公開前に止めることを重視しています。

### 覚えることが少ない

コマンドは `new`・`build`・`dev` の3つです。記事はTOMLフロントマター付きのMarkdownで書き、ページの見た目は `views/` のテンプレートとCSSで決めます。設定は `config.toml` の5項目だけで、テーマ、プラグイン、設定を重ねる仕組みはありません。テンプレートは[bitview](https://github.com/nnenn0/bitview)で書きます。bitviewはHTMLを関数で組み立てる小さい言語で、値は文字列、真偽値、リスト、レコード、HTMLの5種類しかありません。

### 生成物が小さい

生成するページはJavaScriptを含みません。テンプレートからはスクリプトを書けず、記事の本文にもHTMLを直接書けないので、後から紛れ込むこともありません。CSSは、そのページが使う部品の分だけをインライン展開し、HTMLとともに圧縮します。本文の画像には `width`・`height` 属性と内容のハッシュ付きのURLを付けるので、読み込みでレイアウトがずれず、長期間キャッシュしても差し替えが反映されます。

### 小さいが厳しい

genbitは、公開前に見つけられる誤りをビルドの失敗にします。

- テンプレートは、どのページを描画するよりも前に、genbitが渡す変数の型で検査します。存在しない項目、属性の綴りの誤り、`p` の中の `div` のようにHTMLで置けない入れ子、どのページからも使われない関数は、下書きのときにしか通らない分岐の中にあっても、位置付きのエラーになります。
- 記事の本文のサイト内リンクと画像は、参照先がなければ失敗します。本文とテンプレートに書けるURLのスキームは `http:`・`https:`・`mailto:` と相対URLだけです。
- 出力パスの衝突と、シンボリックリンクの入力を拒否します。
- ビルドは一時領域で行い、成功したときだけ `dist/` を入れ替えます。`genbit build --dry-run` は同じ検査をすべて行い、`dist/` を変更しません。

## genbitがやらないこと

小さい責務と小さい生成物を保つため、次の機能は持ちません。必要なら、ビルドの前後に外部のツールと組み合わせてください。

- ブラウザーで動くJavaScript（アクセス解析、コメント欄、サイト内検索など）
- シンタックスハイライト
- テーマの管理、プラグイン、CMS
- タグ以外の分類、多言語のサイト
- 画像の縮小・形式変換・圧縮
- ホスティングサービスへのデプロイ

## 特徴

- **ライブリロード** — `genbit dev` は保存のたびに再ビルドし、SSEでブラウザーを再読み込みします。再ビルドに失敗しても、最後に成功したサイトを配信し続けます。
- **下書き** — `drafts/` の記事は `genbit dev` だけで印付きでプレビューし、`genbit build` の出力には含めません。
- **部品ごとのHTMLとCSS** — テンプレートの部品ごとに、HTMLとCSSを並べて置きます。初期テーマはJavaScriptなしでOSのライト/ダーク設定に追従します。
- **メタデータとRSSの生成** — canonicalリンク、`sitemap.xml`、`robots.txt`、Open Graph、JSON-LD、RSS 2.0フィードを生成します。
- **記事ごとのディレクトリ** — 記事は `index.md` と画像などの素材を1つのディレクトリにまとめ、`/entries/hello-world/` のようなURLで配信します。本文から素材を相対パスで参照できます。
- **タグ・見出しリンク** — タグページ、リンク付きの見出し、コードブロックの言語ラベルに対応します。

## インストール

[GitHub Releases](https://github.com/nnenn0/genbit/releases)でビルド済みバイナリを配布しています。

| プラットフォーム | ターゲット |
| --- | --- |
| Linux x86_64（静的リンク） | `x86_64-unknown-linux-musl` |
| macOS Apple Silicon | `aarch64-apple-darwin` |

```sh
VERSION=$(curl -fsSLI -o /dev/null -w '%{url_effective}' https://github.com/nnenn0/genbit/releases/latest | sed 's|.*/||')
TARGET=aarch64-apple-darwin # Linuxでは x86_64-unknown-linux-musl
curl -fsSLO "https://github.com/nnenn0/genbit/releases/download/$VERSION/genbit-$VERSION-$TARGET.tar.gz"
curl -fsSLO "https://github.com/nnenn0/genbit/releases/download/$VERSION/SHA256SUMS"
shasum -a 256 --check --ignore-missing SHA256SUMS
tar -xzf "genbit-$VERSION-$TARGET.tar.gz"
install "genbit-$VERSION-$TARGET/genbit" ~/.local/bin/
```

`VERSION` には最新のリリースのタグ（`v` で始まる版）が入ります。特定の版を入れるときは直接指定してください。`~/.local/bin` は `PATH` に含まれる任意のディレクトリに置き換えてください。アーカイブには、genbitが含む依存クレートのライセンスをまとめた `THIRD_PARTY_LICENSES.md` も入っています。

<details>
<summary>Artifact Attestationsの検証、macOSのGatekeeper、ソースからのビルド</summary>

各アーカイブには[GitHub Artifact Attestations](https://docs.github.com/actions/security-for-github-actions/using-artifact-attestations)を付けています。GitHub CLIがあれば、このリポジトリのワークフローでビルドされたことを確認できます。

```sh
gh attestation verify "genbit-$VERSION-$TARGET.tar.gz" --repo nnenn0/genbit
```

macOSのバイナリは署名・公証していません。ブラウザーでダウンロードして実行を拒否された場合は、`com.apple.quarantine` 属性を外してください。

```sh
xattr -d com.apple.quarantine genbit
```

Rust 1.98.1のCargoが使える環境では、ソースからもインストールできます。

```sh
cargo install --locked --git https://github.com/nnenn0/genbit --tag "$VERSION"
```

</details>

## クイックスタート

```sh
genbit new my-blog
cd my-blog
genbit dev      # http://127.0.0.1:3000
```

`config.toml` を編集し、`content/` の下にMarkdownを追加します。公開するときは次を実行します。

```sh
genbit build    # dist/ にサイトを生成
```

`dist/` の中身を静的ホスティングサービスへ配置してください。公開先は `/entries/hello-world/` に `entries/hello-world/index.html` を返し、`404.html` をHTTP 404で返せる必要があります。

## コマンド

| コマンド | 内容 |
| --- | --- |
| `genbit new <name>` | 設定・ビュー・サンプル記事を含む新しいサイトのディレクトリを作ります。名前にはASCII英数字・`-`・`_` を使え、先頭は英数字にします。既存のパスは上書きしません。 |
| `genbit build` | カレントディレクトリのサイトを `dist/` へビルドします。記事内のサイト内リンクの参照先がなければ失敗します。 |
| `genbit build --dry-run` | ビルドと同じ検査をすべて行い、`dist/` は変更しません。 |
| `genbit dev [--host <ip>] [--port <port>]` | 下書きを含めてビルドして配信し（既定は `127.0.0.1:3000`）、変更があれば再ビルドして再読み込みします。`dist/` は変更しません。 |

## サイトの構成

```text
my-blog/
├── .gitignore           # dist/ を除外
├── config.toml          # title, description, site_url, og_image, timezone
├── content/             # 記事のディレクトリ → /entries/hello-world/
│   └── entries/hello-world/index.md   # 画像などの素材も同じディレクトリに置く
├── drafts/              # 下書き。content/ と同じ構成で、genbit dev だけが読む
├── views/
│   ├── pages/           # ページごとのbitviewとCSS: root, page, tags, tag, not-found
│   └── components/      # ページが使う部品のbitviewとCSS: document, layout, entry-list など
└── static/              # そのままコピー（assets/img/ に複数の記事で使う画像、assets/site/ にfavicon・OGP画像）
```

```markdown
+++
title = "最初の記事"
description = "記事で扱う内容を簡潔に説明します。"
created_at = 2026-09-26 09:00
updated_at = 2026-09-26 09:00
tags = ["rust", "web"]
+++

ここに本文を書きます。
```

## ドキュメント

| ドキュメント | 内容 |
| --- | --- |
| [設定](docs/configuration.md) | `config.toml` の項目とURLの規則 |
| [コンテンツ](docs/content.md) | フロントマター、タグ、記事URL、Markdownの処理 |
| [ビュー](docs/views.md) | ページと部品の構成、テンプレート変数、CSSの合成、初期テーマ |
| [出力と公開](docs/output.md) | 生成ファイル、`dist/` の保護、公開先の条件、開発サーバー |
| [開発](docs/development.md) | Dockerを使ったgenbit自体の開発 |

## 既知の制限

- 公開先はドメインのルートに限ります。サブパス（例: `https://example.com/blog/`）への配置には対応していません。
- 初期テンプレートとサンプル記事は日本語で、`<html lang="ja">` を出力します。他の言語で使う場合は `views/components/document.bv` を編集してください。
- AVIFとSVGの画像には寸法属性を付けません。本文の画像はPNG・JPEG・GIF・WebPで用意してください（[docs/content.md](docs/content.md#画像)）。
- ビルド済みバイナリはLinux x86_64とmacOS Apple Siliconだけです。Windowsには対応していません。

`dist/` の入れ替えに関する例外的なケースは[出力と公開](docs/output.md#既知の制限)を参照してください。

## ライセンス

ライセンスは[MIT License](LICENSE)です。genbitのコードと、同梱する初期テンプレート・CSS・サンプル記事・favicon・OGP画像に適用します。`genbit new` でサイトへコピーされた初期素材を再配布するときも、著作権表示とライセンス文を残してください。利用者が自分で書いた記事や追加した素材には、このライセンスを適用しません。外部のRust依存関係には、それぞれのライセンスが適用されます。
