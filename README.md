# ferrocc

基於 Rust 與 [`ferrous-opencc`](https://crates.io/crates/ferrous-opencc) 打造的輕量級 Docker 資料夾監控轉檔工具。將檔案或整個資料夾丟進 `input`，自動轉換繁簡體後依原目錄結構輸出至 `output`。

## 功能特色

- **純 Rust 單一執行檔**：常駐記憶體佔用極低（約數 MB），適合長期部署於 Synology NAS 或低功耗主機。
- **子資料夾遞迴處理**：保留完整目錄結構，並可選擇同步轉換檔名與資料夾名稱。
- **EPUB 電子書原生支援**：自動解包轉換內文與目錄，維持 `mimetype` 不壓縮規範，轉為台灣正體時自動修正 `<dc:language>` 為 `zh-TW`。
- **自動偵測文字編碼**：支援 UTF-8、GBK、GB18030、Big5 等常見編碼，統一輸出乾淨的 UTF-8。
- **設定檔熱重載**：修改 `config.toml` 存檔後自動生效，無須重啟容器；語法錯誤時會保留舊設定不中斷服務。
- **失敗隔離機制**：轉換失敗的檔案自動移至 `failed/` 並產生同名 `.error.txt` 記錄錯誤原因，不卡住後續任務。

## 快速開始（Synology Container Manager / Docker Compose）

1. 在主機或 NAS 建立目錄（如 `/docker/ferrocc`）與子資料夾：
   ```text
   ferrocc/
   ├── docker-compose.yml
   ├── config/
   │   └── config.toml      # 由 config/config.example.toml 複製修改
   ├── input/
   ├── output/
   ├── done/
   └── failed/
   ```
2. 複製專案內的 `docker-compose.yml` 並將 `<你的github帳號>` 改為實際帳號（小寫）。
3. 在 Synology Container Manager → **專案 (Project)** 新增並指向此目錄，或於 CLI 執行 `docker compose up -d`。

## 授權條款（License）

本專案採用 [Apache License 2.0](LICENSE) 授權。

### 第三方元件與致謝

- [ferrous-opencc](https://crates.io/crates/ferrous-opencc)（Apache-2.0）：提供純 Rust 實作的繁簡轉換核心與內建字典。
- [OpenCC](https://github.com/BYVoid/OpenCC)（Apache-2.0）：上游開放中文轉換字典與轉換規則。
