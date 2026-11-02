# PCB Clearance

Altium / KiCad の RS-274X ガーバーを渡すと、銅箔の絶縁ギャップが **0.1 mm以下**の位置と画像を自動出力する Python アプリです。CAD、本体基板ファイル、エージェント、オンライン接続は実行時に不要です。

## GUI（推奨）

操作の要点は [簡易マニュアル（A4・1枚）](docs/PCB-Clearance_簡易マニュアル.pdf) にまとめています。

Windows 64bit 用の `dist/PCB-Clearance.exe` をダブルクリックしてください。exe だけを任意のフォルダーへコピーして使えます。利用するPCへの Python・CAD・アプリのインストールは不要です。

1. 「ファイルを追加…」でガーバーを選択します。複数選択や「フォルダーを追加…」も使えます。
2. クリアランスを指定します。既定は **0.1 mm以下**です。
3. 「検査開始」を押します。計算中も画面は応答し、完了した層を表示します。
4. 完了すると既定のブラウザーで HTML レポートを開きます。「結果を開く」から再表示もできます。

結果は既定で **exe の隣の `results/clearance-日時/`** に保存します。GUI の「結果の保存先」から変更できます。書き込み可能な場所へ exe を置くか、保存先を指定してください。検査中はアプリを閉じず、完了を待ってください。

HTML の表示は従来と同じです。違反がある層だけを表示し、全体図の番号付きマークと、拡大画像を含む結果表を出力します。

ソースから GUI を起動する場合は `python pcb_clearance_gui.py` を実行します。この場合の既定の保存先はプロジェクトの `output/` です。

## exe の再ビルド（開発用）

Windows 64bit と Python 3.11 以上で実行します。

```powershell
python -m pip install -r requirements-build.txt
.\build_exe.ps1
```

`dist/PCB-Clearance.exe` が生成されます。単一ファイルに Python・Tk・検査ライブラリを含めます。起動時に OS の一時フォルダーへ内部ファイルを展開し、終了時に削除します。`build/` と `dist/` は Git 管理対象外です。

## CLI（開発・バッチ処理用）

Python 3.11 以上を使います。最初にこのフォルダーで依存ライブラリをインストールしてください。

```powershell
python -m pip install -r requirements.txt
```

フォルダーを指定するだけで銅箔層を選び、測定から画像・レポートの生成まで実行します。

```powershell
python check_gerber_clearance.py 'D:\dev\mounting\suc-600\MIN\PCB_DATA(MIN-778-03)\geb'
```

出力先を指定する例です。既存の結果との混在を避けるため、新規または空のフォルダーを指定してください。省略時は実行ディレクトリの `output/clearance-日時/` に保存します。

```powershell
python check_gerber_clearance.py 'D:\dev\mounting\suc-600\MIN\PCB_DATA(MIN-778-03)\geb' --min-clearance 0.1 --outdir '.\output\MIN-778-03-check'
```

ファイルの直接指定、複数指定、クォートしたワイルドカードにも対応します。

```powershell
python check_gerber_clearance.py 'D:\gerber\board.GTL' 'D:\gerber\board.GBL'
python check_gerber_clearance.py 'D:\gerber\*Cu*.gbr'
python -m pcb_clearance 'D:\gerber'
```

任意で `python -m pip install -e .` を実行すると `pcb-clearance` コマンドも使えます。

## 出力

| ファイル | 内容 |
| --- | --- |
| `index.html` | 違反がある層の番号付き位置マークを示した全体図と、拡大画像を含む結果表。検査エラーも表示。ローカルブラウザーで閲覧可能 |
| `violations.csv` | 層名、番号、ギャップmm、座標、対向する両端点、分類、画像ファイル名。UTF-8 BOM付き |
| `report.json` | 上記に加え、処理状態、しきい値、設定、層の属性、除外数、処理時間、検査エラー |
| `*-overview.png` | 各層の銅箔全体と検出位置 |
| `*-0001.png` など | 各箇所の実形状、測定線、座標、幅、スケール |

座標は **ガーバー原点のmm、X右向き・Y上向き**です。画像は裏面も上面から透視した方向で描きます。Altium の `Board_Origin` コメントや KiCad の配置原点から CAD 固有座標への自動補正は行いません。

JSON の `layers[].violations[]` の例:

```json
{
  "id": 1,
  "gap_mm": 0.08,
  "x_mm": 10.5,
  "y_mm": 20.5,
  "point_a_mm": [10.46, 20.5],
  "point_b_mm": [10.54, 20.5],
  "kind": "self_approach",
  "image": "board.GTL-xxxxxxxx-0001.png"
}
```

`kind` は `self_approach`（同じ連結銅箔の自己接近）または `separate_islands`（別の連結銅箔）です。ネット情報がないガーバーでも動作します。電位や短絡の有無をこの分類から推定しません。

終了コードは **0:検出なし、1:検出あり、2:入力・検査・出力エラー**です。エラーと検出が混在する場合は2を返します。バッチ処理では `report.json` の `status` も確認できます。

## 対象層と対応形式

