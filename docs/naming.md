# 名前

正式名は未定。crate 名・exe 名・リポジトリ名は最後にまとめて変える(Cargo.toml の `name` と docs 内の表記)。

決めるときの条件:

- 打ちやすい 2〜4 文字のシム名(`c` `cf` `z` `zi`)とは別に、本体 exe の名前が要る
- crates.io / GitHub / winget で既存と衝突しない
- 英単語として意味が通り、cd や jump を連想できる
- 社名・実名・職場を連想させない

候補はここに追記していく。

## 候補(2026-09-09 に crates.io / GitHub / winget を実測)

crates.io は API の 404 を空きとみなした。GitHub は同名検索の最上位、winget は該当なしを空きとした。

| 候補 | 意味 | crates.io | GitHub の同名 | winget | 判断 |
|---|---|---|---|---|---|
| thither | その場所へ | 空き | 4 star の旅行アプリのみ | 空き | 本命 |
| whither | どの場所へ(疑問) | 空き | 10 star の停止プロジェクト | 空き | 対抗 |
| yonder | あそこ | 空き | 136 star の R パッケージ | 空き | 惜しい |
| jaunt | 小旅行 | 空き | 135 star の言語処理系(停止) | 空き | 惜しい |
| alight | 降り立つ | 空き | PHP フレームワーク 89 star | 空き | 却下 |
| landfall | 上陸 | 空き | ゲーム会社 Landfall が有名 | 空き | 却下 |
| moor | 停泊する | 空き | 1154 star のページャ | 空き | 却下 |
| wend | 道を行く | 空き | 検索が中国語の wenda に埋もれる | 空き | 却下 |
| beeline / hop / skip / stride / roam / berth / bearing / waypoint / warp / portal | - | 使用済み | - | - | 却下 |

thither を本命にする理由:

- 「その場所へ」がそのまま cd の意味。hither and thither で英語として通る
- 3 つのレジストリすべてで実質空き。検索しても他のツールに埋もれない
- whither は音が weather / whether と紛れる。口頭で伝えるときに毎回確認が要る
- yonder と jaunt は既存プロジェクトの規模が中途半端に大きく、検索で並んでしまう

シム名(`c` `cf` `z` `zi`)は名前を変えても据え置く。exe 名を打つのは init の 1 行だけなので、
長さより「意味が通ること」と「衝突しないこと」を優先した。
