# thither

cmd.exe / PowerShell / bash で同じ操作感のディレクトリ移動ツール。
数文字打って Enter で cd する。Rust + ratatui。

cmd と PowerShell を行き来しながら、フォルダ検索・ファイル名からの移動・履歴・階層探索を
同じ画面で使いたい人向け。開発中。

- フォルダ名を覚えている: `c openssl`
- ファイル名だけ分かる: `cf Cargo.toml` で、そのファイルの親フォルダへ移動
- 名前を知らず中を見たい: `c` を開き、Shift-Tab で browse に切り替える

`c`・`cf`・browse は本体だけで動く。履歴を使う `z <keywords>`・`zi`・recent には
別途 `zoxide` が必要（`z` の引数なしはホームへ移動）。fd・fzf は不要。
thither 経由で移動したフォルダは、zoxide が利用できる場合に記録する。

## 導入

### Windows: zip を展開するだけ

リリースの zip には thither.exe と、cmd.exe 用の `c.cmd` `cf.cmd` `z.cmd` `zi.cmd`、
PowerShell 用の `c.ps1` `cf.ps1` `z.ps1` `zi.ps1` が入っている。
PATH の通ったフォルダに展開すれば、cmd.exe でも PowerShell でも設定ファイルを触らずに使える。

- 展開したフォルダの中身がブロックされて .ps1 が動かないときは、そのフォルダで `Unblock-File *.ps1`
- PowerShell の実行ポリシーが Restricted のままだと .ps1 は動かない。`RemoteSigned` にするか、下の 1 行方式を使う
- `$PROFILE` で zoxide init している場合、`z` と `zi` は zoxide のエイリアスが優先される。
  thither の `zi` を使いたいときは下の 1 行方式にする(エイリアスを外してから定義する)

手元でビルドしたときは、同じ構成を自分で作れる。

```powershell
cargo build --release
thither init cmd --out C:\path\on\PATH
thither init powershell --out C:\path\on\PATH
```

### PowerShell: プロファイルに 1 行

`$PROFILE` の zoxide init より後に足す。

```powershell
Invoke-Expression (& C:\path\to\thither.exe init powershell | Out-String)
```

### bash / zsh: rc ファイルに 1 行

`~/.bashrc` か `~/.zshrc` の zoxide init より後に足す。

```bash
eval "$(thither init bash)"
```

## コマンド

| コマンド | 動作 |
|---|---|
| `c [query]` | カレント配下のディレクトリを選んで cd |
| `c -` | thither で移動する直前のフォルダへ戻る（繰り返すと往復） |
| `cf [query]` | ファイルを選んでその親ディレクトリに cd |
| `z <keywords>` | zoxide の履歴から一致する 1 件に cd(引数なしでホーム) |
| `zi [query]` | zoxide の履歴を一覧から選んで cd |

画面の中では Tab でモードが dirs → files → recent → favorites → browse と切り替わる。
browse は yazi 風に 1 階層ずつ歩く画面で、Right で入り、Left で親へ戻る。
初めて browse に切り替えるときは検索一覧の選択先を開く。
一度開いた browse はモードを往復しても現在地・選択・フィルタを保持する。
モード名を再度クリックしても現在地や選択はリセットしない。
F5 で現在の一覧とプレビューを更新できる。browse では絞り込みと選択位置を可能な限り保つ。
履歴の取得失敗は画面に理由を表示し、F5 で再試行、Tab でほかのモードへ移動できる。

本体だけ更新した場合も、シェル連携の変更を反映するには `thither init` を再実行する。
関数を使う PowerShell / bash では、初期化を読み直すか新しいシェルを開く。

### 戻る・お気に入り

browse 内では **Alt＋←で戻る、Alt＋→で進む**。親階層への移動とは異なり、訪問した場所の履歴を辿る。
戻った先のフィルタ・選択も復元する。戻った後に別の場所へ移動すると進む履歴を破棄する。
履歴は実行中のみ、各方向最大100件。削除済みの場所は飛ばす。
現在の入力ライブラリではマウスのサイドボタンを直接受け取れないため、マウスの設定ソフトで
戻るボタンに Alt＋←、進むボタンに Alt＋→を割り当てると利用できる。

`c -` は同じシェル内で `c`・`cf`・`z`・`zi` が成功したときの移動元に戻る。
キャンセル・失敗・同じ場所への移動では戻り先を更新しない。
通常の `cd` による移動は記録しない。戻り先がない場合や削除済みの場合は、理由を表示して移動しない。

画面内の **Ctrl-B** で、選択中のフォルダをお気に入りに登録・解除できる。
登録済みのフォルダには黄色の **★** を表示する。アイコン表示を無効にしていても表示され、
Ctrl-B で登録・解除するとすぐ切り替わる。別のシェルから変更した場合は F5 で反映する。
ファイルを選択している場合は、その親フォルダを登録する。
files モードの ★ は、そのファイルの親フォルダがお気に入りであることを示す。
Tab で **favorites** を開き、絞り込んで Enter で移動する。zoxide は不要。
削除済みのフォルダも一覧に残すので、Ctrl-B で解除できる。

