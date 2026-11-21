# BatteryBar

Windows タスクバー（通知領域の左）に、**選択したワイヤレス機器のうち残量が閾値以下になったものだけ**を横一列で警告表示する、純 Rust・完全オフラインの軽量常駐ツールです。

## 特徴
- **低下時のみ表示（既定）**: 選んだ機器の残量が十分なときはタスクバー横一列を自動で隠し、システムトレイの小さなアイコンだけ残します。
- **機器ごとの選択・個別閾値**: 検出された機器の一覧から監視対象を ON/OFF し、全体閾値（15〜50%）または機器ごとの個別閾値をメニューから設定できます。
- **軽量・単一バイナリ**: Rust + Win32 API のみ（約 380 KB / Private メモリ約 2 MB、管理者権限不要）。
- **完全オフライン設計**: ネットワーク通信・自動更新・テレメトリなし。外部実行ファイルの同梱やプロセス注入も行いません。

## 対応するバッテリー取得元
- **Bluetooth 機器全般**: Windows の「Bluetooth とデバイス」で残量を取得できる BLE / Classic Bluetooth 機器（キーボード・マウス・イヤホン・ヘッドホン等）
- **Logitech ワイヤレス機器**: HID++ 2.0 対応の LIGHTSPEED / Bolt / Unifying / G ヘッドセット（読み取り専用の独自 Software ID で既存ユーティリティと共存）
- **標準 HID バッテリー対応機器**: HID Battery Usage (`0x06:0x20` / `0x85:0x65`) 対応の周辺機器（ペンタブレット等の Digitizer は誤動作防止のため除外）
- **Xbox / XInput コントローラー**: `xinput1_4.dll` 経由で無線コントローラーを検出
- **Windows スマートフォン連携（Phone Link）**: ローカル状態ファイルの読み取り専用パース（iPhone / Android、日本語・英語対応）
- **PC 本体 / UPS バッテリー**: ノート PC や UPS 接続時のみ自動検出

## 使い方
1. [Releases](https://github.com/Exerea/battery-bar/releases) から `battery-bar.exe` をダウンロードして起動します（インストール不要）。
2. 右下のシステムトレイに出るバッテリーアイコンをクリックし、検出された機器から監視したいものを ON にします。
3. 必要に応じて各機器の個別閾値・全体閾値（既定 30% 以下）・Windows 起動時の自動起動を切り替えます。
   - 見た目確認用: メニューの「選択機器を常にタスクバーに表示（確認用）」または `battery-bar.exe --always`
   - 検出一覧の確認（CLI）: `battery-bar.exe --list`（JSON 出力は `--json`）
   - 設定の保存先: `%APPDATA%\BatteryBar\config.json`

> **Note**: 未署名の実行ファイルのため、初回起動時に Windows SmartScreen の確認画面が表示される場合があります。気になる場合は以下の手順でソースから直接ビルドできます。

## ビルド方法
Windows + Rust (MSVC ツールチェイン) が必要です。

```powershell
powershell -ExecutionPolicy Bypass -File .\build-release.ps1
```

`target\release\battery-bar.exe` にローカルパス情報を含まないクリーンなリリースバイナリが生成されます。

## License
MIT
