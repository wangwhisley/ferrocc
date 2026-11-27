use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Seek, Write};
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime};

use chardetng::EncodingDetector;
use ferrous_opencc::config::BuiltinConfig;
use ferrous_opencc::OpenCC;
use regex::Regex;
use serde::Deserialize;
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const CONFIG_PATH: &str = "/config/config.toml";
const INPUT_DIR: &str = "/input";
const OUTPUT_DIR: &str = "/output";
const DONE_DIR: &str = "/done";
const FAILED_DIR: &str = "/failed";

const EPUB_TEXT_EXTS: &[&str] = &[
    ".xhtml", ".html", ".htm", ".ncx", ".opf", ".xml", ".txt", ".svg",
];

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
struct AppConfig {
    opencc_config: String,
    convert_filename: bool,
    keep_original: bool,
    poll_interval: u64,
    text_extensions: Vec<String>,
    ignore_names: Vec<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            opencc_config: "s2twp.json".to_string(),
            convert_filename: true,
            keep_original: true,
            poll_interval: 3,
            text_extensions: vec![
                ".txt", ".srt", ".ass", ".ssa", ".vtt", ".sub", ".md", ".lrc", ".csv", ".xml",
                ".html", ".htm", ".json",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            ignore_names: vec![".DS_Store", "Thumbs.db", "@eaDir"]
                .into_iter()
                .map(String::from)
                .collect(),
        }
    }
}

fn parse_builtin_config(name: &str) -> Result<BuiltinConfig, String> {
    let key = name.trim().to_lowercase();
    let key = key.strip_suffix(".json").unwrap_or(&key);
    match key {
        "s2t" => Ok(BuiltinConfig::S2t),
        "t2s" => Ok(BuiltinConfig::T2s),
        "s2tw" => Ok(BuiltinConfig::S2tw),
        "tw2s" => Ok(BuiltinConfig::Tw2s),
        "s2twp" => Ok(BuiltinConfig::S2twp),
        "tw2sp" => Ok(BuiltinConfig::Tw2sp),
        "s2hk" => Ok(BuiltinConfig::S2hk),
        "hk2s" => Ok(BuiltinConfig::Hk2s),
        "t2tw" => Ok(BuiltinConfig::T2tw),
        "tw2t" => Ok(BuiltinConfig::Tw2t),
        "t2hk" => Ok(BuiltinConfig::T2hk),
        "hk2t" => Ok(BuiltinConfig::Hk2t),
        "t2jp" => Ok(BuiltinConfig::T2jp),
        "jp2t" => Ok(BuiltinConfig::Jp2t),
        _ => Err(format!("不支援的 OpenCC 模式: {name}")),
    }
}

struct ConfigManager {
    path: PathBuf,
    last_mtime: Option<SystemTime>,
    cfg: AppConfig,
    converter: OpenCC,
    text_exts: HashSet<String>,
    ignore_names: HashSet<String>,
}

impl ConfigManager {
    fn new(path: &str) -> Self {
        let cfg = AppConfig::default();
        let converter =
            OpenCC::from_config(BuiltinConfig::S2twp).expect("初始化預設 OpenCC 失敗");
        let text_exts = cfg.text_extensions.iter().map(|s| s.to_lowercase()).collect();
        let ignore_names = cfg.ignore_names.iter().cloned().collect();

        let mut mgr = Self {
            path: PathBuf::from(path),
            last_mtime: None,
            cfg,
            converter,
            text_exts,
            ignore_names,
        };
        mgr.reload_if_changed(true);
        mgr
    }

    fn reload_if_changed(&mut self, force: bool) {
        let Ok(meta) = fs::metadata(&self.path) else {
            if force {
                println!("找不到 {}，使用內建預設設定", self.path.display());
            }
            return;
        };

        let Ok(mtime) = meta.modified() else { return };
        if !force && self.last_mtime == Some(mtime) {
            return;
        }

        let result: Result<(), String> = (|| {
            let content = fs::read_to_string(&self.path).map_err(|e| e.to_string())?;
            let new_cfg: AppConfig = toml::from_str(&content).map_err(|e| e.to_string())?;
            let builtin = parse_builtin_config(&new_cfg.opencc_config)?;

            if force || new_cfg.opencc_config != self.cfg.opencc_config {
                self.converter = OpenCC::from_config(builtin).map_err(|e| e.to_string())?;
            }

            self.text_exts = new_cfg
                .text_extensions
                .iter()
                .map(|s| s.to_lowercase())
                .collect();
            self.ignore_names = new_cfg.ignore_names.iter().cloned().collect();
            self.cfg = new_cfg;
            Ok(())
        })();

        self.last_mtime = Some(mtime);
        match result {
            Ok(()) => println!(
                "[設定已載入] 模式: {} | 檔名轉換: {} | 保留原檔: {} | 輪詢: {}s",
                self.cfg.opencc_config,
                self.cfg.convert_filename,
                self.cfg.keep_original,
                self.cfg.poll_interval.max(1)
            ),
            Err(e) => println!("[設定檔載入失敗，維持舊設定] {e}"),
        }
    }
}

