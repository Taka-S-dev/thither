# thither

cmd.exe / PowerShell / bash で同じ操作感のディレクトリ移動ツール。
数文字打って Enter で cd する。Rust + ratatui。

開発中。設計は [docs/design.md](docs/design.md)、仕様は [docs/spec.md](docs/spec.md)。

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
| `cf [query]` | ファイルを選んでその親ディレクトリに cd |
| `z <keywords>` | zoxide の履歴から一致する 1 件に cd(引数なしでホーム) |
| `zi [query]` | zoxide の履歴を一覧から選んで cd |

画面の中では Tab でモードが dirs → files → recent → browse と切り替わる。
browse は yazi 風に 1 階層ずつ歩く画面で、Right で入り、Left で親へ戻る。
