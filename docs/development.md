# 開発

genbitの開発・検証はDocker Compose内で行います。ホストにRustツールチェーンを入れる必要はありません。

```sh
docker compose build cli
docker compose run --rm cli fmt --check
docker compose run --rm cli clippy --locked --all-targets --all-features -- -D warnings
docker compose run --rm cli test --locked --all-targets --all-features
```

CIはpull requestと `main` へのpushで動き、Ubuntuでfmt・Clippy・テストを、macOSでテストを実行します。PRを作っていないブランチへのpushではCIは動きません。依存のビルド結果は `main` の実行でキャッシュし、PRはそのキャッシュを使います。

Rustの版は `rust-toolchain.toml` で固定しています。版を上げるときは、`rust-toolchain.toml` の `channel`、`Cargo.toml` の `rust-version`、`Dockerfile` の `FROM rust:<版>-bookworm` を同時に変更してください。3か所が一致しないとCIが失敗します。Dependabotは `Dockerfile` だけを更新するため、その更新PRはCIで止まります。

`v*` タグをpushするとリリース用のワークフローが動き、バイナリをビルドしてGitHub Releaseの下書きを作ります。

## Dockerでサイトをプレビューする

`SITE_DIR` にはサイトの絶対パスを指定してください。

```sh
SITE_DIR=/absolute/path/to/my-blog
docker compose run --rm \
  --publish 127.0.0.1:3000:3000 \
  --volume "$SITE_DIR:/site" \
  --workdir /site \
  cli run --locked --manifest-path /workspace/Cargo.toml -- dev --host 0.0.0.0
```

ホストのブラウザーで `http://127.0.0.1:3000` を開きます。

macOSのDocker Desktopでは、bind mount越しのファイル削除イベントがコンテナに届かない場合があります（[docker/for-mac#7246](https://github.com/docker/for-mac/issues/7246)）。削除したページが残った場合は、別のコンテナでビルドし、ブラウザーを再読み込みしてください。

```sh
docker compose run --rm \
  --volume "$SITE_DIR:/site" \
  --workdir /site \
  cli run --locked --manifest-path /workspace/Cargo.toml -- build
```

ファイルの作成・編集とSSEによる再読み込みは、この環境でも動作します。
