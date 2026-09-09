# 名前

**2026-09-09 に thither に決めた。**「その場所へ」という意味の英語で、cd がやることと向きが一致する。
crates.io / GitHub / winget のいずれでも実質空きで、口頭で言っても他の語と紛れない。
仮称の navkit から、crate 名・exe 名・環境変数(`THITHER_QUERY`)・設定フォルダ名まで置き換えた。
残る作業はリポジトリのフォルダ名と、手元の導入先フォルダ名。どちらも動作には影響しない。

以下は決めるまでの経過。同じ検討を繰り返さないために残す。

決めるときの条件:

- 打ちやすい 2〜4 文字のシム名(`c` `cf` `z` `zi`)とは別に、本体 exe の名前が要る
- crates.io / GitHub / winget で既存と衝突しない
- 英単語として意味が通り、cd や jump を連想できる
- 社名・実名・職場を連想させない

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

## navkit を正式名にしない理由

navkit は 2026-09-09 にバッチ版を置いたフォルダ名(navigation kit の略)がそのまま残ったもの。
仮称として使ってきたが、正式名には向かない。

- TomTom が自動車向けナビゲーションエンジンを NavKit の名前で売っている。分野は違うが商標として先行する
- GitHub に同名リポジトリが 3 つある(iOS のナビゲーションバー、Hitman の MOD ツール、ナビゲーションライブラリ)
- kit は道具箱の意味で、複数のものをまとめた印象を与える。実体は 1 つのコマンドなので合わない

## 候補 2 巡目(同じ日に同じ方法で実測)

| 候補 | 意味 | crates.io | GitHub の同名 | 判断 |
|---|---|---|---|---|
| stile | 柵を越える踏み段 | 空き | 9 star のみ | style の打ち間違いに見える。却下 |
| guidepost | 分かれ道の道標 | 空き | 21 star のみ | 無難だが平凡で長い。次点 |
| trailhead | 登山道の入口 | 空き | Salesforce の学習サービスが有名 | 却下 |
| doorway | 戸口 | 空き | iOS のアニメーション 204 star | SEO の doorway page を連想する。却下 |
| thence / hence | その場所から | 空き | - | from の意味なので cd と向きが逆。却下 |
| cairn / blaze / quay / haven / helm / beacon / buoy / fathom / sextant / threshold / milestone / signpost | 航海・道標系 | 使用済み | - | 却下 |

2 巡目で thither を上回るものは出なかった。次点は guidepost。