コマンドからも管理できる（パス省略時は現在のフォルダ）。

```text
thither favorite add
thither favorite add "C:\work\my project"
thither favorite remove "C:\work\my project"
thither favorite list
```

保存先は設定ファイルと同じフォルダの `favorites.toml`。
複数のシェルから登録しても更新が失われないよう、保存処理を排他制御する。

## アイコン表示（任意）

端末のフォントに [Nerd Font](https://www.nerdfonts.com/) を指定している場合、
設定ファイルに次を追加すると、検索結果・browse・プレビューにアイコンを表示できる。

```toml
icons = true
```

設定先の優先順は `THITHER_CONFIG_DIR`、exe 隣の `config` フォルダ（存在する場合）、ユーザー設定フォルダ。
ユーザー設定フォルダの設定ファイルは Windows では `%APPDATA%\thither\config.toml`、Linux では
`~/.config/thither/config.toml`（`XDG_CONFIG_HOME` 設定時はその配下）、
macOS では `~/Library/Application Support/thither/config.toml`。
ファイルがなければ作成する。

既定は `false`。対応フォントの自動判定は行わないため、四角や文字化けが出る場合は
`icons = false` に戻す。フォントをインストールするだけでなく、端末側で選択する必要がある。
アイコンは表示専用で、検索や移動先のパスには影響しない。
Excel（xlsx / xlsm など）・CSV / TSV・Word・PowerPoint・PDF・設定ファイル・
データベース・画像・音声・動画・圧縮ファイルなどを種類別に表示する。
拡張子の大文字・小文字は区別せず、未対応の拡張子には汎用ファイルアイコンを使う。
アイコンは種類ごとに色分けする（表計算は緑、Word は青、PowerPoint はオレンジ、PDF は赤）。
ファイル名の色と検索一致部分の強調は維持する。

## マウス・Esc操作

一覧はマウスにも対応する（既定で有効）。項目をクリックすると選択し、一覧上のホイールで
3項目ずつ上下に移動する。上部のモード名もクリックで切り替えられる。
browse の中央列はクリックで選択し、左右の列のフォルダはクリックでそのフォルダへ移動する。
中央列のフォルダはダブルクリック（同じ位置・項目を500 ms以内に2回）または Right で中に入る。
検索欄の `[Filter: フォルダ名]` が絞り込み対象を示す。例えば C をダブルクリックしてから
`ssl` と入力すると C の中身を絞り込む。ファイルのダブルクリックでは外部アプリを起動しない。
左列の現在地の項目をクリックした場合は親フォルダへ戻る。
左右の列のファイルは、その親フォルダへ移動して選択する。ホイールは中央列が対象。
アクションメニューは項目をクリック、または Enter で実行する。ホイールで選択を上下に移動できる。
各列の項目を右クリックすると既定のアプリで開く。フォルダはファイルマネージャで開く。
Ctrl＋右クリックでは、その項目のアクションメニューを開く（Ctrl-P と同じ操作）。
左右の列でも現在のフォルダを移動せず、クリックした項目を対象にする。
検索画面の右側プレビューも右クリック・Ctrl＋右クリックに対応する。
プレビューを左クリックすると browse に切り替え、フォルダなら移動、ファイルなら親フォルダで選択する。
一覧・プレビューの左クリックではファイルを外部アプリで開かない。シェルの移動先の確定は Enter を使う。
端末標準の文字選択・ホイール操作を優先する場合は `config.toml` に `mouse = false` を指定して再起動する。

一覧画面では、フィルタ入力中の Esc はフィルタをクリアする。空の状態で Esc を押すと終了する。
Ctrl-C はフィルタの有無にかかわらず終了する。

## アクションメニュー

**Ctrl-P** で、選択中のファイル・フォルダに対する操作を開く。
文字入力で絞り込み、上下キーで選択、Enter で実行、Esc / Ctrl-P で元の画面に戻る。
Space は検索文字の入力に使う。メニューを開いた時点の対象に対して実行する。

設定なしで、ファイルマネージャで開く・VS Code で開く・パスのコピーが使える。
ファイルでは関連付けアプリで開く操作も表示する。VS Code は `code` が PATH に必要。
ファイルの **Open temporary copy (TEMP_)** は、OS の一時フォルダ内に毎回専用フォルダを作り、
`TEMP_元のファイル名` にコピーして既定のアプリで開く。コピー先は画面下部に表示する。
保存先は OS の一時フォルダ内の `thither-copies`。**Open temporary copies folder** で開き、
不要なコピーを確認して手動削除できる。このメニューはフォルダ選択中にも表示する。
以前の版が作った `thither-copy-*` は一時フォルダ直下に残る。開いているアプリへの影響を避けるため、自動移動しない。
一時コピーの上限は既定で **100 MiB**。`config.toml` の `temp_copy_max_mib = 100` で変更でき、
`0` で一時コピーを無効にする。実行時に設定を読み直す。
開始前にサイズを確認し、コピー中にファイルが大きくなっても上限を超えて書き込まない。
上限超過・コピー失敗時は途中のコピーを削除し、アプリは起動しない。フォルダのコピーは対象外。
原本や以前のコピーは上書きしない。thither 終了時も自動削除せず、原本への書き戻しも行わない。
残したい編集結果はアプリ側で「名前を付けて保存」する。一時フォルダは OS に削除される場合がある。
単一ファイルのコピーなので、相対リンクや関連ファイルに依存する文書は動作が変わる場合がある。
原本を他のアプリが更新している最中のコピーは、そのアプリで保存・更新を止めてから実行する。
Linux のパスのクリップボードコピーには Wayland で `wl-copy`、それ以外では `xclip` が必要。

独自のメニューを作るには次を実行する。既存の設定は上書きしない。

編集用のひな形は [examples/config/actions.json](examples/config/actions.json) に同梱している。
ポータブル版では `examples/config` を exe の隣へ `config` という名前でコピーすると使える。
開発時は `target/release/config` がコピー先になる。既存設定がある場合は上書きせず、必要な項目だけ追加する。
このひな形はバージョン管理する。実際に使う設定や個人のお気に入りはコミットしない。

```text
thither actions init
thither actions check
```

設定ファイルは `config.toml` と同じフォルダの **`actions.json`**。
ポータブル運用では exe と同じ場所に `config` フォルダを作成する。
その中の `config.toml`・`favorites.toml`・`actions.json` を使用するので、フォルダごと持ち運べる。
`config` フォルダがない場合は従来のユーザー設定フォルダを使う。既存設定の自動コピーは行わない。
保存先を分けたい場合は、環境変数 `THITHER_CONFIG_DIR` に設定フォルダを指定できる
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
保存後は `thither actions check` で確認し、Ctrl-P でメニューを開き直すと反映される。

`scripts/task.ps1` などの相対 `program` は設定フォルダ基準。
`.ps1` は PowerShell 7 の `pwsh -NoProfile -File` で実行する。
`program: "pwsh"` と `args: ["-NoProfile", "-File", "scripts/task.ps1", "{path}"]` でも指定できる。
Windows の `.cmd` / `.bat` も `program` に直接指定できる。
通常の `args` の相対パスは自動変換しないため、設定側のファイルを渡すときは `{config}/scripts/...` を使う
（PowerShell の `-File` 直後は設定フォルダ基準）。波括弧を文字として渡すときは `{{`・`}}` と書く。

対象パスは環境変数 `THITHER_TARGET`、対象フォルダは `THITHER_DIR`、設定フォルダは `THITHER_CONFIG` でも参照できる。
引数を1本のシェルコマンド文字列に結合しない。`cmd /c` や `pwsh -Command` のコードへ対象パスを埋め込む代わりに、
スクリプトファイルと引数、またはこれらの環境変数を使う。
`detach` は起動できたかまでを確認するので、実行結果やエラーを読みたい操作には `terminal` を使う。

## ネットワークドライブでの制限

Windows では、ネットワーク上の `dirs` / `files` 再帰走査を開始前にブロックする。
UNC パス、ネットワークドライブに割り当てられたドライブ文字、種類を確認できないドライブが対象。
ブロック時は Tab で `browse` などに切り替える。F5 でも制限は解除されない。
走査先ルートのリンク先も確認し、走査途中のディレクトリ再解析ポイント（ジャンクション等）は辿らない。
`browse` は使用できるが、一覧表示やプレビューのための通信は発生する。
Linux / macOS のネットワークマウントの自動判定には未対応。

## 開発時の検証

`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test` を実行する。
統合テストは制御された外部プロセスと実際のシェルで、移動・終了コード・環境の復元を検証する。
PowerShell 7（`pwsh`）と bash が必要。Windows では Git Bash を使う
（標準以外の場所にある場合は `THITHER_TEST_BASH` に実行ファイルのパスを設定）。

性能測定は `THITHER_BENCH_ROOT` に対象フォルダを指定し、次で再実行できる。

```text
cargo test --release --bin thither benchmark_local_tree -- --ignored --nocapture
```

2026-09-09 の Windows x86_64 / release ビルドでは、既定の除外設定で約13.7万ファイルを
87〜102 ms で走査し、`src` への検索更新は約5〜6 ms（3回測定）。
これは内部処理の測定で、プロセス起動から実端末への初回表示や入力遅延を保証する値ではない。