- X2 `TF.FileFunction,Copper,...` を最優先します。通常の属性と Altium の `G04 #@!` コメント属性の両方を読みます。
- 属性がない場合は Altium の `.GTL` / `.GBL` / `.G1` など / `.GP1` など、KiCad の `F_Cu` / `B_Cu` / `In1_Cu` などを自動選択します。
- レジスト・シルク・ペースト・機械層・ドリルは対象外。選択から外したファイルはレポートに記録します。フォルダー内の未知の層名は自動選択しないため、必要ならファイルを直接指定してください。
- フォルダーの直下を検査します。サブフォルダーは再帰検索しません。ZIPは事前に展開してください。
- 正極性・負極性、`LPD`/`LPC` の順序付き合成、`IPPOS`/`IPNEG`、Altium のサーマルマクロを扱います。
- `TF.FilePolarity,Negative` は画像を「銅箔のない領域」と解釈します。`IPNEG` の画像反転とは別に適用します。
- 極性属性も IP 指定もない `.GPn` は Altium のネガプレーンとして扱い、その仮定をレポートに残します。`--polarity positive` / `negative` で明示指定できます。
- mm / inch、絶対座標、先行ゼロ・末尾ゼロ省略、モーダル D 操作、直線、単象限・多象限円弧、フラッシュ、リージョンと cut-in による穴に対応します。
- アパーチャは `C/R/O/P`、標準アパーチャの穴、マクロの円・アウトライン・正多角形・線・矩形・サーマルと変数演算に対応します。`LM/LR/LS` も扱います。
- ストロークは穴なし円形または矩形を扱います。矩形は固定姿勢のまま掃引し、線端を丸めません。
- ステップリピート、ブロックアパーチャ、モアレ、旧式のオフセット等の未対応命令は、黙って無視せずその層をエラーにします。他層の処理とレポートは継続します。

## 判定と限界

既存の `check-gerber-clearance` スキルのアパーチャ・円弧処理と、絶縁側を向く輪郭辺ペアの測定方式を流用しています。新しいパーサーで描画順と極性を保持し、配列演算と空間インデックスで候補を絞ります。層は独立したプロセスで並列計算します。

既存方式の平行辺の最短点選択も修正しました。閉端を持つスリットでは、辺の端を選ぶと銅箔の境界上になって見逃すため、平行辺の重なる区間の中央で測定します。

このアプリの判定は既存スキルの「未満」から **以下**へ変更しています。浮動小数点比較では `gap <= threshold + tolerance` とし、既定の `tolerance` は `0.0000001 mm` です。これは数値比較の許容差であり、円弧近似の誤差を補償する値ではありません。

注意点:

- 円・円弧は1/4円64分割で近似します。円の最大径方向誤差は `r × (1 − cos(π/256))`（rは半径mm）です。両側が円弧の場合は双方の近似誤差の影響があり、0.1 mm近傍の合否を厳密な真円として保証するものではありません。
- 鋭角コーナーの誤検出を避けるため、同じ輪郭上の周回距離が `gap × 6 + 0.05 mm` 未満の候補を既定で除外します。短い本物のスリットを除外する可能性があります。`--no-arc-filter` で無効化できます。
- 既定では半径0.3 mm内の同じ島ペアの測定点をまとめます。表示するのは最小ギャップを持つ実測点です。座標を平均化しません。`sample_count` と `sample_bounds_mm` に集約情報を残します。
- 銅箔の重なり・既に接続している形状の短絡、零幅の接触、銅箔の線幅、基板外形までの距離は検査しません。
- ネガ層は無限の銅箔背景を余白付き矩形で代用し、その人工外周を測定から除きます。ネガ層の面積と全体図の外周は基板外形を表しません。
- 画像・判定は幾何処理で自動生成します。エージェントの目視判断を実行時の前提にしません。製造上の意図を理解した DRC の代替ではありません。

## オプション

| オプション | 既定値 | 内容 |
| --- | --- | --- |
| `--min-clearance` / `--threshold` | `0.1` | この値以下のギャップを報告、mm |
| `--outdir` | 日時付きフォルダー | 新規または空の出力先 |
| `--jobs` / `-j` | 最大4 | 並列層数。1で逐次 |
| `--polarity` | `auto` | `auto / positive / negative` |
| `--no-arc-filter` | 無効 | コーナー除外を停止 |
| `--cluster-radius` | `0.3` | 検出点を集約する半径、mm |
| `--window-mm` | `2.0` | 拡大画像の基本範囲、mm |
| `--tolerance` | `0.0000001` | 数値比較の許容差、mm |

## テスト

```powershell
python -B -m unittest discover -s tests -v
```

既知寸法の合成ガーバーで0.1 mmの境界、mm/inch、描画・ファイル極性、サーマル、モーダル座標、矩形ストローク、KiCadの穴、自己接近、コーナー除外、未対応命令、並列CLI、終了コード、CSV/JSON/PNG生成を検証します。テスト用ファイルはOS一時ディレクトリへ生成し、自動削除します。

## 由来と仕様

- 流用元: `D:\dev\pcb\sl-u52\agent-skills\shared\check-gerber-clearance\scripts\gerber_clearance.py`。元スキルと元基板データは変更していません。
- 実装時に参照した仕様: [Ucamco Gerber Layer Format Specification 2026.05](https://www.ucamco.com/files/downloads/file_en/554/gerber-layer-format-specification-revision-2026-05_en.pdf)。特に領域の輪郭、サーマル、オブジェクト極性、ファイル極性を確認しています。
