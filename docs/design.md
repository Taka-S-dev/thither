# 設計書

対象: シェル横断のディレクトリ移動ツール(仮称 navkit。正式名は未定、docs/naming.md 参照)。
このファイルは「なぜこの形にしたか」を残す。手順や操作は docs/spec.md。

## 目的

「今どこへ行きたいか」を数文字打って Enter するだけで cd できる道具を、
cmd.exe / PowerShell 7 / (将来) bash で同じ操作感にする。

現状の代替:

- PowerShell プロファイルの `c` / `cf`(fd + fzf の関数)と zoxide の `z` / `zi`
- `C:\Users\takay\navkit\`: cmd 用バッチ 4 本 + PowerShell アダプタ(2026-09-09 作成)
- yazi(ファイルマネージャ。汎用すぎるので専用品を作る)

## 制約と、それを踏まえた構造

### 子プロセスは親シェルのカレントディレクトリを変えられない

どの言語で書いても本体 exe 単体では cd できない。zoxide と同じく
「本体は選んだパスを標準出力に書く」「各シェル用の数行のシムが受け取って cd する」の二層にする。

```
navkit pick [--mode dirs|files|recent] [--query Q]  → 選択パスを stdout(1 行)、未選択なら exit 1
navkit init cmd|powershell|bash                     → そのシェル用のシム定義を stdout
```

シムの利用側:

- PowerShell: `Invoke-Expression (& navkit init powershell | Out-String)` を $PROFILE に 1 行
- cmd: `navkit init cmd` が出力する .cmd 群を PATH 上のフォルダに置く(cmd に関数は無い)
- bash/zsh: `eval "$(navkit init bash)"`

### cmd.exe はコマンドライン引数を壊す(2026-09-09 に実測)

- `call` は引数中の `^` を再解釈して消す。`^^` と書いても `for /f ('...')` の子 cmd がもう一度食う
- `for /f ('...')` 内で exe をフルパス + 引用符で呼ぶと、引用符が 3 つ以上になった時点で先頭と末尾だけ剥がされて壊れる
- 対策: 本体はクエリを引数ではなく環境変数(`NAVKIT_QUERY`)でも受け取れるようにする。cmd のシムは引数を環境変数に入れてから本体を呼ぶ

### エイリアスは関数より優先される(PowerShell)

zoxide init が `z` / `zi` をエイリアスとして定義するため、同名の関数を後から定義しても呼ばれない。
シムは `Remove-Alias` してから関数を定義する。

### fd は Windows でディレクトリ末尾に `\` を付ける

`fd -t d -a` の出力は `C:\...\dir\`。`"...\"` を引数に渡すと MSVC 系のコマンドライン解析で `\"` がエスケープ扱いになる。
本体は自前走査(ignore クレート)なので fd には依存しないが、パスを出力するときは末尾の区切りを付けない。

### zoxide との関係

- 履歴の主は zoxide のままにする(PowerShell の cd フックで全移動が記録される)
- 本体の `recent` モードは `zoxide query -l`(スコア順の一覧)を読む。DB 形式を直接読まない
- cd した先を `zoxide add` するのはシム側の仕事

## 構成要素(Rust)

| 役割 | クレート | 選定理由 |
|---|---|---|
| CLI | clap | 定番 |
| 走査 | ignore | fd / ripgrep の走査エンジン。並列、.gitignore 対応、除外指定 |
| あいまい一致 | nucleo | Helix の絞り込みエンジン。入力に追従する非同期マッチ |
| TUI | ratatui + crossterm | Windows コンソール対応。yazi と同じ組み合わせ |
| 設定 | directories + toml | 除外ディレクトリなど機微な設定を機械ごとに持つ |

UI は最初から自前(fzf を呼ばない)。理由: 将来「履歴 / 配下フォルダ / ファイル」を
1 画面でタブ切り替えし、右にプレビューを出す形にしたいが、fzf では 1 本のリストしか扱えない。

## マイルストーン

1. `pick --mode dirs`: カレント配下のディレクトリを絞って選ぶ。fzf の `c` と同じ体感になるまで詰める
2. `init powershell` / `init cmd`: シム出力。C:\Users\takay\navkit のバッチ版を置き換える
3. `--mode files`(cf 相当)、`--mode recent`(zoxide 一覧)
4. 1 画面統合: Tab でモード切替、右ペインに選択中ディレクトリの中身
5. bash/zsh 対応、リリースビルド(GitHub Releases に exe)

## 性能の目標値(2026-09-09 実測ベース)

`C:\Users\takay\folder\work\C`(ディレクトリ 7,861、ファイル 142,949、linux カーネル含む)で:

- 走査: fd は約 0.1 秒、`dir /s /b` は約 0.5〜1.0 秒。ignore クレートで fd 同等を狙う
- 絞り込み: 1 文字入力ごとに 16 ms 以内で再描画(nucleo の非同期マッチで走査完了を待たない)
- 起動: プロセス開始から画面表示まで 50 ms 以内

## 関連する既存設定

- PowerShell プロファイル: `C:\Users\takay\OneDrive\ドキュメント\PowerShell\Microsoft.PowerShell_profile.ps1`
  (fd/fzf/rg の `fs` 系ツール、zoxide init、navkit.ps1 の読み込み、WezTerm 向け OSC 7 出力)
- WezTerm 設定: https://github.com/Taka-S-dev/wezterm-config(`~/.wezterm.lua` はシンボリックリンク)
- Neovim 設定: https://github.com/Taka-S-dev/nvim-config
- バッチ版: `C:\Users\takay\navkit\`(c.cmd cf.cmd z.cmd zi.cmd tools.cmd navkit.ps1、bin\ に fd/fzf/zoxide の exe)