fn is_ignored(rel_path: &Path, ignore_names: &HashSet<String>) -> bool {
    for comp in rel_path.components() {
        if let Component::Normal(os_str) = comp {
            let name = os_str.to_string_lossy();
            if ignore_names.contains(name.as_ref())
                || name.starts_with('.')
                || name.starts_with("~$")
            {
                return true;
            }
        }
    }
    false
}

fn is_file_stable(path: &Path) -> bool {
    let Ok(m1) = fs::metadata(path) else {
        return false;
    };
    thread::sleep(Duration::from_secs(1));
    let Ok(m2) = fs::metadata(path) else {
        return false;
    };
    m1.len() == m2.len() && m2.len() > 0
}

fn decode_bytes(raw: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(raw) {
        return s.to_string();
    }
    let mut detector = EncodingDetector::new();
    detector.feed(raw, true);
    let encoding = detector.guess(None, true);
    let (decoded, _, _) = encoding.decode(raw);
    decoded.into_owned()
}

fn convert_text_file(
    src: &Path,
    dst: &Path,
    mgr: &ConfigManager,
) -> Result<(), Box<dyn std::error::Error>> {
    let raw = fs::read(src)?;
    let text = decode_bytes(&raw);
    let converted = mgr.converter.convert(&text);
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(dst, converted.as_bytes())?;
    Ok(())
}

fn convert_epub_file(
    src: &Path,
    dst: &Path,
    mgr: &ConfigManager,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let infile = File::open(src)?;
    let outfile = File::create(dst)?;
    convert_epub_io(infile, outfile, mgr)
}

fn convert_epub_io<R: Read + Seek, W: Write + Seek>(
    reader: R,
    writer: W,
    mgr: &ConfigManager,
) -> Result<(), Box<dyn std::error::Error>> {
    let is_to_tw = mgr.cfg.opencc_config.to_lowercase().contains("tw");
    let lang_re = Regex::new(r"(?i)(<dc:language[^>]*>)\s*zh[-_](CN|Hans|SG)\s*(</dc:language>)")?;

    let mut zin = ZipArchive::new(reader)?;
    let mut zout = ZipWriter::new(writer);

    // 依 EPUB 規範，mimetype 必須放第一個且不壓縮
    if let Ok(mut entry) = zin.by_name("mimetype") {
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zout.start_file("mimetype", opts)?;
        zout.write_all(&buf)?;
    }

    for i in 0..zin.len() {
        let mut entry = zin.by_index(i)?;
        let name = entry.name().to_string();
        if name == "mimetype" {
            continue;
        }

        if entry.is_dir() {
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zout.add_directory(&name, opts)?;
            continue;
        }

        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;

        let ext = Path::new(&name)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
            .unwrap_or_default();

        if EPUB_TEXT_EXTS.contains(&ext.as_str()) {
            let text = decode_bytes(&buf);
            let mut converted = mgr.converter.convert(&text);
            if is_to_tw && ext == ".opf" {
                converted = lang_re.replace_all(&converted, "${1}zh-TW${3}").into_owned();
            }
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            zout.start_file(&name, opts)?;
            zout.write_all(converted.as_bytes())?;
        } else {
            let opts = SimpleFileOptions::default().compression_method(entry.compression());
            zout.start_file(&name, opts)?;
            zout.write_all(&buf)?;
        }
    }

    zout.finish()?;
    Ok(())
}

fn get_output_rel_path(rel_path: &Path, mgr: &ConfigManager) -> PathBuf {
    if !mgr.cfg.convert_filename {
        return rel_path.to_path_buf();
    }
    let mut out = PathBuf::new();
    for comp in rel_path.components() {
        if let Component::Normal(os_str) = comp {
            out.push(mgr.converter.convert(&os_str.to_string_lossy()));
        }
    }
    out
}

/// 跨掛載點移動檔案（Docker 不同 mount 時 rename 會失敗，自動改用 copy + remove）
fn move_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::rename(src, dst).is_err() {
        fs::copy(src, dst)?;
        fs::remove_file(src)?;
    }
    Ok(())
}

