# 仕様

## コマンド

### `navkit pick`

候補を TUI で絞り込み、選ばれたパスを標準出力に 1 行書いて終了する。

| オプション | 既定 | 意味 |
|---|---|---|
| `--mode dirs\|files\|recent` | dirs | dirs: カレント配下のディレクトリ / files: ファイル(選ぶとその親ディレクトリを出力) / recent: zoxide の履歴 |
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
| powershell | 関数 `c` `cf` `z` `zi` の定義。zoxide の同名エイリアスを先に外す。選択後に `zoxide add` |
| cmd | `c.cmd` `cf.cmd` `z.cmd` `zi.cmd` の内容(`--out <dir>` でファイル書き出し)。引数を `NAVKIT_QUERY` に入れて本体を呼ぶ |
| bash | 同名の関数定義(将来) |

シムの動作は 3 つとも同じ: 本体を呼ぶ → 出力があれば cd → `zoxide add` → 終了コードを返す。
`z` だけは本体を呼ばず `zoxide query -- <keywords>` の結果に cd する(引数なしならホーム)。

## 画面(pick)

```
┌ [dirs] C:\Users\takay\folder\work\C ─────────────── 7861 ┐
│ > ope█                                                   │
│   openssl-1.1.1q                                         │
│   Libcurl/curl                                           │
│   ...                                                    │
└ Tab: mode  Enter: cd  Esc: cancel ───────────────────────┘
```

| キー | 動作 |
|---|---|
| 文字入力 | 絞り込み(nucleo、大小無視、スペース区切りで AND) |
| Up/Down, Ctrl-K/Ctrl-J | 候補移動 |
| Enter | 決定 |
| Esc, Ctrl-C | キャンセル(終了コード 1) |
| Tab | モード切替 dirs → files → recent → dirs(マイルストーン 4) |

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
