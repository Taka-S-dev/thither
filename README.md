# navkit (仮称)

cmd.exe / PowerShell / bash で同じ操作感のディレクトリ移動ツール。
数文字打って Enter で cd する。Rust + ratatui。

開発中。設計は [docs/design.md](docs/design.md)、仕様は [docs/spec.md](docs/spec.md)。

## 使い方(開発版)

```powershell
cargo build --release
```

PowerShell: `$PROFILE` の zoxide init より後に 1 行足す。

```powershell
Invoke-Expression (& C:\path\to\navkit.exe init powershell | Out-String)
```

bash / zsh: `~/.bashrc` か `~/.zshrc` の zoxide init より後に 1 行足す。

```bash
eval "$(navkit init bash)"
```

cmd.exe: PATH の通ったフォルダに c.cmd cf.cmd z.cmd zi.cmd を書き出す。
リリースの zip にはこの 4 本が同梱されているので、zip を PATH の通ったフォルダに展開するだけでよい。

```
navkit init cmd --out C:\path\on\PATH
```

| コマンド | 動作 |
|---|---|
| `c [query]` | カレント配下のディレクトリを選んで cd |
| `cf [query]` | ファイルを選んでその親ディレクトリに cd |
| `z <keywords>` | zoxide の履歴から一致する 1 件に cd(引数なしでホーム) |
| `zi [query]` | zoxide の履歴を一覧から選んで cd |
