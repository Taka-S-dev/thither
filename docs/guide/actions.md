# アクションメニュー

選択中のファイルやフォルダを、別のアプリやスクリプトに渡すための画面。
tadoru 自体はコピー・削除・名前変更を持たない。渡すところまでが役目。

**Ctrl-P** で、選択中のファイル・フォルダに対する操作を開く。
文字入力で絞り込み、上下キーで選択、Enter で実行、Esc / Ctrl-P で元の画面に戻る。
Space は検索文字の入力に使う。メニューを開いた時点の対象に対して実行する。

設定なしで、ファイルマネージャで開く・VS Code で開く・パスのコピーが使える。
ファイルでは関連付けアプリで開く操作も表示する。VS Code は `code` が PATH に必要。
ファイルの **Open temporary copy (TEMP_)** は、OS の一時フォルダ内に毎回専用フォルダを作り、
`TEMP_元のファイル名` にコピーして既定のアプリで開く。コピー先は画面下部に表示する。
保存先は OS の一時フォルダ内の `tadoru-copies`。**Open temporary copies folder** で開き、
不要なコピーを確認して手動削除できる。このメニューはフォルダ選択中にも表示する。
以前の版が作った `tadoru-copy-*` は一時フォルダ直下に残る。開いているアプリへの影響を避けるため、自動移動しない。
一時コピーの上限は既定で **100 MiB**。`config.toml` の `temp_copy_max_mib = 100` で変更でき、
`0` で一時コピーを無効にする。実行時に設定を読み直す。
開始前にサイズを確認し、コピー中にファイルが大きくなっても上限を超えて書き込まない。
上限超過・コピー失敗時は途中のコピーを削除し、アプリは起動しない。フォルダのコピーは対象外。
原本や以前のコピーは上書きしない。tadoru 終了時も自動削除せず、原本への書き戻しも行わない。
残したい編集結果はアプリ側で「名前を付けて保存」する。一時フォルダは OS に削除される場合がある。
単一ファイルのコピーなので、相対リンクや関連ファイルに依存する文書は動作が変わる場合がある。
原本を他のアプリが更新している最中のコピーは、そのアプリで保存・更新を止めてから実行する。
Linux のパスのクリップボードコピーには Wayland で `wl-copy`、それ以外では `xclip` が必要。

独自のメニューを作るには次を実行する。既存の設定は上書きしない。

編集用のひな形は [examples/config/actions.json](../../examples/config/actions.json) に同梱している。
ポータブル版では `examples/config` を exe の隣へ `config` という名前でコピーすると使える。
開発時は `target/release/config` がコピー先になる。既存設定がある場合は上書きせず、必要な項目だけ追加する。
このひな形はバージョン管理する。実際に使う設定や個人のお気に入りはコミットしない。

```text
tadoru actions init
tadoru actions check
```

設定ファイルは `config.toml` と同じフォルダの **`actions.json`**。
ポータブル運用では exe と同じ場所に `config` フォルダを作成する。
その中の `config.toml`・`favorites.toml`・`actions.json` を使用するので、フォルダごと持ち運べる。
`config` フォルダがない場合は従来のユーザー設定フォルダを使う。既存設定の自動コピーは行わない。
保存先を分けたい場合は、環境変数 `TADORU_CONFIG_DIR` に設定フォルダを指定できる
（最優先。`config.toml`・`favorites.toml`・`actions.json` に共通）。
自作スクリプトは `config/scripts` に置き、`program` を `scripts/my-tool.bat` のような相対パスにすると持ち運びやすい。
お気に入りの登録先やコマンド内に書いた絶対パスは、別の PC へ移す際に見直す必要がある。
メニューを開くたびに読み直す。JSON の誤りは画面に表示し、既定の操作と移動機能は使い続けられる。
選択したプロジェクト内の設定を自動実行・自動読込することはない。

```json
{
  "version": 1,
  "include_defaults": true,
  "actions": [
    {
      "name": "VS Codeで開く",
      "program": "code",
      "args": ["{path}"],
      "target": "any",
      "run": "detach"
    },
    {
      "name": "自作スクリプト",
      "program": "scripts/task.ps1",
      "args": ["{path}"],
      "target": "directory",
      "run": "terminal"
    }
  ]
}
```

| 設定 | 意味 |
|---|---|
| `version` | 設定形式のバージョン。現在は `1` を指定 |
| `name` | メニューに表示する名前。必須 |
| `program` | 実行ファイルまたはスクリプト。引数は含めない |
| `args` | 引数の配列。`{path}` は対象、`{dir}` は対象のフォルダ（ファイルなら親）、`{config}` は設定フォルダ |
| `target` | `any`・`file`・`directory`。既定は `any` |
| `run` | `terminal` は画面を一時的に閉じ、結果と終了コードを確認して Enter / Esc で戻る。`detach` は出力を表示せずバックグラウンド起動。既定は `terminal` |
| `cwd` | 作業フォルダ。既定は `{dir}`。相対パスは設定フォルダ基準 |
| `include_defaults` | `false` にすると自作アクションだけを表示。既定は `true` |

追加するときは `actions` 配列内の `{ ... }` を1つコピーし、名前・プログラム・引数を変更する。
項目の間はカンマで区切り、最後の項目の後にはカンマを付けない。JSON にはコメントを書けない。
文字列はダブルクォートで囲む。Windows パスは `C:/tools/task.bat` または
`C:\\tools\\task.bat` と書く。`program` と `args` は分け、引数は1個ずつ配列に入れる。
保存後は `tadoru actions check` で確認し、Ctrl-P でメニューを開き直すと反映される。

`scripts/task.ps1` などの相対 `program` は設定フォルダ基準。
`.ps1` は PowerShell 7 の `pwsh -NoProfile -File` で実行する。
`program: "pwsh"` と `args: ["-NoProfile", "-File", "scripts/task.ps1", "{path}"]` でも指定できる。
Windows の `.cmd` / `.bat` も `program` に直接指定できる。
通常の `args` の相対パスは自動変換しないため、設定側のファイルを渡すときは `{config}/scripts/...` を使う
（PowerShell の `-File` 直後は設定フォルダ基準）。波括弧を文字として渡すときは `{{`・`}}` と書く。

対象パスは環境変数 `TADORU_TARGET`、対象フォルダは `TADORU_DIR`、設定フォルダは `TADORU_CONFIG` でも参照できる。
引数を1本のシェルコマンド文字列に結合しない。`cmd /c` や `pwsh -Command` のコードへ対象パスを埋め込む代わりに、
スクリプトファイルと引数、またはこれらの環境変数を使う。
`detach` は起動できたかまでを確認するので、実行結果やエラーを読みたい操作には `terminal` を使う。