fn error_log_path(failed_dst: &Path) -> PathBuf {
    let mut os_str = failed_dst.as_os_str().to_os_string();
    os_str.push(".error.txt");
    PathBuf::from(os_str)
}

fn write_error_log(
    failed_dst: &Path,
    rel_path: &Path,
    opencc_config: &str,
    err: &dyn std::fmt::Display,
) {
    let log_path = error_log_path(failed_dst);
    if let Some(parent) = log_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let content = format!(
        "檔案: {}\n轉換模式: {}\n錯誤原因: {}\n",
        rel_path.display(),
        opencc_config,
        err
    );
    if let Err(write_err) = fs::write(&log_path, content) {
        println!("寫入錯誤紀錄失敗 ({}): {write_err}", log_path.display());
    }
}

fn cleanup_empty_dirs(root: &Path, ignore_names: &HashSet<String>) {
    for entry in WalkDir::new(root).contents_first(true).into_iter().flatten() {
        let p = entry.path();
        if p == root || !p.is_dir() {
            continue;
        }
        if let Ok(rel) = p.strip_prefix(root) {
            if is_ignored(rel, ignore_names) {
                continue;
            }
        }
        if fs::read_dir(p)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
        {
            let _ = fs::remove_dir(p);
        }
    }
}

fn process_all(mgr: &mut ConfigManager) {
    let input_root = Path::new(INPUT_DIR);
    let output_root = Path::new(OUTPUT_DIR);
    let done_root = Path::new(DONE_DIR);
    let failed_root = Path::new(FAILED_DIR);

    let mut files: Vec<PathBuf> = WalkDir::new(input_root)
        .into_iter()
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| e.into_path())
        .collect();
    files.sort();

    for src in files {
        mgr.reload_if_changed(false);

        let Ok(rel_path) = src.strip_prefix(input_root) else {
            continue;
        };
        if is_ignored(rel_path, &mgr.ignore_names) {
            continue;
        }

        let ext = src
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
            .unwrap_or_default();

        if ext != ".epub" && !mgr.text_exts.contains(&ext) {
            continue;
        }

        if !is_file_stable(&src) {
            continue;
        }

        let out_rel = get_output_rel_path(rel_path, mgr);
        let dst = output_root.join(&out_rel);
        let done_dst = done_root.join(rel_path);
        let failed_dst = failed_root.join(rel_path);

        println!("正在處理: {}", rel_path.display());
        let res = if ext == ".epub" {
            convert_epub_file(&src, &dst, mgr)
        } else {
            convert_text_file(&src, &dst, mgr)
        };

        match res {
            Ok(()) => {
                let post_res = if mgr.cfg.keep_original {
                    move_file(&src, &done_dst)
                } else {
                    fs::remove_file(&src)
                };
                match post_res {
                    Ok(()) => println!("完成 -> {}", out_rel.display()),
                    Err(e) => println!("已轉出但清理原檔失敗 ({}): {e}", rel_path.display()),
                }
            }
            Err(e) => {
                println!("轉換失敗 ({}): {e}", rel_path.display());

                if dst.exists() {
                    let _ = fs::remove_file(&dst);
                }

                match move_file(&src, &failed_dst) {
                    Ok(()) => {
                        write_error_log(&failed_dst, rel_path, &mgr.cfg.opencc_config, &e);
                        println!(
                            "已將失敗檔案移至 /failed/{}（錯誤詳情見 .error.txt）",
                            rel_path.display()
                        );
                    }
                    Err(move_err) => {
                        println!(
                            "移動失敗檔案至 /failed 失敗 ({}): {move_err}",
                            rel_path.display()
                        );
                    }
                }
            }
        }
    }

    cleanup_empty_dirs(input_root, &mgr.ignore_names);
}

