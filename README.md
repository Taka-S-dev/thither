# tadoru

**tadoru**（辿る）は、cmd.exe / PowerShell / bash で同じ操作感のディレクトリ移動ツール。
数文字打って Enter で目的地へ辿り着き、階層を 1 段ずつ辿ることもできる。Rust + ratatui。

- フォルダ名を覚えている: `c openssl`
- ファイル名だけ分かる: `cf Cargo.toml` で、そのファイルの親フォルダへ移動
- 名前を知らず中を見たい: `c` を開き、Shift-Tab で browse に切り替える

`c`・`cf`・browse・favorites は本体だけで動く。fd・fzf は不要。
履歴を使う `z`・`zi`・recent には `zoxide` が要る（無い場合は画面で案内する）。

## 導入

```text
tadoru setup
```

書き込む内容と対象ファイルを表示してから確認を求める。`--yes` で確認を省ける。
シェルは環境から判定する。追記するのは目印で囲んだ 1 ブロックだけで、再実行しても増えない。
やめるときは目印から目印までを消す。

cmd.exe には起動時に読むファイルが無いので、`setup` の対象外。PATH の通ったフォルダに置く。
リリースの zip には exe と 8 本のシムが入っているので、展開するだけでよい。

```text
tadoru init cmd --out C:\path\on\PATH
```

手作業で設定したい場合や、`init` が出す内容そのものは
[導入の詳細](docs/guide/setup.md)を参照。

## コマンド

| コマンド | 動作 |
|---|---|
| `c [query]` | カレント配下のディレクトリを選んで cd |
| `cf [query]` | ファイルを選んでその親ディレクトリに cd |
| `z <keywords>` | zoxide の履歴から一致する 1 件に cd（引数なしでホーム） |
| `zi [query]` | zoxide の履歴を一覧から選んで cd |
| `c -` | 直前に居たフォルダへ戻る（繰り返すと往復） |

## 画面

Tab でモードが dirs → files → recent → favorites → browse と切り替わる。
どのモードでも、文字を打てば絞り込み、Enter でそこへ cd、Esc で終了する。

| キー | 動作 |
|---|---|
| Tab / Shift-Tab | モード切替 |
| Ctrl-B | 選択中のフォルダをお気に入りに登録・解除 |
| Ctrl-P | 選択中の項目に対するアクションメニュー |
| F5 | 一覧とプレビューを更新 |
| Right / Left | browse で階層を下る・上がる |
| Alt+← / Alt+→ | browse の訪問履歴を戻る・進む |

browse は Miller columns。左が親、中央が今の階層、右が選択先の中身。
Finder の列表示や ranger、yazi と同じ並べ方で、cd 専用なのでファイル操作は持たない。
上枠のモード名と、その下のパスはクリックできる。パスは階層名を押すとそこへ移動する。

くわしくは[画面と操作](docs/guide/screen.md)と[アクションメニュー](docs/guide/actions.md)。

## 設定

設定ファイルは任意で、無ければ既定値で動く。自動では作らない。雛形が要るときは次を実行する。

```text
tadoru config init
```

アイコン表示、マウスの有効・無効、一時コピーの上限、走査から外すフォルダを指定できる。
置き場所と各項目は[画面と操作](docs/guide/screen.md)を参照。

## 開発

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

統合テストは実際のシェルで移動・終了コード・環境の復元を検証する。
PowerShell 7（`pwsh`）と bash が要る。Windows では Git Bash を使う
（標準以外の場所にある場合は `TADORU_TEST_BASH` に実行ファイルのパスを設定）。

性能測定は対象フォルダを指定して再実行できる。

```text
cargo test --release --bin tadoru benchmark_local_tree -- --ignored --nocapture
```

2026-09-10 の Windows x86_64 / release ビルドでは、既定の除外設定で約13.7万ファイルを
94〜100 ms で走査し、`src` への検索更新は約5〜7 ms（3回測定）。
これは内部処理の測定で、プロセス起動から実端末への初回表示や入力遅延を保証する値ではない。
画面は変化があったときだけ描き直すので、開いたまま放置しても CPU を使い続けない。
