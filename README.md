# PCB Clearance

Altium / KiCad のガーバーデータから、銅箔間の狭いクリアランスを検出するツールです。
位置と拡大画像を HTML で確認できます。

既定では **0.1 mm未満**を検出します。**0.1 mmちょうどは違反に含めません。**

## 使い方

[Windows版exeをダウンロード](https://github.com/koba164/pcb_clearance/releases/latest/download/PCB-Clearance.exe)

Windows 64bit 用の `PCB-Clearance.exe` を実行します。インストールや Python・CADソフトの準備は不要です。

1. 「ファイルを追加」「フォルダーを追加」「ZIPを追加」でガーバーを選択します。
2. 表示された検査対象の一覧を確認します。不要なファイルは「選択から外す」で除外できます。
3. クリアランスを指定し、「検査開始」を押します。
4. 完了すると、ブラウザーで HTML レポートが開きます。

フォルダー指定では、下位フォルダーも含めて銅箔ガーバーを自動選択します。フォルダー内のZIPも検索します。
ZIPは展開せずに追加でき、ZIP内の下位フォルダーも検索します。リストではZIPの内部パスを確認でき、個別に除外できます。
無圧縮・Deflate圧縮に対応します。ZIP内にさらに入っているZIPは検索しません。暗号化ZIPには対応していません。

[簡易マニュアル（A4・1枚）](docs/PCB-Clearance_簡易マニュアル.pdf)

## 検査結果

- 違反がある層の全体図に、番号付きで位置を表示します。
- 結果表で、各箇所の拡大画像・座標・ギャップ幅を確認できます。
- 座標一覧の CSV と、機械処理用の JSON も出力します。

結果は既定で exe の隣の `results/clearance-日時/` に保存されます。保存先は GUI で変更できます。
結果を共有するときは、画像を含む **結果フォルダーごと**渡してください。

## 注意点

- 座標はガーバー原点基準の mm 表記です。CADの表示座標とは異なる場合があります。
- 検査対象は絶縁ギャップです。既に接続している銅箔の短絡や、線幅は検査しません。
- 円弧近似やコーナー除外により、測定誤差や短いスリットの見逃しが生じる場合があります。
- Python 版から形状演算ライブラリを変更したため、等幅のギャップが連続する箇所では、近傍集約後の代表座標や件数が変わる場合があります。
- 形状演算は 0.0000000001 mm の固定格子を使用し、絶対座標 ±100000 mm 以内を扱います。範囲外は検査エラーになります。
- 未対応のガーバー命令などで検査できない層は、エラーとして表示します。

## ソースから実行

Rust stable（edition 2024 対応）を使用します。[rustup](https://rustup.rs/) で導入してください。
Windows の既定の MSVC ツールチェーンでは Visual Studio Build Tools の「C++ によるデスクトップ開発」が必要です。
検査処理・GUI・画像生成は Rust で実行し、Python は不要です。

```powershell
cargo run --locked --bin pcb-clearance-gui
```

exe をビルドする場合は、Windows 上で次を実行します。

```powershell
.\build_exe.ps1
```

GUI の `dist/PCB-Clearance.exe` と CLI の `dist/pcb-clearance-cli.exe` が生成されます。

CLI から検査する例:

```powershell
cargo run --release --locked --bin pcb-clearance -- "D:\gerber" --min-clearance 0.1 --outdir "D:\results\board"
```

出力先には新規または空のフォルダーを指定します。終了コードは、違反なし `0`、違反あり `1`、入力・出力または層の検査エラー `2` です。
`--jobs`、`--polarity auto|positive|negative`、`--no-arc-filter`、`--cluster-radius`、`--window-mm`、`--tolerance` を指定できます。
`--threshold` は `--min-clearance` の別名です。GUI を含めずにビルドする場合は `--no-default-features` を追加してください。

検証:

```powershell
cargo test --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
```

層ごとの検査は Rust のスレッドで並列実行します（既定は最大4層）。GUI はバックグラウンド検査の完了通知を受け取り、完了した層数だけ進捗バーを進めます。