fn main() {
    for dir in [INPUT_DIR, OUTPUT_DIR, DONE_DIR, FAILED_DIR] {
        let _ = fs::create_dir_all(dir);
    }

    let mut mgr = ConfigManager::new(CONFIG_PATH);
    println!("開始監控 {INPUT_DIR}...");

    loop {
        mgr.reload_if_changed(false);
        process_all(&mut mgr);
        thread::sleep(Duration::from_secs(mgr.cfg.poll_interval.max(1)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_builtin_config() {
        assert!(parse_builtin_config("s2twp.json").is_ok());
        assert!(parse_builtin_config("S2TWP").is_ok());
        assert!(parse_builtin_config("tw2sp.json").is_ok());
        assert!(parse_builtin_config("invalid_mode").is_err());
    }

    #[test]
    fn test_opencc_s2twp_conversion() {
        let cc = OpenCC::from_config(BuiltinConfig::S2twp).expect("載入 s2twp 失敗");
        assert_eq!(cc.convert("软件测试"), "軟體測試");
    }

    #[test]
    fn test_decode_bytes_utf8_and_gbk() {
        let sample = "简体中文编码测试";
        assert_eq!(decode_bytes(sample.as_bytes()), sample);

        let (gbk_bytes, _, _) = encoding_rs::GBK.encode(sample);
        assert_eq!(decode_bytes(&gbk_bytes), sample);
    }

    #[test]
    fn test_is_ignored() {
        let ignore_names: HashSet<String> = [".DS_Store", "Thumbs.db", "@eaDir"]
            .into_iter()
            .map(String::from)
            .collect();

        assert!(is_ignored(Path::new(".DS_Store"), &ignore_names));
        assert!(is_ignored(Path::new("book/@eaDir/cover.jpg"), &ignore_names));
        assert!(is_ignored(Path::new("sub/~$temp.txt"), &ignore_names));
        assert!(is_ignored(Path::new(".hidden/file.txt"), &ignore_names));
        assert!(!is_ignored(Path::new("novel/chapter1.txt"), &ignore_names));
    }

    #[test]
    fn test_partial_toml_uses_defaults() {
        let partial = r#"
            opencc_config = "t2s.json"
            poll_interval = 10
        "#;
        let cfg: AppConfig = toml::from_str(partial).expect("解析 TOML 失敗");
        assert_eq!(cfg.opencc_config, "t2s.json");
        assert_eq!(cfg.poll_interval, 10);
        assert!(cfg.convert_filename);
        assert!(cfg.keep_original);
    }

    #[test]
    fn test_error_log_path() {
        assert_eq!(
            error_log_path(Path::new("/failed/novel/book.epub")),
            PathBuf::from("/failed/novel/book.epub.error.txt")
        );
    }

    #[test]
    fn test_convert_epub_in_memory() {
        use std::io::Cursor;

        let mut src_buf = Cursor::new(Vec::new());
        let fake_jpg: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];

        {
            let mut zw = ZipWriter::new(&mut src_buf);
            let deflated =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            let stored =
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

            zw.start_file("OEBPS/content.opf", deflated).unwrap();
            let opf_xml = "<package><metadata><dc:title>软件测试指南</dc:title><dc:language>zh-CN</dc:language></metadata></package>";
            zw.write_all(opf_xml.as_bytes()).unwrap();

            zw.start_file("OEBPS/ch1.xhtml", deflated).unwrap();
            zw.write_all("<html><body>这是一本软件测试电子书。</body></html>".as_bytes())
                .unwrap();

            zw.start_file("OEBPS/cover.jpg", stored).unwrap();
            zw.write_all(fake_jpg).unwrap();

            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();

            zw.finish().unwrap();
        }

        src_buf.set_position(0);
        let mut dst_buf = Cursor::new(Vec::new());
        let mgr = ConfigManager::new("/nonexistent/config.toml");
        convert_epub_io(&mut src_buf, &mut dst_buf, &mgr).expect("EPUB 轉換失敗");

        dst_buf.set_position(0);
        let mut zin = ZipArchive::new(dst_buf).expect("讀取產出 EPUB 失敗");

        {
            let mut first = zin.by_index(0).unwrap();
            assert_eq!(first.name(), "mimetype");
            assert_eq!(first.compression(), CompressionMethod::Stored);
            let mut content = String::new();
            first.read_to_string(&mut content).unwrap();
            assert_eq!(content, "application/epub+zip");
        }

        {
            let mut opf = zin.by_name("OEBPS/content.opf").unwrap();
            let mut content = String::new();
            opf.read_to_string(&mut content).unwrap();
            assert!(content.contains("<dc:language>zh-TW</dc:language>"));
            assert!(content.contains("<dc:title>軟體測試指南</dc:title>"));
        }

        {
            let mut ch1 = zin.by_name("OEBPS/ch1.xhtml").unwrap();
            let mut content = String::new();
            ch1.read_to_string(&mut content).unwrap();
            assert!(content.contains("這是一本軟體測試電子書。"));
        }

        {
            let mut img = zin.by_name("OEBPS/cover.jpg").unwrap();
            let mut bytes = Vec::new();
            img.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, fake_jpg);
        }
    }
}
