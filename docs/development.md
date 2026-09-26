# 開発

genbitの開発・検証はDocker Compose内で行います。ホストにRustツールチェーンを入れる必要はありません。

```sh
docker compose build cli
docker compose run --rm cli fmt --check
docker compose run --rm cli clippy --locked --all-targets --all-features -- -D warnings
docker compose run --rm cli test --locked --all-targets --all-features
```

CIはpull requestと `main` へのpushで動き、Ubuntuでfmt・Clippy・テストを、macOSでテストを実行します。PRを作っていないブランチへのpushではCIは動きません。依存のビルド結果は `main` の実行でキャッシュし、PRはそのキャッシュを使います。

Ubuntuのテストは[cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov)でカバレッジを計測しながら実行し、ファイルごとの要約をジョブのSummaryに出します。閾値は設けていないため、カバレッジが下がってもCIは失敗しません。`tests/cli.rs` から起動した `genbit` の子プロセスも計測に含まれますが、プロセスが正常に終了しないと結果が書き出されません。また、`src/*.rs` 内の `#[cfg(test)]` モジュールも集計に含まれます。手元で計測するには、コンテナ内にcargo-llvm-covを入れて実行します。

```sh
docker compose run --rm --entrypoint sh cli -c '
  rustup component add llvm-tools-preview &&
  cargo install cargo-llvm-cov --locked --version 0.9.1 &&
  cargo llvm-cov --locked --all-targets --all-features --summary-only'
```

依存は `.github/workflows/deny.yml` がcargo-denyで検査します。設定は `deny.toml` で、脆弱性などの勧告、許可していないライセンス、crates.io以外の取得元があると失敗します。勧告は依存を変えなくても後から出るため、週1回の定期実行でも検査します。影響がないと判断した勧告は、`deny.toml` の `ignore` に理由と一緒に追加してください。

Rustの版は `rust-toolchain.toml` で固定しています。版を上げるときは、`rust-toolchain.toml` の `channel`、`Cargo.toml` の `rust-version`、`Dockerfile` の `FROM rust:<版>-bookworm` を同時に変更してください。3か所が一致しないとCIが失敗します。Dependabotは `Dockerfile` だけを更新するため、その更新PRはCIで止まります。

リリースするときは、`Cargo.toml` の版を上げた変更を `main` にマージしてから、GitHubの **Actions > Release > Run workflow** で `main` を選んで実行します。ワークフローは版から `v<版>` のタグ名を決め、検査・ビルド・アーカイブの確認が通ればGitHub Releaseの下書きを作ります。下書きの内容を確認して公開すると、そのときにタグが作られます。同じ版のタグや下書きがすでにあると失敗します。`v*` タグは削除も付け替えもできないため、失敗しても版を上げ直す必要がないよう、ワークフローはタグを作りません。

リリースの実行中は、`.github/workflows` を変更するPRを `main` にマージしないでください。下書きの対象コミットと `main` の最新とでworkflowが異なると、GitHubはworkflowを変更する権限を求めるため、`GITHUB_TOKEN` では下書きを作れず `HTTP 403: Resource not accessible by integration` で失敗します。このときは失敗したジョブを再実行せず、`main` の最新でワークフローを実行し直してください。

配布アーカイブには、依存クレートの著作権表示とソースの入手先をまとめた `THIRD_PARTY_LICENSES.md` を同梱します。`scripts/third-party-licenses.sh` がcargo-aboutで生成し、内容は `about.toml` と `about.hbs` で決まります。依存を追加して `about.toml` の `accepted` にないライセンスが入ると生成が失敗します。`about.toml` の `accepted` と `deny.toml` の `allow` は同じ一覧にそろえてください。ずれていると `tests/license_lists.rs` のテストが失敗します。CIでも同じスクリプトと、release ビルドでの `scripts/smoke-test.sh` を実行するので、リリースの前に気づけます。

手元で一覧を生成するには、CIと同じ版のcargo-aboutを入れてから実行します。

```sh
cargo install cargo-about --locked --version 0.9.2
scripts/third-party-licenses.sh THIRD_PARTY_LICENSES.md
```

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
