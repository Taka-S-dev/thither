# 仕様

## コマンド

### `navkit pick`

候補を TUI で絞り込み、選ばれたパスを標準出力に 1 行書いて終了する。

| オプション | 既定 | 意味 |
|---|---|---|
| `--mode dirs\|files\|recent\|browse` | dirs | dirs: カレント配下のディレクトリ / files: ファイル(選ぶとその親ディレクトリを出力) / recent: zoxide の履歴 / browse: 1 階層ずつ歩く(下記) |
| `--query <文字列>` | 空 | 初期絞り込み。環境変数 `NAVKIT_QUERY` があればそれを優先(cmd のシム用) |
| `--root <パス>` | カレント | 走査の起点 |
| `--select-1` | off | 候補が 1 つなら画面を出さず即出力 |

終了コード: 0 = 選択して出力した / 1 = キャンセル(Esc, Ctrl-C)または候補なし / 2 = 引数や環境のエラー

除外(dirs / files): `.git` `node_modules` `dist` `build` `target` と設定ファイルの `exclude`。
.gitignore は無視する(fd の `--no-ignore` 相当)。ビルド成果物も移動先になりうるため。

### `navkit init <shell>`

シェル用のシム定義を標準出力に書く。

| shell | 出力 |
|---|---|
| powershell | 関数 `c` `cf` `z` `zi` の定義。zoxide の同名エイリアスを先に外す。選択後に `zoxide add`。`--out <dir>` なら `c.ps1` `cf.ps1` `z.ps1` `zi.ps1` をファイルで書き出す(PATH に置けばプロファイル不要。ただし zoxide のエイリアスが `z` `zi` に優先する) |
| cmd | `c.cmd` `cf.cmd` `z.cmd` `zi.cmd` の内容(`--out <dir>` でファイル書き出し)。引数を `NAVKIT_QUERY` に入れて本体を呼ぶ |
| bash | 同名の関数定義(zsh でも同じ)。`eval "$(navkit init bash)"` で読む。`--out` は無い(関数でないと cd できない) |

`--out` で書いた .cmd と .ps1 は、まず自分と同じフォルダの navkit.exe を使い、無ければ生成時の exe を使う。
リリースの zip はこの 8 本と exe を同梱する。

シムの動作は 3 つとも同じ: 本体を呼ぶ → 出力があれば cd → `zoxide add` → 終了コードを返す。
`z` だけは本体を呼ばず `zoxide query -- <keywords>` の結果に cd する(引数なしならホーム)。

## 画面(pick)

```
╭──────────────────────────────────────────────────────────────╮╭ openssl-1.1.1q ──────╮
│ > ope█                                                       ││ apps\                │
│   12/7861 ───────────────────────────────────────────────────││ crypto\              │
│ [dirs|files|recent] C:\Users\takay\folder\work\C             ││ CHANGES              │
│ ▌ openssl-1.1.1q                                             ││ ...                  │
│   Libcurl\curl                                               ││                      │
╰──────────────────────── Tab: mode  Enter: cd  Esc: cancel ───╯╰──────────────────────╯
```

fzf の `--height 40% --layout=reverse --border` と同じ見た目を狙う。カーソル行から画面下端までを
インラインで使い、上のコマンド履歴は残す。開いたばかりのウィンドウなら全面、プロンプトが下のほうに
あるときは 40%(最低 12 行)ぶんまで履歴をスクロールさせて確保する。終了時は描いた範囲を消してプロンプトに戻る。
上から順にプロンプト、件数(走査中はスピナー付き)と罫線、ヘッダ(モードと起点)、候補。
色は fzf の既定テーマの番号をそのまま使う(枠 240、プロンプト 110、件数 144、ヘッダ 109、
ポインタ 161、一致文字 108、選択行は 255 on 236 の太字)。

左がタブ付きの候補一覧、右が選択中のディレクトリの中身(ディレクトリを先に、末尾に区切り付き)。
files モードでは親ディレクトリの中身を出す。幅 80 桁未満なら右ペインは出さない。

| キー | 動作 |
|---|---|
| 文字入力 | 絞り込み(nucleo、大小無視、スペース区切りで AND) |
| Up/Down, Ctrl-K/Ctrl-J | 候補移動 |
| Enter | 決定 |
| Esc, Ctrl-C | キャンセル(終了コード 1) |
| Tab / Shift-Tab | モード切替 dirs → files → recent → browse → dirs(Shift-Tab は逆順)。クエリは引き継ぎ、各モードの走査結果は保持 |

### browse モード

yazi と同じ 3 列(親 | 今の階層 | 選択先の中身)で 1 階層ずつ歩く。「名前を知らないので見て回りたい」ときの画面。
絞り込みの画面と起点を共有する: dirs / files / recent で選んでいたフォルダから歩き始め、
browse で降りた場所が次に Tab で戻ったときの起点になる。

```
╭──────────────────────────────────────────────────────────────────────────────╮
│ > █                                                                          │
│   8/8 ───────────────────────────────────────────────────────────────────────│
│ [dirs|files|recent|browse] C:\Users\takay\folder\work\C\openssl-1.1.1q       │
│   Libcurl\        │ ▌ apps\                     │  aes\                      │
│   linux\          │   crypto\                   │  bn\                       │
│ ▌ openssl-1.1.1q\ │   doc\                      │  build.info                │
│   postgres\       │   CHANGES                   │                            │
╰────────────────────── Left: up  Right: enter  Tab: mode  Enter: cd  Esc: cancel ╯
```

| キー | 動作 |
|---|---|
| 文字入力 | 今の階層の中だけを絞り込み(nucleo、大小無視)。階層を移ると消える |
| Right, Ctrl-L | 選択中のフォルダに入る(ファイルなら何もしない) |
| Left, Ctrl-H | 親へ。出てきたフォルダを選択した状態に戻す |
| Backspace | 絞り込みがあれば 1 文字消す。無ければ親へ(yazi と同じ) |
| Up/Down, Ctrl-K/Ctrl-J, PageUp/PageDown | 候補移動 |
| Enter | 選択中のフォルダに cd。ファイルを選んでいれば今の階層に cd |

1 階層で読むのは最大 20,000 件。それ以上あるフォルダは切り詰める。

表示は起点からの相対パスで、区切りは OS のもの。一致した文字は色付き(fzf と同じ)。出力は絶対パスで末尾区切りなし。

## 設定ファイル

`%APPDATA%\navkit\config.toml`(Windows)/ `~/.config/navkit/config.toml`。無くても動く。

```toml
# 走査から外すディレクトリ名(どの階層でも)
exclude = [".git", "node_modules", "dist", "build", "target"]
# プロジェクトルートとみなさない親フォルダ(絶対パス)。機械固有なので設定ファイル側に置く
exclude_roots = ["C:/Users/takay/folder/work/C"]
```

## テスト観点

- `^` `&` `|` `%` `!` を含むクエリが cmd / PowerShell 両方のシム経由で本体に届く
- 空白と日本語を含むパス(例: `C:\Users\takay\OneDrive\ドキュメント`)で cd できる
- 候補ゼロで Enter しても落ちない、Esc で 1 を返す
- 14 万ファイルのツリーで走査完了前から絞り込みが効く
