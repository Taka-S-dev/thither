# 導入の詳細

`tadoru setup` で足りる場合は [README](../../README.md) だけ読めばよい。
ここは手作業で設定したい場合と、うまく動かない場合の資料。

## setup が書く内容

目印で囲んだ 1 ブロックを、シェルの起動ファイルに追記する。

```text
# >>> tadoru >>>
Invoke-Expression (& C:\path\to\tadoru.exe init powershell | Out-String)
# <<< tadoru <<<
```

再実行しても増えない。内容が変わっていれば差し替える。既に同じなら何もしない。
やめるときは目印から目印までを消す。書き込みは一時ファイルを経由するので、
途中で中断しても起動ファイルが壊れた状態で残らない。

シェルは環境から判定する。明示したいときは `tadoru setup powershell` のように指定する。

## 手動で設定する

dotfiles をバージョン管理している、共有マシンで設定を書き換えられない、
zoxide との読み込み順を自分で決めたい。そういう場合は自分で書く。

`z` と `zi` は zoxide がエイリアスとして定義する。エイリアスは関数より優先されるため、
tadoru の行は **zoxide init より後**に置く必要がある。

| シェル | 書くファイル | 書く内容 |
|---|---|---|
| PowerShell | `$PROFILE` | `Invoke-Expression (& C:\path\to\tadoru.exe init powershell \| Out-String)` |
| bash / zsh | `~/.bashrc` `~/.zshrc` | `eval "$(tadoru init bash)"` |
| cmd.exe | なし | `tadoru init cmd --out <PATH の通ったフォルダ>` |

`tadoru init <shell>` は初期化コードを標準出力に書くだけで、ファイルには触らない。
どのファイルに、どの順序で入れるかは利用者が決めることなので、`init` は判断しない。

## PowerShell の .ps1 を PATH に置く方法

プロファイルを触らずに済ませたい場合、`c.ps1` などを PATH に置く手もある。

```text
tadoru init powershell --out C:\path\on\PATH
```

制約が 2 つある。zoxide を使っていると `z` と `zi` はエイリアスが優先されるので、
この方法では置き換えられない。実行ポリシーが Restricted だと .ps1 は動かない。
zip から展開した直後はブロック属性が付くことがあるので、
そのフォルダで `Unblock-File *.ps1` を実行する。

## 更新したとき

本体を差し替えただけならそのまま動く。シムは隣の実行ファイルを先に探す。
シェル連携の内容そのものが変わった場合は `tadoru setup` か `tadoru init` を実行し直し、
新しいシェルを開くか初期化を読み直す。

## zoxide との関係

履歴を使う `z`・`zi`・recent は zoxide の記録を読む。無い環境では、
入れ方と代わりの手段を画面に表示する。`c`・`cf`・browse・favorites は本体だけで動く。

履歴を自前で持たないのは、zoxide がシェルの cd フックですべての移動を記録しているため。
tadoru が自前で持つと tadoru 経由の移動しか残らず、既存の履歴も捨てさせることになる。
tadoru 経由で移動したフォルダは、zoxide が使える場合に `zoxide add` で記録する。
