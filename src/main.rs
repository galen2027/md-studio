#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod docx;
mod editor_assets;
mod session;
#[cfg(target_os = "windows")]
mod single_instance;
mod source_map;

use notify::{Event, RecursiveMode, Watcher};
use pulldown_cmark::{html, CowStr, Event as MdEvent, Options, Parser, Tag, TagEnd};
use session::DocumentSession;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event::{Event as TaoEvent, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tao::window::{Theme, Window, WindowBuilder};
use wry::{WebView, WebViewBuilder};

const ICON_BYTES: &[u8] = include_bytes!("../assets/icon.ico");
const DEFAULT_W: f64 = 900.0;
const DEFAULT_H: f64 = 700.0;
const LOCAL_IMAGE_SCHEME: &str = "mdpreview-image";
const LOCAL_IMAGE_GENERATIONS_TO_KEEP: u64 = 8;
static APP_DIRTY: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Diagnostic log + dialog parenting
// ---------------------------------------------------------------------------

/// Append one timestamped (UTC) line to <config>/md-preview.log. The log is
/// truncated once it exceeds 1 MB. Used to diagnose lifecycle issues such as
/// close/quit flows; failures are ignored by design.
fn log_event(msg: &str) {
    let dir = config_dir();
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("md-preview.log");
    if let Ok(meta) = fs::metadata(&path) {
        if meta.len() > 1_000_000 {
            let _ = fs::write(&path, "---- log rotated (size cap) ----\n");
        }
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write;
        let _ = writeln!(file, "[{}] {msg}", utc_now_string());
    }
}

/// Format the current UTC time as `YYYY-MM-DD HH:MM:SS.mmm` without pulling
/// in a date crate (civil-from-days algorithm).
fn utc_now_string() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let millis = now.subsec_millis();
    let days = (secs / 86_400) as i64;
    let secs_of_day = secs % 86_400;
    let (hour, min, sec) = (
        secs_of_day / 3600,
        secs_of_day % 3600 / 60,
        secs_of_day % 60,
    );
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {hour:02}:{min:02}:{sec:02}.{millis:03} UTC")
}

/// Parent handle captured once at startup so every native dialog (file picker,
/// message box) is owned by the main window. Without an owner, dialogs can
/// open *behind* the WebView2 window while modally disabling it, which looks
/// exactly like an app hang ("未响应").
#[derive(Copy, Clone)]
struct MainWindowParent {
    window: raw_window_handle::RawWindowHandle,
    display: raw_window_handle::RawDisplayHandle,
}

// The raw handles are only ever used on the main event-loop thread to parent
// modal dialogs; sharing the static across threads is safe in practice.
unsafe impl Send for MainWindowParent {}
unsafe impl Sync for MainWindowParent {}

impl raw_window_handle::HasWindowHandle for MainWindowParent {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        // Safe in practice: the main window outlives every dialog shown from
        // the event loop, and dialogs are only created while it exists.
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(self.window) })
    }
}

impl raw_window_handle::HasDisplayHandle for MainWindowParent {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Ok(unsafe { raw_window_handle::DisplayHandle::borrow_raw(self.display) })
    }
}

static MAIN_WINDOW_PARENT: Mutex<Option<MainWindowParent>> = Mutex::new(None);

fn capture_main_window_parent(window: &Window) {
    use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
    let pair = window
        .window_handle()
        .ok()
        .map(|h| h.as_raw())
        .zip(window.display_handle().ok().map(|h| h.as_raw()));
    *MAIN_WINDOW_PARENT.lock().unwrap() =
        pair.map(|(window, display)| MainWindowParent { window, display });
}

/// Apply the main window as dialog owner when it was captured.
trait ParentDialog: Sized {
    fn parented(self, parent: &MainWindowParent) -> Self;
}

impl ParentDialog for rfd::FileDialog {
    fn parented(self, parent: &MainWindowParent) -> Self {
        self.set_parent(parent)
    }
}

impl ParentDialog for rfd::MessageDialog {
    fn parented(self, parent: &MainWindowParent) -> Self {
        self.set_parent(parent)
    }
}

fn parented<D: ParentDialog>(dialog: D) -> D {
    match *MAIN_WINDOW_PARENT.lock().unwrap() {
        Some(parent) => dialog.parented(&parent),
        None => dialog,
    }
}

#[derive(Debug)]
struct SelfWriteRecord {
    path: PathBuf,
    content: String,
}

#[derive(Debug)]
enum UserEvent {
    NewFile,
    OpenFile,
    OpenPaths(Vec<PathBuf>, bool),
    ActivateTab(u64),
    CloseTab(u64),
    CloseActiveTab,
    LocateTab(u64),
    FileChanged(PathBuf), // external change: refresh preview AND textarea
    ExternalChangeResolved(bool),
    FileSaved(PathBuf), // our own save: refresh preview only, leave textarea cursor alone
    RichEditPrepared(u64, Result<Option<PathBuf>, String>),
    SourcePreview(u64, String),
    SaveFailed {
        error: String,
        content: String,
        close_tab: Option<u64>,
    },
    QuitAck, // webview answered __mdPreviewSave with "nothing to save": safe to exit
    DirtyChanged(bool),
    ToggleEdit,
    ShowFind,
    Print, // route print through wry's native API (WKWebView ignores window.print())
    CheckUpdates,
    UpdateCheckResult(UpdateCheckResult),
    SetTheme(ThemeChoice),
    OpenUrl(&'static str),
    SaveAs(String), // save active tab under a new/chosen path (content from editor)
    EditorFlushed,  // untitled tab content stored in memory: clear webview dirty quietly
    UntitledPreview(u64, String), // refresh split preview without marking the scratch tab saved
    ExportDocx {
        standardize: bool,
        strip_numbers: bool,
        content: String,
    },
    /// Export the current preview as PDF: the webview sends a self-contained
    /// HTML snapshot, Rust prints it via a headless Chromium browser.
    ExportPdf(String),
    /// Reveal a tab's file in the OS file manager (tab context menu).
    RevealTab(u64),
    /// Save the mermaid lightbox's SVG source via a save dialog.
    DownloadSvg(String),
    /// Save the mermaid lightbox's rasterized PNG (data URL) via a save dialog.
    DownloadPng(String),
    Quit,
    RecentChanged,
    Ready, // first paint landed: inject hljs now; if bench mode, also exit
}

#[derive(Debug)]
enum UpdateCheckResult {
    Available {
        tag: String,
        url: String,
        digest: Option<String>,
    },
    UpToDate,
    Failed,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    fn as_str(self) -> &'static str {
        match self {
            ThemeChoice::System => "system",
            ThemeChoice::Light => "light",
            ThemeChoice::Dark => "dark",
        }
    }

    fn from_str(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "light" => ThemeChoice::Light,
            "dark" => ThemeChoice::Dark,
            _ => ThemeChoice::System,
        }
    }

    fn tao_theme(self) -> Option<Theme> {
        match self {
            ThemeChoice::System => None,
            ThemeChoice::Light => Some(Theme::Light),
            ThemeChoice::Dark => Some(Theme::Dark),
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct EnhanceFlags {
    math: bool,
    mermaid: bool,
}

#[derive(Clone, Debug)]
struct LocalImageAsset {
    path: PathBuf,
    mime: &'static str,
    generation: u64,
}

#[derive(Debug, Default)]
struct LocalImageRegistry {
    rewrites: HashMap<String, String>,
    generation: u64,
    next_id: u64,
    assets: HashMap<String, LocalImageAsset>,
}

type SharedLocalImageRegistry = Arc<Mutex<LocalImageRegistry>>;

impl LocalImageRegistry {
    fn begin_render(&mut self) {
        self.rewrites.clear();
        self.generation = self.generation.wrapping_add(1).max(1);
        self.next_id = 0;
        // WebView2 fetches image subresources asynchronously. Keep a small
        // window of previous render generations so a rapid save/tab switch
        // cannot invalidate an image URL before the browser requests it.
        let oldest = self
            .generation
            .saturating_sub(LOCAL_IMAGE_GENERATIONS_TO_KEEP);
        self.assets.retain(|_, asset| asset.generation >= oldest);
    }

    fn register(&mut self, path: PathBuf, mime: &'static str) -> String {
        self.next_id += 1;
        let key = format!("{}/{}", self.generation, self.next_id);
        self.assets.insert(
            key.clone(),
            LocalImageAsset {
                path,
                mime,
                generation: self.generation,
            },
        );
        local_image_transport_url(&key)
    }
}

fn local_image_transport_url(key: &str) -> String {
    // Wry's WebView2 backend registers custom protocols as
    // `http://<scheme>.<host>/...`. URLs inside NavigateToString HTML are not
    // rewritten automatically, so emit that transport form explicitly on
    // Windows. Other WebViews consume the native scheme URL.
    #[cfg(target_os = "windows")]
    {
        format!("http://{LOCAL_IMAGE_SCHEME}.localhost/{key}")
    }
    #[cfg(not(target_os = "windows"))]
    {
        format!("{LOCAL_IMAGE_SCHEME}://localhost/{key}")
    }
}

impl EnhanceFlags {
    fn any(self) -> bool {
        self.math || self.mermaid
    }
}

fn is_help_arg(arg: &str) -> bool {
    arg == "-h" || arg == "--help"
}

fn is_version_arg(arg: &str) -> bool {
    arg == "-V" || arg == "--version"
}

fn print_help() {
    println!(
        "Markdown Studio {}\n\nUsage:\n  md-preview [--edit] [file.md ...]\n\nOptions:\n  -h, --help       Show this help message\n  -V, --version    Show version information\n      --edit       Open files in source edit mode",
        env!("CARGO_PKG_VERSION")
    );
}

fn print_version() {
    println!("Markdown Studio {}", env!("CARGO_PKG_VERSION"));
}

#[derive(Copy, Clone)]
enum Lang {
    Zh,
    En,
}

/// Release date shown in the About popover; bump together with Cargo.toml.
const RELEASE_DATE: &str = "2026-11-19";
/// Developer name shown in the About popover.
const DEVELOPER_NAME: &str = "Rowin";

fn detect_lang() -> Lang {
    sys_locale::get_locale()
        .map(|l| {
            if l.to_lowercase().starts_with("zh") {
                Lang::Zh
            } else {
                Lang::En
            }
        })
        .unwrap_or(Lang::En)
}

struct Strings {
    preview_edit_zh: bool,
    btn_preview_edit: &'static str,
    preview_edit_hint: &'static str,
    drop_hint: &'static str,
    cannot_read: &'static str,
    open_file: &'static str,
    recent_title: &'static str,
    missing_title: &'static str,
    missing_body: &'static str,
    locate_file: &'static str,
    close_tab: &'static str,
    btn_edit: &'static str,
    btn_preview: &'static str,
    btn_split: &'static str,
    btn_split_exit: &'static str,
    btn_new: &'static str,
    new_filename: &'static str,
    btn_open: &'static str,
    btn_search: &'static str,
    btn_print: &'static str,
    btn_update: &'static str,
    btn_zoom: &'static str,
    btn_zoom_out: &'static str,
    btn_zoom_reset: &'static str,
    btn_zoom_in: &'static str,
    search_placeholder: &'static str,
    replace_placeholder: &'static str,
    replace_one: &'static str,
    replace_all: &'static str,
    replace_done: &'static str,
    stat_words: &'static str,
    stat_chars: &'static str,
    btn_toc: &'static str,
    toc_title: &'static str,
    btn_save_as: &'static str,
    btn_word: &'static str,
    untitled_name: &'static str,
    export_standardize: &'static str,
    export_strip_numbers: &'static str,
    export_now: &'static str,
    export_done_title: &'static str,
    export_done_body: &'static str,
    unsaved_title: &'static str,
    unsaved_body: &'static str,
    btn_about: &'static str,
    about_developer: &'static str,
    about_version: &'static str,
    about_updated: &'static str,
    print_now: &'static str,
    pdf_export: &'static str,
    pdf_done_body: &'static str,
    pdf_no_browser: &'static str,
    tab_reveal: &'static str,
    lb_zoom_out: &'static str,
    lb_zoom_in: &'static str,
    lb_fit: &'static str,
    lb_pan: &'static str,
    lb_png: &'static str,
    lb_svg: &'static str,
    lb_close: &'static str,
    img_done_body: &'static str,
    save_failed_title: &'static str,
    save_failed_body: &'static str,
    save_failed_save_as: &'static str,
    save_failed_discard_exit: &'static str,
    save_failed_discard_close: &'static str,
    save_failed_cancel: &'static str,
}

impl Strings {
    fn for_lang(lang: Lang) -> Self {
        match lang {
            Lang::Zh => Strings {
                preview_edit_zh: true,
                btn_preview_edit: "所见即所得编辑（标题 / 段落 / 列表 / 表格）",
                preview_edit_hint: "所见即所得编辑：直接修改排版后的文字，自动保存 / Ctrl+S。",
                drop_hint: "Drop a .md file here or press Cmd/Ctrl+O to open",
                cannot_read: "无法读取文件",
                open_file: "Open File",
                recent_title: "Recent",
                missing_title: "文件已移动或删除",
                missing_body: "这个标签会继续保留。你可以重新定位文件，或关闭标签。",
                locate_file: "重新定位",
                close_tab: "关闭标签",
                btn_edit: "编辑 (Cmd/Ctrl+E)",
                btn_preview: "预览 (Cmd/Ctrl+E)",
                btn_split: "分栏显示（左预览 / 右编辑）",
                btn_split_exit: "退出分栏显示",
                btn_new: "新建 Markdown (Cmd/Ctrl+N)",
                new_filename: "新建.md",
                btn_open: "Open File (Cmd/Ctrl+O)",
                btn_search: "搜索 (Cmd/Ctrl+F)",
                btn_print: "打印 (Cmd/Ctrl+P)",
                btn_update: "Update",
                btn_zoom: "正文缩放",
                btn_zoom_out: "缩小正文 (Cmd/Ctrl+-)",
                btn_zoom_reset: "重置正文缩放 (Cmd/Ctrl+0)",
                btn_zoom_in: "放大正文 (Cmd/Ctrl++)",
                search_placeholder: "搜索",
                replace_placeholder: "替换为",
                replace_one: "替换",
                replace_all: "全部替换",
                replace_done: "已替换 {count} 处",
                stat_words: "字",
                stat_chars: "字符",
                btn_toc: "章节导航 (Cmd/Ctrl+T)",
                toc_title: "目录",
                btn_save_as: "另存为 (Cmd/Ctrl+Shift+S)",
                btn_word: "导出 Word",
                untitled_name: "未命名",
                export_standardize: "格式标准化（宋体五号、去粗体底色、去空格）",
                export_strip_numbers: "去掉标题编号（保留层级）",
                export_now: "导出",
                export_done_title: "导出成功",
                export_done_body: "Word 文档已导出到：",
                unsaved_title: "未保存的更改",
                unsaved_body: "这个文档还没有保存过。是否先保存到文件？",
                btn_about: "关于",
                about_developer: "开发者",
                about_version: "版本",
                about_updated: "更新时间",
                print_now: "打印…",
                pdf_export: "导出 PDF",
                pdf_done_body: "PDF 已导出到：",
                pdf_no_browser: "未找到 Edge 或 Chrome，无法直接导出 PDF。请使用“打印”并在系统对话框中选择“另存为 PDF”。",
                tab_reveal: "打开文件所在文件夹",
                lb_zoom_out: "缩小",
                lb_zoom_in: "放大",
                lb_fit: "适应窗口",
                lb_pan: "按住空格键并拖动",
                lb_png: "下载 PNG（高清 3×）",
                lb_svg: "下载 SVG（矢量原图）",
                lb_close: "关闭",
                img_done_body: "图片已保存到：",
                save_failed_title: "无法保存原文件",
                save_failed_body: "原文件可能是只读文件或当前账户没有写入权限。你的编辑内容仍保留在编辑器中。",
                save_failed_save_as: "另存为…",
                save_failed_discard_exit: "放弃修改并退出",
                save_failed_discard_close: "放弃修改并关闭标签",
                save_failed_cancel: "取消",
            },
            Lang::En => Strings {
                preview_edit_zh: false,
                btn_preview_edit: "WYSIWYG edit (headings / paragraphs / lists / tables)",
                preview_edit_hint: "Edit formatted content directly. Autosave / Ctrl+S.",
                drop_hint: "Drop a .md file here or press Cmd/Ctrl+O to open",
                cannot_read: "Cannot read file",
                open_file: "Open File",
                recent_title: "Recent",
                missing_title: "File Moved or Deleted",
                missing_body: "This tab is kept. Locate the file again or close the tab.",
                locate_file: "Locate File",
                close_tab: "Close Tab",
                btn_edit: "Edit (Cmd/Ctrl+E)",
                btn_preview: "Preview (Cmd/Ctrl+E)",
                btn_split: "Split view (preview left / editor right)",
                btn_split_exit: "Exit split view",
                btn_new: "New Markdown (Cmd/Ctrl+N)",
                new_filename: "Untitled.md",
                btn_open: "Open File (Cmd/Ctrl+O)",
                btn_search: "Find (Cmd/Ctrl+F)",
                btn_print: "Print (Cmd/Ctrl+P)",
                btn_update: "Update",
                btn_zoom: "Content zoom",
                btn_zoom_out: "Zoom out (Cmd/Ctrl+-)",
                btn_zoom_reset: "Reset zoom (Cmd/Ctrl+0)",
                btn_zoom_in: "Zoom in (Cmd/Ctrl++)",
                search_placeholder: "Find",
                replace_placeholder: "Replace with",
                replace_one: "Replace",
                replace_all: "Replace All",
                replace_done: "{count} replaced",
                stat_words: "non-space",
                stat_chars: "chars",
                btn_toc: "Outline (Cmd/Ctrl+T)",
                toc_title: "Outline",
                btn_save_as: "Save As (Cmd/Ctrl+Shift+S)",
                btn_word: "Export Word",
                untitled_name: "Untitled",
                export_standardize: "Standardize (SimSun 10.5pt, no bold/shading, strip spaces)",
                export_strip_numbers: "Strip heading numbers (keep levels)",
                export_now: "Export",
                export_done_title: "Export Complete",
                export_done_body: "Word document exported to:",
                unsaved_title: "Unsaved Changes",
                unsaved_body: "This document has never been saved. Save it to a file first?",
                btn_about: "About",
                about_developer: "Developer",
                about_version: "Version",
                about_updated: "Released",
                print_now: "Print…",
                pdf_export: "Export PDF",
                pdf_done_body: "PDF exported to:",
                pdf_no_browser: "Edge or Chrome was not found, so direct PDF export is unavailable. Use Print and choose “Save as PDF” in the system dialog instead.",
                tab_reveal: "Open Containing Folder",
                lb_zoom_out: "Zoom out",
                lb_zoom_in: "Zoom in",
                lb_fit: "Fit to window",
                lb_pan: "Hold Space and drag",
                lb_png: "Download PNG (3x HD)",
                lb_svg: "Download SVG (vector original)",
                lb_close: "Close",
                img_done_body: "Image saved to:",
                save_failed_title: "Could Not Save Original File",
                save_failed_body: "The original file may be read-only or your account may not have write access. Your edits are still preserved in the editor.",
                save_failed_save_as: "Save As…",
                save_failed_discard_exit: "Discard and Exit",
                save_failed_discard_close: "Discard and Close Tab",
                save_failed_cancel: "Cancel",
            },
        }
    }
}

fn config_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("MD_PREVIEW_CONFIG_DIR") {
        return PathBuf::from(path);
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("APPDATA"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("md-preview")
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(".config/md-preview")
    }
}

#[derive(Copy, Clone)]
struct WindowGeom {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

fn geom_path() -> PathBuf {
    config_dir().join("window.geom")
}

fn theme_path() -> PathBuf {
    config_dir().join("theme.txt")
}

fn load_theme_choice() -> ThemeChoice {
    fs::read_to_string(theme_path())
        .map(|raw| ThemeChoice::from_str(&raw))
        .unwrap_or_default()
}

fn save_theme_choice(choice: ThemeChoice) {
    let dir = config_dir();
    let _ = fs::create_dir_all(&dir);
    let _ = fs::write(dir.join("theme.txt"), choice.as_str());
}

fn show_info_dialog(title: &str, description: &str) {
    let _ = parented(rfd::MessageDialog::new())
        .set_level(rfd::MessageLevel::Info)
        .set_title(title)
        .set_description(description)
        .show();
}

/// Load the optional Word export style template.
/// Lookup order: `MD_PREVIEW_TEMPLATE` env var, then `template.docx`
/// next to the executable. Missing or unparsable files fall back to the
/// built-in styles.
fn load_export_template() -> Option<docx::TemplateStyle> {
    let path = std::env::var_os("MD_PREVIEW_TEMPLATE")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| {
            let exe = std::env::current_exe().ok()?;
            let candidate = exe.parent()?.join("template.docx");
            candidate.is_file().then_some(candidate)
        })?;
    let bytes = fs::read(&path).ok()?;
    docx::TemplateStyle::from_docx(&bytes)
}

fn show_warning_dialog(title: &str, description: &str) {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("MD_PREVIEW_TEST_WARNING_LOG") {
        use std::io::Write;
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{title}: {description}");
        }
        return;
    }
    let _ = parented(rfd::MessageDialog::new())
        .set_level(rfd::MessageLevel::Warning)
        .set_title(title)
        .set_description(description)
        .show();
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SaveFailureChoice {
    SaveAs,
    Discard,
    Cancel,
}

fn classify_save_failure_choice(
    result: rfd::MessageDialogResult,
    save_as_label: &str,
    discard_label: &str,
) -> SaveFailureChoice {
    match result {
        rfd::MessageDialogResult::Custom(label) if label == save_as_label => {
            SaveFailureChoice::SaveAs
        }
        rfd::MessageDialogResult::Custom(label) if label == discard_label => {
            SaveFailureChoice::Discard
        }
        // Native fallbacks when custom labels are unavailable still preserve
        // the safe Yes/No/Cancel ordering.
        rfd::MessageDialogResult::Yes => SaveFailureChoice::SaveAs,
        rfd::MessageDialogResult::No => SaveFailureChoice::Discard,
        _ => SaveFailureChoice::Cancel,
    }
}

fn ask_after_save_failure(
    strings: &Strings,
    error: &str,
    closing_window: bool,
) -> SaveFailureChoice {
    let discard_label = if closing_window {
        strings.save_failed_discard_exit
    } else {
        strings.save_failed_discard_close
    };
    let result = parented(rfd::MessageDialog::new())
        .set_level(rfd::MessageLevel::Warning)
        .set_title(strings.save_failed_title)
        .set_description(format!("{}\n\n{}", strings.save_failed_body, error))
        .set_buttons(rfd::MessageButtons::YesNoCancelCustom(
            strings.save_failed_save_as.to_string(),
            discard_label.to_string(),
            strings.save_failed_cancel.to_string(),
        ))
        .show();
    classify_save_failure_choice(result, strings.save_failed_save_as, discard_label)
}

#[cfg(target_os = "windows")]
const WEBVIEW2_DOWNLOAD_URL: &str =
    "https://developer.microsoft.com/en-us/microsoft-edge/webview2/";

#[cfg(target_os = "windows")]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum WindowsWebViewStartupFailure {
    MissingRuntime,
    Other,
}

#[cfg(target_os = "windows")]
fn classify_windows_webview_startup_failure(details: &str) -> WindowsWebViewStartupFailure {
    let normalized = details.to_ascii_lowercase();
    let runtime_missing = [
        "0x80070002", // HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND)
        "webview2 runtime was not found",
        "webview2 runtime is not installed",
        "webview2 runtime not installed",
        "couldn't find a compatible webview2 runtime",
        "could not find a compatible webview2 runtime",
    ]
    .iter()
    .any(|marker| normalized.contains(marker));

    if runtime_missing {
        WindowsWebViewStartupFailure::MissingRuntime
    } else {
        WindowsWebViewStartupFailure::Other
    }
}

/// Report a WebView startup failure without panicking. The dialog is native
/// and parented to the already-created main window, so it remains visible even
/// when the WebView2 runtime itself cannot be created.
#[cfg(target_os = "windows")]
fn show_windows_webview_startup_failure(error: &wry::Error) -> ! {
    let details = format!("{error:?}");
    let failure = classify_windows_webview_startup_failure(&details);
    let log_path = config_dir().join("md-preview.log");
    log_event(&format!(
        "webview startup failed: kind={failure:?}; error={details}"
    ));

    match failure {
        WindowsWebViewStartupFailure::MissingRuntime => {
            let download_label = "下载 WebView2".to_string();
            let choice = parented(rfd::MessageDialog::new())
                .set_level(rfd::MessageLevel::Error)
                .set_title("Markdown Studio 无法启动")
                .set_description(format!(
                    "此电脑缺少或无法加载 Microsoft Edge WebView2 Runtime，Markdown Studio 无法显示预览。\n\n点击“下载 WebView2”打开微软官方下载页面，安装 Evergreen Runtime 后重新启动 Markdown Studio。\n\n诊断日志：{}",
                    log_path.display()
                ))
                .set_buttons(rfd::MessageButtons::OkCancelCustom(
                    download_label.clone(),
                    "关闭".to_string(),
                ))
                .show();

            if matches!(choice, rfd::MessageDialogResult::Custom(label) if label == download_label)
            {
                if let Err(open_error) = open::that(WEBVIEW2_DOWNLOAD_URL) {
                    log_event(&format!("webview2 download page open failed: {open_error}"));
                    let _ = parented(rfd::MessageDialog::new())
                        .set_level(rfd::MessageLevel::Warning)
                        .set_title("无法打开浏览器")
                        .set_description(format!(
                            "请手动访问微软 WebView2 官方下载页：\n{WEBVIEW2_DOWNLOAD_URL}\n\n诊断日志：{}",
                            log_path.display()
                        ))
                        .show();
                }
            }
        }
        WindowsWebViewStartupFailure::Other => {
            let _ = parented(rfd::MessageDialog::new())
                .set_level(rfd::MessageLevel::Error)
                .set_title("Markdown Studio 无法启动")
                .set_description(format!(
                    "预览组件初始化失败。请重新启动 Markdown Studio；若问题仍存在，可重启 Windows 后重试，并将诊断日志提供给维护人员。\n\n诊断日志：{}\n\n错误详情：{error}",
                    log_path.display()
                ))
                .show();
        }
    }

    std::process::exit(1);
}

/// Locate a Chromium-based browser for headless PDF printing.
/// `MD_PREVIEW_PDF_BROWSER` overrides the auto-detection. Candidates come
/// from both environment roots and well-known absolute paths, because some
/// launchers (e.g. Git Bash) do not propagate ProgramFiles variables.
#[cfg(target_os = "windows")]
fn find_pdf_browser() -> Option<PathBuf> {
    if let Some(custom) = std::env::var_os("MD_PREVIEW_PDF_BROWSER") {
        let custom = PathBuf::from(custom);
        if custom.is_file() {
            return Some(custom);
        }
    }
    const EDGE_REL: &str = r"Microsoft\Edge\Application\msedge.exe";
    const CHROME_REL: &str = r"Google\Chrome\Application\chrome.exe";
    let mut roots: Vec<PathBuf> = Vec::new();
    for env_key in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
        if let Some(root) = std::env::var_os(env_key) {
            roots.push(PathBuf::from(root));
        }
    }
    // SystemDrive is "C:" without a separator; join onto "C:\" explicitly —
    // "C:" + join would produce a drive-relative path that resolves against
    // the current directory instead of the drive root.
    let system_drive = std::env::var_os("SystemDrive")
        .map(|d| format!(r"{}\\", d.to_string_lossy()))
        .unwrap_or_else(|| r"C:\".to_string());
    for fixed in [r"Program Files (x86)", "Program Files"] {
        roots.push(PathBuf::from(&system_drive).join(fixed));
    }
    let found = roots
        .into_iter()
        .flat_map(|root| [root.join(EDGE_REL), root.join(CHROME_REL)])
        .find(|p| p.is_file());
    if found.is_none() {
        log_event("pdf browser detection: no candidate matched");
    }
    found
}

/// Render a self-contained HTML snapshot to PDF with headless Edge/Chrome.
/// `--no-pdf-header-footer` keeps the Chromium header/footer (date, title,
/// about:blank URL, page numbers) out of the output entirely.
#[cfg(target_os = "windows")]
fn export_pdf_via_browser(browser: &Path, html: &str, out: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let work_dir = std::env::temp_dir().join(format!("md-preview-pdf-{stamp}"));
    fs::create_dir_all(&work_dir).map_err(|e| e.to_string())?;
    let html_path = work_dir.join("document.html");
    fs::write(&html_path, html).map_err(|e| e.to_string())?;
    let html_url = format!("file:///{}", html_path.to_string_lossy().replace('\\', "/"));

    let mut child = std::process::Command::new(browser)
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--no-first-run")
        .arg("--no-pdf-header-footer")
        .arg(format!(
            "--user-data-dir={}",
            work_dir.join("profile").display()
        ))
        .arg(format!("--print-to-pdf={}", out.display()))
        .arg(&html_url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| e.to_string())?;

    // Headless print-to-pdf normally finishes in a few seconds; cap at 60s.
    let deadline = Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(100))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_dir_all(&work_dir);
                return Err("PDF export timed out".to_string());
            }
            Err(e) => {
                let _ = fs::remove_dir_all(&work_dir);
                return Err(e.to_string());
            }
        }
    }
    let _ = fs::remove_dir_all(&work_dir);
    let ok = fs::metadata(out).map(|m| m.len() > 0).unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err("PDF export produced no output".to_string())
    }
}

/// Default file stem for mermaid diagram downloads: "<doc>-diagram".
fn diagram_stem(session: &Arc<Mutex<DocumentSession>>, strings: &Strings) -> String {
    let base = {
        let session = session.lock().unwrap();
        match session.active() {
            Some(tab) if !tab.untitled => tab
                .path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| strings.untitled_name.to_string()),
            _ => strings.untitled_name.to_string(),
        }
    };
    format!("{base}-diagram")
}

/// Reveal a file in the OS file manager, selecting it in place.
#[cfg(target_os = "windows")]
fn reveal_in_folder(path: &Path) {
    // Session paths may carry a verbatim drive or UNC prefix, neither of
    // which explorer.exe accepts. Reuse the same conversion as file
    // association commands so `\\?\UNC\server\share` keeps both leading
    // backslashes instead of becoming the invalid `UNC\server\share` form.
    let clean = windows_shell_path(path);
    let _ = std::process::Command::new("explorer.exe")
        .arg(format!("/select,{clean}"))
        .spawn();
}

#[cfg(target_os = "macos")]
fn reveal_in_folder(path: &Path) {
    let _ = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
}

#[cfg(all(unix, not(target_os = "macos")))]
fn reveal_in_folder(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
    }
}

fn confirm_open_update(tag: &str) -> bool {
    matches!(
        parented(rfd::MessageDialog::new())
            .set_level(rfd::MessageLevel::Info)
            .set_title("Update Available")
            .set_description(format!(
                "Markdown Studio {tag} is available. Open the release page to download it?"
            ))
            .set_buttons(rfd::MessageButtons::OkCancelCustom(
                "Open Release".to_string(),
                "Cancel".to_string(),
            ))
            .show(),
        rfd::MessageDialogResult::Custom(label) if label == "Open Release"
    )
}

fn load_window_geom() -> Option<WindowGeom> {
    let txt = fs::read_to_string(geom_path()).ok()?;
    let parts: Vec<&str> = txt.trim().split(',').collect();
    if parts.len() != 4 {
        return None;
    }
    Some(WindowGeom {
        x: parts[0].parse().ok()?,
        y: parts[1].parse().ok()?,
        w: parts[2].parse().ok()?,
        h: parts[3].parse().ok()?,
    })
}

fn save_window_geom(window: &Window) {
    let Ok(pos) = window.outer_position() else {
        return;
    };
    let scale = window.scale_factor();
    let size = window.inner_size();
    let geom = WindowGeom {
        x: pos.x as f64 / scale,
        y: pos.y as f64 / scale,
        w: size.width as f64 / scale,
        h: size.height as f64 / scale,
    };
    if geom.w < 200.0 || geom.h < 150.0 {
        return;
    }
    let dir = config_dir();
    let _ = fs::create_dir_all(&dir);
    let _ = fs::write(
        dir.join("window.geom"),
        format!("{},{},{},{}", geom.x, geom.y, geom.w, geom.h),
    );
}

/// Clean-exit path shared by window close, app quit, and quit-ack.
///
/// Observed hang: after the close handler ran to completion (geometry and
/// session persisted, `ControlFlow::Exit` set), the process could stay alive
/// as "未响应" with a single waiting thread — stuck inside the WebView2
/// runtime's DLL detach during event-loop teardown, notably when the window
/// is closed seconds after startup while WebView2 is still initializing.
///
/// Everything worth saving is persisted before this point, so hide the
/// window (the app feels closed instantly even if something below stalls)
/// and exit the process directly, skipping the risky teardown entirely.
fn clean_exit_now(window: &Window, session: &DocumentSession, reason: &str) -> ! {
    log_event(reason);
    save_window_geom(window);
    persist_session(session);
    window.set_visible(false);
    log_event("exit: state persisted, skipping webview teardown");
    std::process::exit(0);
}

/// Return the saved geometry only when its center still falls inside
/// some connected monitor — prevents the window from landing off-screen
/// after a display swap.
fn geom_visible(geom: &WindowGeom, event_loop: &EventLoop<UserEvent>) -> bool {
    let cx = geom.x + geom.w / 2.0;
    let cy = geom.y + geom.h / 2.0;
    event_loop.available_monitors().any(|mon| {
        let scale = mon.scale_factor();
        let mp = mon.position();
        let ms = mon.size();
        let mx = mp.x as f64 / scale;
        let my = mp.y as f64 / scale;
        let mw = ms.width as f64 / scale;
        let mh = ms.height as f64 / scale;
        cx >= mx && cx <= mx + mw && cy >= my && cy <= my + mh
    })
}

fn centered_geom(event_loop: &EventLoop<UserEvent>) -> WindowGeom {
    if let Some(mon) = event_loop.primary_monitor() {
        let scale = mon.scale_factor();
        let mp = mon.position();
        let ms = mon.size();
        let mx = mp.x as f64 / scale;
        let my = mp.y as f64 / scale;
        let mw = ms.width as f64 / scale;
        let mh = ms.height as f64 / scale;
        WindowGeom {
            x: mx + (mw - DEFAULT_W) / 2.0,
            y: my + (mh - DEFAULT_H) / 2.0,
            w: DEFAULT_W,
            h: DEFAULT_H,
        }
    } else {
        WindowGeom {
            x: 100.0,
            y: 100.0,
            w: DEFAULT_W,
            h: DEFAULT_H,
        }
    }
}

#[cfg(test)]
fn md_to_html(md: &str) -> String {
    md_to_html_with_base(md, None)
}

#[cfg(test)]
fn md_to_html_with_base(md: &str, base_dir: Option<&Path>) -> String {
    let local_images = Arc::new(Mutex::new(LocalImageRegistry::default()));
    md_to_html_with_registry(md, base_dir, &local_images)
}

fn md_to_html_with_registry(
    md: &str,
    base_dir: Option<&Path>,
    local_images: &SharedLocalImageRegistry,
) -> String {
    local_images.lock().unwrap().begin_render();
    let (front_matter, markdown) = split_yaml_front_matter(md)
        .map(|(metadata, body)| (Some(metadata), body))
        .unwrap_or((None, md));
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM;
    let parser = Parser::new_ext(markdown, opts);
    let events = embed_local_images(
        add_mark_highlights(add_heading_ids(source_map::events(
            parser,
            markdown,
            md,
            md.len() - markdown.len(),
        ))),
        base_dir,
        local_images,
    );
    let mut html_out = String::new();
    if let Some(metadata) = front_matter {
        html_out.push_str(r#"<aside class="front-matter"><pre>"#);
        html_out.push_str(&html_escape_text(metadata.trim_end_matches(['\r', '\n'])));
        html_out.push_str("</pre></aside>\n");
    }
    html::push_html(&mut html_out, events.into_iter());
    for (original, transport) in &local_images.lock().unwrap().rewrites {
        let src = format!("src=\"{}\"", html_escape_attr(transport));
        html_out = html_out.replace(
            &src,
            &format!("{src} data-mdp-source=\"{}\"", html_escape_attr(original)),
        );
    }
    source_map::attach(html_out)
}

fn split_yaml_front_matter(md: &str) -> Option<(&str, &str)> {
    let mut lines = md.split_inclusive('\n');
    let first = lines.next()?;
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return None;
    }

    let metadata_start = first.len();
    let mut offset = metadata_start;
    for line in lines {
        let value = line.trim_end_matches(['\r', '\n']);
        if value == "---" || value == "..." {
            let metadata = &md[metadata_start..offset];
            let body = &md[offset + line.len()..];
            return Some((metadata, body));
        }
        offset += line.len();
    }
    None
}

fn embed_local_images<'a>(
    events: Vec<MdEvent<'a>>,
    base_dir: Option<&Path>,
    local_images: &SharedLocalImageRegistry,
) -> Vec<MdEvent<'a>> {
    let Some(base_dir) = base_dir else {
        return events;
    };

    events
        .into_iter()
        .map(|event| match event {
            MdEvent::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                let embedded = local_image_asset_url(base_dir, dest_url.as_ref(), local_images);
                if let Some(ref transport) = embedded {
                    local_images
                        .lock()
                        .unwrap()
                        .rewrites
                        .insert(dest_url.to_string(), transport.clone());
                }
                MdEvent::Start(Tag::Image {
                    link_type,
                    dest_url: embedded.map(CowStr::from).unwrap_or(dest_url),
                    title,
                    id,
                })
            }
            _ => event,
        })
        .collect()
}

fn local_image_asset_url(
    base_dir: &Path,
    url: &str,
    local_images: &SharedLocalImageRegistry,
) -> Option<String> {
    let image_path = resolve_local_image_path(base_dir, url)?;
    let mime = image_mime_type(&image_path)?;
    Some(local_images.lock().unwrap().register(image_path, mime))
}

fn resolve_local_image_path(base_dir: &Path, url: &str) -> Option<PathBuf> {
    let raw = url.trim();
    if raw.is_empty() || raw.starts_with('#') {
        return None;
    }

    // `file://` references are common in generated Markdown and already carry
    // their own percent-decoding/platform conversion rules.
    let candidate = if raw
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:"))
    {
        let parsed = url::Url::parse(raw).ok()?;
        if parsed
            .host_str()
            .is_some_and(|host| !host.eq_ignore_ascii_case("localhost"))
        {
            return None;
        }
        parsed.to_file_path().ok()?
    } else {
        let path_part = raw.split(['#', '?']).next()?.trim();
        // Do not resolve UNC/network shares from untrusted Markdown: that can
        // trigger SMB access and credential negotiation merely by previewing.
        if path_part.starts_with("//") || path_part.starts_with("\\\\") {
            return None;
        }
        let decoded = percent_decode_path_segment(path_part)?;
        #[cfg(target_os = "windows")]
        let decoded = decoded.replace('/', "\\");
        let decoded_path = PathBuf::from(decoded);

        // Preserve remote/data URLs unchanged. A Windows drive path is
        // absolute before this check, so its `C:` prefix is not mistaken for
        // a URI scheme.
        if !decoded_path.is_absolute() && url::Url::parse(path_part).is_ok() {
            return None;
        }
        if decoded_path.is_absolute() {
            decoded_path
        } else {
            base_dir.join(decoded_path)
        }
    };

    // Only the exact local path explicitly referenced by Markdown is
    // registered in the opaque asset map. This safely supports `../`, local
    // absolute paths, and file URLs without exposing a browsable filesystem
    // root to the WebView.
    let candidate = fs::canonicalize(candidate).ok()?;
    candidate.is_file().then_some(candidate)
}

fn local_image_protocol_response(
    local_images: &SharedLocalImageRegistry,
    request_path: &str,
) -> wry::http::Response<Cow<'static, [u8]>> {
    let key = request_path.trim_start_matches('/');
    let asset = local_images.lock().unwrap().assets.get(key).cloned();
    let Some(asset) = asset else {
        return wry::http::Response::builder()
            .status(404)
            .header(wry::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .header(wry::http::header::CACHE_CONTROL, "no-store")
            .header("X-Content-Type-Options", "nosniff")
            .body(Cow::Borrowed(&b"Not Found"[..]))
            .unwrap();
    };

    match fs::read(&asset.path) {
        Ok(bytes) => {
            let content_length = bytes.len().to_string();
            wry::http::Response::builder()
                .status(200)
                .header(wry::http::header::CONTENT_TYPE, asset.mime)
                .header(wry::http::header::CONTENT_LENGTH, content_length)
                .header(wry::http::header::CACHE_CONTROL, "no-store")
                .header("X-Content-Type-Options", "nosniff")
                .body(Cow::Owned(bytes))
                .unwrap()
        }
        Err(_) => wry::http::Response::builder()
            .status(404)
            .header(wry::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .header(wry::http::header::CACHE_CONTROL, "no-store")
            .header("X-Content-Type-Options", "nosniff")
            .body(Cow::Borrowed(&b"Not Found"[..]))
            .unwrap(),
    }
}

fn test_diagram_download_path(extension: &str) -> Option<PathBuf> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var_os("MD_PREVIEW_TEST_DOWNLOAD_DIR")
        .map(|directory| PathBuf::from(directory).join(format!("diagram.{extension}")))
}

fn percent_decode_path_segment(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = bytes.get(i + 1).and_then(|b| hex_value(*b))?;
            let lo = bytes.get(i + 2).and_then(|b| hex_value(*b))?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn image_mime_type(path: &Path) -> Option<&'static str> {
    match path
        .extension()?
        .to_string_lossy()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some("image/png"),
        "jpg" | "jpeg" | "jpe" | "jfif" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        "bmp" => Some("image/bmp"),
        "ico" => Some("image/x-icon"),
        "avif" => Some("image/avif"),
        _ => None,
    }
}

fn add_mark_highlights<'a>(events: Vec<MdEvent<'a>>) -> Vec<MdEvent<'a>> {
    let mut out = Vec::with_capacity(events.len());

    for event in events {
        match event {
            MdEvent::Text(text) => {
                if text.contains("==") {
                    push_mark_highlight_events(text.as_ref(), &mut out);
                } else {
                    out.push(MdEvent::Text(text));
                }
            }
            _ => out.push(event),
        }
    }

    out
}

fn push_mark_highlight_events<'a>(text: &str, out: &mut Vec<MdEvent<'a>>) {
    let mut rest = text;

    while let Some(open) = rest.find("==") {
        let after_open = open + 2;
        let Some(close_rel) = rest[after_open..].find("==") else {
            break;
        };
        let close = after_open + close_rel;
        let body = &rest[after_open..close];
        if body.trim().is_empty() {
            break;
        }

        if open > 0 {
            out.push(MdEvent::Text(CowStr::Boxed(
                rest[..open].to_string().into_boxed_str(),
            )));
        }
        out.push(MdEvent::Html(CowStr::Borrowed(
            r#"<mark class="mdp-mark">"#,
        )));
        out.push(MdEvent::Text(CowStr::Boxed(
            body.to_string().into_boxed_str(),
        )));
        out.push(MdEvent::Html(CowStr::Borrowed("</mark>")));
        rest = &rest[close + 2..];
    }

    if !rest.is_empty() {
        out.push(MdEvent::Text(CowStr::Boxed(
            rest.to_string().into_boxed_str(),
        )));
    }
}

fn add_heading_ids<'a>(mut events: Vec<MdEvent<'a>>) -> Vec<MdEvent<'a>> {
    let mut seen: HashMap<String, usize> = HashMap::new();

    for i in 0..events.len() {
        let generate_id = match &events[i] {
            MdEvent::Start(Tag::Heading { id: Some(id), .. }) => {
                register_heading_id(id.as_ref(), &mut seen);
                false
            }
            MdEvent::Start(Tag::Heading { id: None, .. }) => true,
            _ => false,
        };

        if !generate_id {
            continue;
        }

        let text = collect_heading_text(&events, i);
        let base = heading_slug(&text);
        let id_value = unique_heading_id(base, &mut seen);
        if let MdEvent::Start(Tag::Heading { id, .. }) = &mut events[i] {
            *id = Some(CowStr::Boxed(id_value.into_boxed_str()));
        }
    }

    events
}

fn collect_heading_text(events: &[MdEvent<'_>], start: usize) -> String {
    let mut text = String::new();

    for event in events.iter().skip(start + 1) {
        match event {
            MdEvent::End(TagEnd::Heading(_)) => break,
            MdEvent::Text(value)
            | MdEvent::Code(value)
            | MdEvent::InlineMath(value)
            | MdEvent::DisplayMath(value) => text.push_str(value.as_ref()),
            MdEvent::SoftBreak | MdEvent::HardBreak => text.push(' '),
            _ => {}
        }
    }

    text
}

fn heading_slug(text: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;

    for c in text.trim().chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() || c == '_' || c == '-' {
            slug.push(c);
            last_dash = false;
        } else if c.is_whitespace() && !slug.is_empty() && !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }

    while slug.ends_with('-') {
        slug.pop();
    }

    if slug.is_empty() {
        "section".to_string()
    } else {
        slug
    }
}

fn register_heading_id(id: &str, seen: &mut HashMap<String, usize>) {
    if !id.is_empty() {
        *seen.entry(id.to_string()).or_insert(0) += 1;
    }
}

fn unique_heading_id(base: String, seen: &mut HashMap<String, usize>) -> String {
    let count = seen.entry(base.clone()).or_insert(0);
    let id = if *count == 0 {
        base
    } else {
        format!("{base}-{count}")
    };
    *count += 1;
    id
}

// Embedded highlight.js + themes (offline)
const HLJS_JS: &str = include_str!("../assets/hljs/highlight.min.js");
const HLJS_LIGHT: &str = include_str!("../assets/hljs/github.min.css");
const HLJS_DARK: &str = include_str!("../assets/hljs/github-dark.min.css");
// Extra language pack(s) not in the `common` bundle. Each file
// ends with `hljs.registerLanguage(...)` and only works if evaluated
// in the same scope as the main bundle — we concat them into hljs-src.
const HLJS_EXTRA_LANGS: &str = concat!(
    // Delphi / Pascal (aliases: dpr, dfm, pas, pascal) — user requested
    include_str!("../assets/hljs/delphi.min.js"),
);
const PREVIEW_ENHANCE_JS: &str = include_str!("../assets/enhance/preview-enhance.js");
const PREVIEW_EDIT_JS: &str = include_str!("../assets/enhance/preview-edit.js");
const SPLIT_SYNC_JS: &str = include_str!("../assets/enhance/split-sync.js");
const MERMAID_EDIT_JS: &str = include_str!("../assets/enhance/mermaid-edit.js");
const UPDATE_CHECK_JS: &str = include_str!("../assets/enhance/update-check.js");
const KATEX_JS: &str = include_str!("../assets/katex/katex.min.js");
const KATEX_CSS: &str = include_str!("../assets/katex/katex.inline.css");
const MERMAID_JS: &str = include_str!("../assets/mermaid/mermaid.min.js");
const MAX_RECENT_FILES: usize = 8;

fn html_escape_ta(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;")
}

fn html_escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

fn html_escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn recent_files_path() -> PathBuf {
    config_dir().join("recent-files.txt")
}

fn session_path() -> PathBuf {
    config_dir().join("session.json")
}

fn tabs_json(session: &DocumentSession) -> String {
    let tabs = session
        .tabs
        .iter()
        .map(|tab| {
            let name = if tab.untitled {
                tab.path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Untitled".to_string())
            } else {
                tab.path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| tab.path.to_string_lossy().to_string())
            };
            serde_json::json!({
                "id": tab.id,
                "name": name,
                "path": tab.path.to_string_lossy(),
                "active": session.active_id == Some(tab.id),
                "missing": tab.missing,
                "dirty": tab.dirty,
                "untitled": tab.untitled,
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&tabs).expect("tab state is serializable")
}

fn missing_preview_html(tab_id: u64, path: &Path, s: &Strings) -> String {
    format!(
        r#"<div class="missing-file"><div class="missing-mark">!</div><h2>{}</h2><p>{}</p><code>{}</code><div class="missing-actions"><button type="button" data-locate-tab="{tab_id}">{}</button><button type="button" data-close-tab="{tab_id}">{}</button></div></div>"#,
        html_escape_text(s.missing_title),
        html_escape_text(s.missing_body),
        html_escape_text(&path.to_string_lossy()),
        html_escape_text(s.locate_file),
        html_escape_text(s.close_tab),
    )
}

fn load_recent_files() -> Vec<PathBuf> {
    let Ok(txt) = fs::read_to_string(recent_files_path()) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for line in txt.lines() {
        let path = PathBuf::from(line);
        if line.is_empty() || !path.exists() || files.iter().any(|p| p == &path) {
            continue;
        }
        files.push(path);
        if files.len() == MAX_RECENT_FILES {
            break;
        }
    }
    files
}

fn save_recent_files(files: &[PathBuf]) {
    let dir = config_dir();
    let _ = fs::create_dir_all(&dir);
    let body = files
        .iter()
        .take(MAX_RECENT_FILES)
        .map(|p| p.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n");
    let _ = fs::write(dir.join("recent-files.txt"), body);
}

fn remember_recent_file(files: &Arc<Mutex<Vec<PathBuf>>>, path: &Path) {
    let mut recent = files.lock().unwrap();
    recent.retain(|p| p != path);
    recent.insert(0, path.to_path_buf());
    recent.truncate(MAX_RECENT_FILES);
    save_recent_files(&recent);
}

fn forget_recent_file(files: &Arc<Mutex<Vec<PathBuf>>>, path: &Path) -> bool {
    let mut recent = files.lock().unwrap();
    let original_len = recent.len();
    recent.retain(|p| p != path);
    if recent.len() == original_len {
        return false;
    }
    save_recent_files(&recent);
    true
}

fn percent_encode_file_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b':' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn base_href_for_file(path: &Path) -> Option<String> {
    let dir = path.parent()?;
    Some(file_url_for_path_dir(dir))
}

fn file_url_for_path_dir(dir: &Path) -> String {
    let mut path = dir.to_string_lossy().replace('\\', "/");
    if cfg!(windows) && !path.starts_with('/') {
        path.insert(0, '/');
    }
    if !path.ends_with('/') {
        path.push('/');
    }
    format!("file://{}", percent_encode_file_path(&path))
}

fn starts_mermaid_fence(line: &str) -> bool {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix("```")
        .or_else(|| trimmed.strip_prefix("~~~"));
    let Some(info) = rest else {
        return false;
    };
    let info = info.trim_start();
    info == "mermaid"
        || info
            .strip_prefix("mermaid")
            .and_then(|s| s.chars().next())
            .map(|c| c.is_whitespace() || c == '{')
            .unwrap_or(false)
}

fn has_unescaped_at(s: &str, index: usize, needle: &str) -> bool {
    if !s[index..].starts_with(needle) {
        return false;
    }
    let mut backslashes = 0;
    for b in s[..index].bytes().rev() {
        if b == b'\\' {
            backslashes += 1;
        } else {
            break;
        }
    }
    backslashes % 2 == 0
}

fn has_unescaped_pair(s: &str, open: &str, close: &str) -> bool {
    let mut pos = 0;
    while let Some(rel) = s[pos..].find(open) {
        let start = pos + rel;
        if !has_unescaped_at(s, start, open) {
            pos = start + open.len();
            continue;
        }
        let body_start = start + open.len();
        let mut search = body_start;
        while let Some(close_rel) = s[search..].find(close) {
            let close_at = search + close_rel;
            if has_unescaped_at(s, close_at, close) {
                return true;
            }
            search = close_at + close.len();
        }
        pos = body_start;
    }
    false
}

fn has_inline_dollar_math(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'$' || !has_unescaped_at(s, i, "$") {
            i += 1;
            continue;
        }
        if bytes.get(i + 1).copied() == Some(b'$')
            || bytes
                .get(i + 1)
                .map(|b| b.is_ascii_whitespace())
                .unwrap_or(true)
        {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < bytes.len() {
            if bytes[j] == b'$'
                && has_unescaped_at(s, j, "$")
                && bytes
                    .get(j.wrapping_sub(1))
                    .map(|b| !b.is_ascii_whitespace())
                    .unwrap_or(false)
            {
                return true;
            }
            j += 1;
        }
        i += 1;
    }
    false
}

fn enhance_flags_for(md: &str) -> EnhanceFlags {
    EnhanceFlags {
        math: has_unescaped_pair(md, "$$", "$$")
            || has_unescaped_pair(md, "\\[", "\\]")
            || has_unescaped_pair(md, "\\(", "\\)")
            || has_inline_dollar_math(md),
        mermaid: md.lines().any(starts_mermaid_fence),
    }
}

fn build_enhancer_bootstrap(flags: EnhanceFlags, loaded: EnhanceFlags) -> Vec<String> {
    if !flags.any() {
        return Vec::new();
    }

    let mut scripts = Vec::new();
    if flags.math && !loaded.math {
        let mut js = String::from("(function(){\nif(!window.katex){\n");
        js.push_str(KATEX_JS);
        js.push_str("\n;try{window.katex=katex;}catch(e){}\n}\n");
        js.push_str("if(window.__setKatexCss)window.__setKatexCss('");
        js.push_str(&escape_js(KATEX_CSS));
        js.push_str("');\n})();");
        scripts.push(js);
    }
    if flags.mermaid && !loaded.mermaid {
        // Mermaid's standalone bundle expects global script scope. Keep it
        // out of the function wrapper that is safe for KaTeX/highlight.js.
        let mut js = String::with_capacity(MERMAID_JS.len() + 80);
        js.push_str(MERMAID_JS);
        js.push_str("\n;try{window.mermaid=mermaid;}catch(e){}\n");
        scripts.push(js);
    }
    scripts.push("if(window.__enhancePreview)window.__enhancePreview();".to_string());
    scripts
}

fn empty_preview_html(s: &Strings, recent_files: &[PathBuf]) -> String {
    let empty_class = if recent_files.is_empty() {
        "empty"
    } else {
        "empty has-recent"
    };
    let mut html = format!(
        r#"<div class="{empty_class}"><div class="icon">#</div><div>{}</div><button class="empty-open" type="button" data-open-file>{}</button>"#,
        html_escape_text(s.drop_hint),
        html_escape_text(s.open_file)
    );

    if !recent_files.is_empty() {
        html.push_str(&format!(
            r#"<div class="recent"><div class="recent-title">{}</div><div class="recent-list">"#,
            html_escape_text(s.recent_title)
        ));
        for (index, path) in recent_files.iter().take(MAX_RECENT_FILES).enumerate() {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_else(|| path.to_string_lossy());
            let parent = path
                .parent()
                .map(|p| p.to_string_lossy())
                .unwrap_or_default();
            html.push_str(&format!(
                r#"<button class="recent-item" type="button" data-recent-index="{index}"><span class="recent-name">{}</span><span class="recent-path">{}</span></button>"#,
                html_escape_text(&name),
                html_escape_text(&parent)
            ));
        }
        html.push_str("</div></div>");
    }

    html.push_str("</div>");
    html
}

fn build_page(
    preview_html: &str,
    raw_md: &str,
    base_href: Option<&str>,
    flags: EnhanceFlags,
    s: &Strings,
    empty: bool,
    native_updater: bool,
) -> String {
    let body_class = if empty { "empty" } else { "" };
    let base_tag = base_href
        .map(|href| format!(r#"<base id="base-href" href="{}">"#, html_escape_attr(href)))
        .unwrap_or_else(|| r#"<base id="base-href">"#.to_string());
    format!(
        r#"<!DOCTYPE html><html><head><meta charset="utf-8">
{base_tag}
<style id="hljs-light">{css_light}</style>
<style id="hljs-dark" media="not all">{css_dark}</style>
<script>{preview_edit_js}</script>
<script>{split_sync_js}</script>
<script>{mermaid_edit_js}</script>
<script>
(function(){{
  var mq = window.matchMedia('(prefers-color-scheme: dark)');
  function apply(e) {{
    document.getElementById('hljs-light').media = e.matches ? 'not all' : '';
    document.getElementById('hljs-dark').media = e.matches ? '' : 'not all';
  }}
  apply(mq); mq.addEventListener('change', apply);
}})();
</script>
<style>
:root {{ color-scheme: light dark; --chrome-top: 10px; --content-scale: 1; }}
/* Reserve scrollbar space permanently so the fixed toolbar doesn't shift
   between modes (one with scrollbar, one without). */
html {{ overflow-y: scroll; scrollbar-gutter: stable; }}
body {{
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif;
  margin: 0; padding: 0;
  line-height: 1.6; font-size: 15px;
  color: #1a1a1a; background: #fff;
}}
body.has-tabs {{ --chrome-top: 50px; }}
#app {{ max-width: 820px; margin: 0 auto; padding: 24px; }}
#preview {{ font-size: calc(15px * var(--content-scale)); }}
#preview .front-matter {{
  margin: 0 0 1.5em;
  padding: .85em 0;
  border-top: 1px solid #e1e4e8;
  border-bottom: 1px solid #e1e4e8;
  color: #59636e;
}}
#preview .front-matter pre {{
  margin: 0;
  padding: 0;
  border-radius: 0;
  overflow: visible;
  background: transparent;
  color: inherit;
  font: inherit;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}}
#preview h1,#preview h2,#preview h3,#preview h4 {{ margin-top: 1.4em; }}
#preview h1 {{ border-bottom: 1px solid #e1e4e8; padding-bottom: .3em; }}
#preview h2 {{ border-bottom: 1px solid #e1e4e8; padding-bottom: .2em; }}
#preview code {{ background: #f0f0f0; padding: 2px 6px; border-radius: 4px; font-size: 90%; }}
#preview pre {{ background: #f6f8fa; padding: 16px; border-radius: 8px; overflow-x: auto; }}
#preview pre code {{ background: none; padding: 0; font-size: 14px; }}
#preview blockquote {{ border-left: 4px solid #ddd; margin: 0; padding: 0 1em; color: #666; }}
#preview .markdown-alert-note,
#preview .markdown-alert-tip,
#preview .markdown-alert-important,
#preview .markdown-alert-warning,
#preview .markdown-alert-caution {{
  margin: 1em 0;
  padding: 0.75em 1em;
  border-radius: 6px;
  color: inherit;
}}
#preview .markdown-alert-title {{
  display: flex;
  align-items: center;
  gap: .35em;
  margin: 0 0 .45em;
  font-weight: 600;
  line-height: 1.25;
}}
#preview .markdown-alert-title + p {{ margin-top: 0; }}
#preview .markdown-alert-note {{ border-color: #0969da; background: #ddf4ff; }}
#preview .markdown-alert-tip {{ border-color: #1a7f37; background: #dafbe1; }}
#preview .markdown-alert-important {{ border-color: #8250df; background: #fbefff; }}
#preview .markdown-alert-warning {{ border-color: #9a6700; background: #fff8c5; }}
#preview .markdown-alert-caution {{ border-color: #cf222e; background: #ffebe9; }}
#preview .markdown-alert-note .markdown-alert-title {{ color: #0969da; }}
#preview .markdown-alert-tip .markdown-alert-title {{ color: #1a7f37; }}
#preview .markdown-alert-important .markdown-alert-title {{ color: #8250df; }}
#preview .markdown-alert-warning .markdown-alert-title {{ color: #9a6700; }}
#preview .markdown-alert-caution .markdown-alert-title {{ color: #cf222e; }}
#preview .mdp-mark {{ border-radius: 3px; padding: 0 0.12em; background: #fff2a8; color: inherit; }}
#preview mark.search-hit {{ border-radius: 3px; padding: 0 0.12em; background: #fff2a8; color: inherit; }}
#preview mark.search-hit.current {{ background: #ffcc4d; color: #1a1a1a; }}
#preview table {{ border-collapse: collapse; width: 100%; }}
#preview .mdp-table-wrap {{
  width: min(calc(100vw - 64px), 1280px);
  margin: 1em 0 1em 50%;
  transform: translateX(-50%);
  overflow-x: auto;
  -webkit-overflow-scrolling: touch;
}}
#preview .mdp-table-wrap table {{ width: max-content; min-width: 100%; }}
#preview table th, #preview table td {{ border: 1px solid #ddd; padding: 8px 12px; text-align: left; }}
#preview table th {{ background: #f6f8fa; font-weight: 600; color: #1a1a1a; white-space: nowrap; }}
#preview table td {{ min-width: 64px; max-width: 360px; vertical-align: top; overflow-wrap: break-word; }}
#preview img {{ max-width: 100%; }}
#preview .katex-display {{ overflow-x: auto; overflow-y: hidden; padding: 0.15em 0; }}
#preview .mdp-mermaid {{ margin: 1.2em 0; overflow-x: auto; text-align: center; cursor: zoom-in; }}
#preview .mdp-mermaid svg {{ max-width: 100%; height: auto; }}
#preview .mdp-mermaid-error, #preview .mdp-math-error {{ color: #b42318; }}
/* Mermaid lightbox: vector scaling stays crisp at any zoom level. */
.mdp-lightbox {{
  position: fixed; inset: 0; z-index: 200; display: none; flex-direction: column;
  background: rgba(0,0,0,.55); backdrop-filter: blur(2px); -webkit-backdrop-filter: blur(2px);
}}
.mdp-lightbox.open {{ display: flex; }}
.mdp-lightbox-toolbar {{
  flex: 0 0 auto; display: flex; align-items: center; gap: 6px; padding: 8px 12px;
  background: rgba(28,28,28,.96); border-bottom: 1px solid rgba(255,255,255,.08);
}}
.mdp-lightbox-toolbar button {{
  padding: 5px 12px; border: 1px solid rgba(255,255,255,.18); border-radius: 6px;
  background: transparent; color: #eee; font-size: 12.5px; cursor: pointer; white-space: nowrap;
}}
.mdp-lightbox-toolbar button:hover {{ background: rgba(255,255,255,.12); }}
.mdp-lightbox-scale {{
  min-width: 48px; color: #bbb; font-size: 12px; text-align: center;
  font-variant-numeric: tabular-nums; user-select: none;
}}
.mdp-lightbox-pan-hint {{ color: #aaa; font-size: 12px; user-select: none; }}
.mdp-lightbox-spacer {{ flex: 1; }}
.mdp-lightbox-body {{ flex: 1; overflow: auto; padding: 28px; cursor: default; }}
.mdp-lightbox.pan-ready .mdp-lightbox-body {{ cursor: grab; }}
.mdp-lightbox .mdp-lightbox-body.panning {{ cursor: grabbing; user-select: none; }}
.mdp-lightbox-canvas {{
  margin: 0 auto; width: max-content; padding: 14px; border-radius: 6px;
  background: #fff; box-shadow: 0 6px 28px rgba(0,0,0,.45);
}}
.mdp-diagram-editor {{ position: absolute; top: 64px; right: 14px; width: 290px; max-height: calc(100vh - 90px); overflow: auto; box-sizing: border-box; padding: 16px; border-radius: 8px; background: #202832; color: #fff; box-shadow: 0 4px 22px #0008; font: 14px/1.5 system-ui,sans-serif; }}
.mdp-diagram-editor[hidden] {{ display: none; }}
.mdp-diagram-editor label {{ display: block; margin-bottom: 12px; }}
.mdp-diagram-editor select,.mdp-diagram-editor textarea {{ display: block; width: 100%; box-sizing: border-box; margin-top: 6px; padding: 8px; font: inherit; color: #111; background: #fff; border: 1px solid #8794a5; border-radius: 4px; }}
.mdp-diagram-editor textarea {{ resize: vertical; min-height: 84px; }}
.mdp-diagram-actions {{ display: flex; gap: 8px; flex-wrap: wrap; }}
.mdp-diagram-editor button {{ padding: 6px 10px; color: #fff; background: #35485e; border: 1px solid #7a8a9e; border-radius: 4px; cursor: pointer; }}
.mdp-diagram-editor button:disabled {{ opacity: .45; cursor: default; }}
.mdp-diagram-editor [data-diagram-confirm] {{ background: #932f2f; margin-top: 12px; }}
.mdp-diagram-editor p {{ white-space: pre-wrap; overflow-wrap: anywhere; margin-bottom: 0; }}
.mdp-lightbox:has(.mdp-diagram-editor:not([hidden])) .mdp-lightbox-body {{ padding-right: 330px; }}
.mdp-lightbox .mdp-node-selected > rect,.mdp-lightbox .mdp-node-selected > polygon,.mdp-lightbox .mdp-node-selected > circle,.mdp-lightbox .mdp-node-selected > path {{ stroke: #1677ff !important; stroke-width: 3px !important; }}
@media (max-width: 600px) {{
  .mdp-diagram-editor {{ top: auto; bottom: 8px; left: 8px; right: 8px; width: auto; max-height: 44vh; }}
  .mdp-lightbox:has(.mdp-diagram-editor:not([hidden])) .mdp-lightbox-body {{ padding-right: 20px; padding-bottom: 46vh; }}
  .mdp-lightbox-pan-hint {{ display: none; }}
}}
#preview hr {{ border: none; border-top: 1px solid #e1e4e8; margin: 2em 0; }}
#preview a {{ color: #0969da; text-decoration: none; }}
#preview a:hover {{ text-decoration: underline; }}
#preview ul, #preview ol {{ padding-left: 2em; }}
#preview input[type="checkbox"] {{ margin-right: 6px; }}
	.empty {{ display: flex; flex-direction: column; align-items: center; justify-content: center;
	  min-height: 60vh; color: #999; font-size: 18px; gap: 12px; text-align: center; }}
	.empty.has-recent {{
	  justify-content: flex-start;
	  min-height: calc(100vh - 48px);
	  padding: clamp(56px, 10vh, 96px) 0 40px;
	  box-sizing: border-box;
	}}
	.empty .icon {{ font-size: 48px; opacity: 0.4; }}
	.empty-open {{
	  margin-top: 6px; min-height: 40px; padding: 0 16px;
	  border: 1px solid #ddd; border-radius: 8px; background: #fff;
	  color: #1a1a1a; font: inherit; font-size: 15px; cursor: pointer;
	}}
	.empty-open:hover {{ background: #f5f5f5; color: #000; }}
	.recent {{ width: min(480px, 100%); margin-top: 16px; text-align: left; }}
	.recent-title {{ margin: 0 0 8px; padding: 0; border: 0; font-size: 11px; font-weight: 600; letter-spacing: 0; color: #b6b6b6; text-transform: uppercase; }}
	.recent-list {{ display: grid; gap: 6px; }}
	.recent-item {{
	  width: 100%; min-height: 44px; padding: 7px 10px; border: 1px solid #eee;
	  border-radius: 8px; background: #fff; color: inherit; text-align: left; cursor: pointer;
	  display: grid; gap: 1px;
	}}
	.recent-item:hover {{ background: #f7f7f7; }}
	.recent-name {{ color: #555; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }}
	.recent-path {{ color: #aaa; font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }}
	.tabbar {{
	  display: none; position: sticky; top: 0; z-index: 110; height: 40px;
	  box-sizing: border-box; align-items: stretch; gap: 4px; padding: 4px 8px;
	  border-bottom: 1px solid #e6e6e6; background: rgba(248,248,248,0.96);
	  backdrop-filter: blur(12px); -webkit-backdrop-filter: blur(12px);
	}}
	body.has-tabs .tabbar {{ display: flex; }}
	.tabs {{ display: flex; flex: 1; min-width: 0; gap: 4px; overflow-x: auto; scrollbar-width: none; }}
	.tabs::-webkit-scrollbar {{ display: none; }}
	.tab {{
	  flex: 0 1 180px; min-width: 96px; max-width: 200px; height: 31px;
	  display: flex; align-items: center; gap: 7px; padding: 0 8px 0 10px;
	  box-sizing: border-box; border: 1px solid transparent; border-radius: 7px;
	  color: #6b6b6b; background: transparent; cursor: default; user-select: none;
	  font-size: 13px;
	}}
	.tab:hover {{ background: rgba(0,0,0,0.045); }}
	.tab.active {{ color: #202020; background: #fff; border-color: #ddd; box-shadow: 0 1px 2px rgba(0,0,0,.04); }}
	.tab.missing {{ color: #a15c00; }}
	.tab-status {{ width: 7px; height: 7px; flex: 0 0 auto; border-radius: 50%; background: transparent; }}
	.tab.dirty .tab-status {{ background: #2979c9; }}
	.tab.missing .tab-status {{ width: auto; height: auto; background: none; border-radius: 0; font-weight: 700; }}
	.tab-name {{ min-width: 0; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }}
	.tab-close {{
	  width: 20px; height: 20px; flex: 0 0 auto; padding: 0; border: 0; border-radius: 5px;
	  display: grid; place-items: center; color: inherit; background: transparent; cursor: pointer;
	  font: 16px/1 -apple-system, BlinkMacSystemFont, sans-serif; opacity: .58;
	}}
	.tab-close:hover {{ opacity: 1; background: rgba(0,0,0,.08); }}
	.tab-open {{
	  width: 31px; height: 31px; flex: 0 0 auto; padding: 0; border: 0; border-radius: 7px;
	  color: #666; background: transparent; cursor: pointer; font: 20px/1 -apple-system, sans-serif;
	}}
	.tab-open:hover {{ color: #111; background: rgba(0,0,0,.06); }}
	.tab-about {{
	  width: 31px; height: 31px; flex: 0 0 auto; padding: 0; border: 0; border-radius: 7px;
	  color: #666; background: transparent; cursor: pointer;
	  font: 700 15px/1 -apple-system, sans-serif; font-style: italic;
	}}
	.tab-about:hover {{ color: #111; background: rgba(0,0,0,.06); }}
	.tab-about.open {{ color: #0969da; background: rgba(9,105,218,.1); }}
	.about-popover {{
	  position: absolute; top: 42px; right: 8px; z-index: 130;
	  display: none; flex-direction: column; gap: 6px; padding: 12px 14px; min-width: 190px;
	  border: 1px solid rgba(0,0,0,.1); border-radius: 10px;
	  background: rgba(255,255,255,.97); box-shadow: 0 8px 24px rgba(0,0,0,.14);
	  backdrop-filter: blur(8px); -webkit-backdrop-filter: blur(8px);
	  font-size: 12.5px; color: #333;
	}}
	.about-popover.open {{ display: flex; }}
	.about-row {{ display: flex; justify-content: space-between; gap: 18px; }}
	.about-label {{ color: #8b8b8b; }}
	.about-value {{ font-weight: 600; color: #202020; user-select: text; }}
	.tab-menu {{
	  position: fixed; z-index: 140; min-width: 180px; padding: 4px;
	  border: 1px solid rgba(0,0,0,.1); border-radius: 9px;
	  background: rgba(255,255,255,.97); box-shadow: 0 8px 24px rgba(0,0,0,.16);
	  backdrop-filter: blur(8px); -webkit-backdrop-filter: blur(8px);
	}}
	.tab-menu button {{
	  display: block; width: 100%; padding: 7px 12px; border: 0; border-radius: 6px;
	  background: transparent; color: #333; font-size: 13px; text-align: left; cursor: pointer;
	}}
	.tab-menu button:hover {{ background: rgba(9,105,218,.1); color: #0969da; }}
	.doc-stats {{
	  flex: 0 0 auto; align-self: center; padding: 0 6px;
	  color: #8b8b8b; font-size: 11px; white-space: nowrap; user-select: none;
	  font-variant-numeric: tabular-nums;
	}}
	.missing-file {{ min-height: 55vh; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 10px; text-align: center; }}
	.missing-file h2, .missing-file p {{ margin: 0; }}
	.missing-file p {{ color: #777; }}
	.missing-file code {{ max-width: min(620px, 90vw); overflow-wrap: anywhere; color: #8a5a12; }}
	.missing-mark {{ width: 38px; height: 38px; border: 2px solid #c98322; border-radius: 50%; display: grid; place-items: center; color: #a15c00; font-weight: 700; font-size: 22px; }}
	.missing-actions {{ display: flex; gap: 8px; margin-top: 8px; }}
	.missing-actions button {{ min-height: 36px; padding: 0 13px; border: 1px solid #d8d8d8; border-radius: 7px; background: #fff; color: #333; cursor: pointer; font: inherit; }}
	.missing-actions button:hover {{ background: #f4f4f4; }}

/* Outline / chapter navigation sidebar (left) */
#toc-panel {{
  display: none; position: fixed; left: 0; top: var(--chrome-top); bottom: 0;
  width: 236px; box-sizing: border-box; overflow-y: auto; overscroll-behavior: contain;
  padding: 12px 8px 24px 14px; border-right: 1px solid #e6e6e6;
  background: #fafafa; z-index: 90; font-size: 13px; line-height: 1.5;
}}
body.toc-open.has-toc #toc-panel {{ display: block; }}
body.toc-open.has-toc #app {{ max-width: none; padding-left: 260px; }}
body.toc-open.has-toc #preview {{ max-width: 820px; margin: 0 auto; }}
body.editing #toc-panel {{ display: none !important; }}
body.editing.toc-open.has-toc #app {{ padding-left: 0; }}
body.split-view #toc-panel {{ display: none !important; }}
body.split-view.toc-open.has-toc #app {{ padding-left: 0; }}
#toc-header {{
  font-size: 11px; font-weight: 600; letter-spacing: .06em; text-transform: uppercase;
  color: #999; padding: 2px 8px 8px; user-select: none;
}}
#toc a.toc-item {{
  display: block; padding: 3px 8px 3px 10px; border-radius: 6px;
  color: #444; text-decoration: none; cursor: pointer;
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}}
#toc a.toc-item:hover {{ background: rgba(0,0,0,.05); color: #111; }}
#toc a.toc-item.active {{ color: #0969da; font-weight: 600; }}
#toc a.toc-item[data-depth="1"] {{ padding-left: 22px; }}
#toc a.toc-item[data-depth="2"] {{ padding-left: 34px; }}
#toc a.toc-item[data-depth="3"] {{ padding-left: 46px; }}
#toc a.toc-item[data-depth="4"] {{ padding-left: 58px; }}
#toc a.toc-item[data-depth="5"] {{ padding-left: 70px; }}
.toolbar button#btn-toc.on {{ color: #0969da; }}
.toolbar button#btn-split.on {{ color: #0969da; background: rgba(9,105,218,.1); }}

/* Floating toolbar (top-right) — hover-reveal, hidden in empty state unless an update exists */
.toolbar {{
  position: fixed; top: var(--chrome-top); right: 12px;
  display: flex; gap: 6px; z-index: 100;
  opacity: 0; pointer-events: none;
  transition: opacity 0.18s ease;
}}
html:hover .toolbar {{ opacity: 1; pointer-events: auto; }}
body.empty .toolbar:not(.has-update) {{ display: none !important; }}
body.empty .toolbar.has-update {{ opacity: 1; pointer-events: auto; }}
body.empty .toolbar.has-update button:not(.update-btn) {{ display: none !important; }}
.toolbar button {{
  width: 34px; height: 34px; padding: 0;
  background: rgba(255,255,255,0.8);
  backdrop-filter: blur(6px);
  -webkit-backdrop-filter: blur(6px);
  border: 1px solid rgba(0,0,0,0.08);
  border-radius: 8px;
  display: grid; place-items: center;
  cursor: pointer; color: #555;
  transition: color 0.15s, background 0.15s;
}}
.toolbar button:hover {{ color: #000; background: rgba(255,255,255,1); }}
.toolbar button[hidden] {{ display: none !important; }}
	.zoom-control {{ position: relative; }}
	.zoom-popover {{
	  position: absolute; top: 40px; right: 0;
	  display: none; align-items: center; gap: 2px; padding: 4px;
	  border: 1px solid rgba(0,0,0,.1); border-radius: 8px;
	  background: rgba(255,255,255,.96); box-shadow: 0 6px 20px rgba(0,0,0,.12);
	  backdrop-filter: blur(8px); -webkit-backdrop-filter: blur(8px);
	}}
	.zoom-control.open .zoom-popover {{ display: flex; }}
	.toolbar .zoom-popover button {{ width: 30px; height: 30px; border: 0; background: transparent; }}
	.toolbar .zoom-popover button:hover {{ background: rgba(0,0,0,.06); }}
	.toolbar .zoom-popover .zoom-reset {{
	  width: 52px; font-size: 11px; font-variant-numeric: tabular-nums;
	}}
	.word-popover {{
	  flex-direction: column; align-items: stretch; gap: 8px; padding: 10px; min-width: 250px;
	}}
	.word-popover label {{
	  display: flex; gap: 7px; align-items: flex-start; font-size: 12.5px;
	  color: #444; cursor: pointer; line-height: 1.45; user-select: none;
	}}
	.word-popover label input {{ margin: 2px 0 0; flex: 0 0 auto; }}
	.toolbar .word-popover .word-export {{
	  width: auto; height: 30px; margin-top: 2px; border: 0; border-radius: 7px;
	  background: #0969da; color: #fff; font: inherit; font-size: 13px; cursor: pointer;
	}}
	.toolbar .word-popover .word-export:hover {{ background: #0757b8; }}
	.print-popover {{
	  flex-direction: column; align-items: stretch; gap: 4px; padding: 6px; min-width: 150px;
	}}
	.toolbar .print-popover .print-action {{
	  width: auto; height: 32px; padding: 0 14px; border: 0; border-radius: 7px;
	  background: transparent; color: #333; font: inherit; font-size: 13px;
	  text-align: left; cursor: pointer; white-space: nowrap;
	}}
	.toolbar .print-popover .print-action:hover {{ background: rgba(9,105,218,.1); color: #0969da; }}
	.toolbar .update-btn {{
	  width: auto; min-width: 76px; padding: 0 11px; grid-auto-flow: column; gap: 5px;
	  font-size: 13px; font-weight: 600; color: #0969da;
	}}
	.toolbar .update-mark {{ font-size: 17px; line-height: 1; transform: translateY(-0.5px); }}
	.findbar {{
	  position: fixed; top: var(--chrome-top); left: 50%; transform: translateX(-50%);
	  display: none; flex-direction: column; align-items: stretch; gap: 6px; z-index: 101;
	  max-width: calc(100vw - 16px); box-sizing: border-box;
	  padding: 6px; border: 1px solid rgba(0,0,0,0.08); border-radius: 10px;
	  background: rgba(255,255,255,0.96); box-shadow: 0 8px 24px rgba(0,0,0,0.12);
	  backdrop-filter: blur(8px); -webkit-backdrop-filter: blur(8px);
	}}
	body.finding .findbar {{ display: flex; }}
	.find-row, .replace-row {{ display: flex; align-items: center; gap: 6px; min-width: 0; }}
	.replace-row {{ display: none; }}
	body.replacing .replace-row {{ display: flex; }}
	.findbar input {{
	  width: min(42vw, 320px); min-width: 0; height: 30px; box-sizing: border-box; border: 0;
	  outline: none; background: transparent; color: inherit; font: inherit;
	}}
	.findbar .replace-row input {{ flex: 1 1 auto; }}
	.findbar span {{ min-width: 14px; color: #8c959f; text-align: center; }}
	.findbar button {{
	  width: 30px; height: 30px; padding: 0; border: 0; border-radius: 7px;
	  display: grid; place-items: center; color: #555; background: transparent; cursor: pointer;
	}}
	.findbar button:hover {{ background: #f0f0f0; color: #111; }}
	.findbar button:disabled {{ opacity: .4; cursor: default; background: transparent; }}
	.findbar .replace-action {{
	  width: auto; min-width: 58px; padding: 0 9px; white-space: nowrap; font: inherit; font-size: 12px;
	}}
	@media (max-width: 520px) {{
	  .findbar {{ left: 8px; right: 8px; transform: none; }}
	  .findbar input {{ width: auto; flex: 1 1 auto; }}
	  .findbar .replace-action {{ min-width: 0; }}
	}}
	@media (prefers-color-scheme: dark) {{
	  body {{ color: #d4d4d4; background: #1e1e1e; }}
	  #preview a {{ color: #6cb6ff; }}
	  #preview h1, #preview h2 {{ border-color: #333; }}
	  #preview .front-matter {{ border-color: #333; color: #a9b1ba; }}
	  #preview .front-matter pre {{ background: transparent !important; }}
	  #preview pre {{ background: #2d2d2d !important; }}
	  #preview code:not(pre code) {{ background: #2d2d2d; }}
	  #preview blockquote {{ border-color: #444; color: #aaa; }}
	  #preview .markdown-alert-note,
	  #preview .markdown-alert-tip,
	  #preview .markdown-alert-important,
	  #preview .markdown-alert-warning,
	  #preview .markdown-alert-caution {{ background: #161b22; color: #d4d4d4; }}
	  #preview .markdown-alert-note {{ border-color: #2f81f7; }}
	  #preview .markdown-alert-tip {{ border-color: #3fb950; }}
	  #preview .markdown-alert-important {{ border-color: #a371f7; }}
	  #preview .markdown-alert-warning {{ border-color: #d29922; }}
	  #preview .markdown-alert-caution {{ border-color: #f85149; }}
	  #preview .markdown-alert-note .markdown-alert-title {{ color: #2f81f7; }}
	  #preview .markdown-alert-tip .markdown-alert-title {{ color: #3fb950; }}
	  #preview .markdown-alert-important .markdown-alert-title {{ color: #a371f7; }}
	  #preview .markdown-alert-warning .markdown-alert-title {{ color: #d29922; }}
	  #preview .markdown-alert-caution .markdown-alert-title {{ color: #f85149; }}
	  #preview table th {{ background: #2d2d2d; color: #f0f0f0; }}
	  #preview table td, #preview table th {{ border-color: #444; }}
	  .mdp-lightbox-canvas {{ background: #1e1e1e; }}
	  #preview hr {{ border-color: #333; }}
	  .toolbar button {{
	    background: rgba(40,40,40,0.8);
    border-color: rgba(255,255,255,0.1);
    color: #bbb;
	  }}
	  .toolbar button:hover {{ color: #fff; background: rgba(55,55,55,1); }}
	  .zoom-popover {{ background: rgba(34,34,34,.96); border-color: rgba(255,255,255,.12); }}
	  .word-popover label {{ color: #ccc; }}
	  .toolbar .word-popover .word-export {{ background: #1f6feb; color: #fff; }}
	  .toolbar .word-popover .word-export:hover {{ background: #1a5cd0; }}
	  .toolbar .zoom-popover button:hover {{ background: rgba(255,255,255,.1); }}
	  .toolbar .update-btn {{ color: #6cb6ff; }}
		  .empty-open {{ background: #242424; border-color: #444; color: #ddd; }}
		  .empty-open:hover {{ background: #2d2d2d; color: #fff; }}
		  .recent-name {{ color: #ddd; }}
		  .recent-item {{ background: #242424; border-color: #333; }}
		  .recent-item:hover {{ background: #2d2d2d; }}
	  .findbar {{ background: rgba(34,34,34,0.96); border-color: rgba(255,255,255,0.1); }}
	  .findbar button:hover {{ background: #333; color: #fff; }}
	  .tabbar {{ background: rgba(28,28,28,.96); border-color: #363636; }}
	  .tab {{ color: #aaa; }}
	  .tab:hover {{ background: rgba(255,255,255,.07); }}
	  .tab.active {{ color: #eee; background: #2c2c2c; border-color: #444; }}
	  .tab.missing {{ color: #e3a04b; }}
	  .tab-close:hover, .tab-open:hover {{ background: rgba(255,255,255,.1); color: #fff; }}
	  .tab-about {{ color: #aaa; }}
	  .tab-about:hover {{ background: rgba(255,255,255,.1); color: #fff; }}
	  .tab-about.open {{ color: #6cb6ff; background: rgba(108,182,255,.12); }}
	  .about-popover {{ background: rgba(34,34,34,.97); border-color: rgba(255,255,255,.12); color: #ccc; }}
	  .about-label {{ color: #888; }}
	  .about-value {{ color: #eee; }}
	  .tab-menu {{ background: rgba(34,34,34,.97); border-color: rgba(255,255,255,.12); }}
	  .tab-menu button {{ color: #ccc; }}
	  .tab-menu button:hover {{ background: rgba(108,182,255,.14); color: #6cb6ff; }}
	  .toolbar .print-popover .print-action {{ color: #ccc; }}
	  .toolbar .print-popover .print-action:hover {{ background: rgba(108,182,255,.14); color: #6cb6ff; }}
	  .missing-file p {{ color: #aaa; }}
	  .missing-actions button {{ background: #292929; border-color: #444; color: #ddd; }}
	  .missing-actions button:hover {{ background: #333; }}
	  #toc-panel {{ background: #232323; border-color: #363636; }}
	  #toc-header {{ color: #777; }}
	  #toc a.toc-item {{ color: #bbb; }}
	  #toc a.toc-item:hover {{ background: rgba(255,255,255,.07); color: #eee; }}
	  #toc a.toc-item.active {{ color: #6cb6ff; }}
	  .toolbar button#btn-toc.on {{ color: #6cb6ff; }}
	  .toolbar button#btn-split.on {{ color: #6cb6ff; background: rgba(108,182,255,.14); }}
	  body.split-view #preview {{ border-color: #363636; }}
	}}

/* Source editor textarea — height is auto-grown by JS to match content,
   so the page (html) owns the only vertical scrollbar. */
#editor {{
  display: none;
  width: 100%;
  box-sizing: border-box;
  border: none; outline: none; resize: none;
  overflow: hidden;
  font: calc(14px * var(--content-scale))/1.6 "SF Mono","Menlo","Consolas",monospace;
  background: transparent; color: inherit;
  padding: 0;
}}
body.editing #preview {{ display: none; }}
body.editing #editor {{ display: block; padding: 16px 24px; }}
body.editing #app {{ max-width: none; padding: 0; }}
body.editing #btn-open,
body.editing #btn-search,
body.editing #print-control {{ display: none; }}

/* Independently scrollable panes share semantic source positions. */
body.split-view {{ overflow: hidden; }}
body.split-view #app {{
  max-width: none; margin: 0; padding: 0;
  display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  position: fixed; top: calc(var(--chrome-top) + 50px); bottom: 0; left: 0; right: 0;
  align-items: stretch;
}}
body.split-view #preview,
body.split-view #editor {{ display: block; min-width: 0; box-sizing: border-box; height: 100% !important; min-height: 0; overflow: auto; }}
body.split-view #preview {{
  max-width: none; margin: 0; padding: 24px;
  border-right: 1px solid #e6e6e6;
}}
body.split-view #editor {{ padding: 16px 24px; }}
body:has(.mdp-lightbox.open) {{ overflow: hidden; }}
.mdp-lightbox {{ overflow: hidden; }}
.mdp-lightbox-body {{ min-width: 0; min-height: 0; box-sizing: border-box; }}
@media (max-width: 600px) {{
  .mdp-lightbox-toolbar {{ flex-wrap: wrap; gap: 4px; padding: 6px; }}
  .mdp-lightbox-pan-hint,.mdp-lightbox-spacer {{ display: none; }}
}}
body.split-view #preview .mdp-table-wrap {{
  width: 100%; margin: 1em 0; transform: none;
}}
body.split-view #btn-toc {{ display: none; }}
@media (max-width: 520px) {{
  body.split-view #preview {{ padding: 12px 10px; }}
  body.split-view #editor {{ padding: 12px 10px; }}
}}

@page {{
  margin: 12mm;
}}

#rich-editor {{ display: none; }}
body.preview-editing #rich-editor {{ display: block; }}
body.preview-editing #preview {{ display: none; }}
#rich-editor .vditor {{ border: 0; background: transparent; width: 100%; max-width: 100%; min-width: 0; }}
#rich-editor .vditor-toolbar {{ position: sticky; top: calc(var(--chrome-top) + 42px); z-index: 80; flex-wrap: wrap; border-radius: 8px; }}
#rich-editor .vditor-wysiwyg pre.vditor-reset {{ padding: 16px 8px !important; font-size: calc(15px * var(--content-scale)); overflow: visible; }}
#rich-editor .vditor-content, #rich-editor .vditor-wysiwyg {{ overflow: visible; min-width: 0; }}
#rich-editor pre.vditor-reset {{ min-width: 0; max-width: 100%; box-sizing: border-box; }}
#rich-editor .vditor-reset table {{ display: block; width: 100%; max-width: 100%; overflow-x: auto; }}
#rich-editor .vditor-reset td, #rich-editor .vditor-reset th {{ overflow-wrap: anywhere; }}
#rich-editor .rich-metadata {{ white-space: pre-wrap; color: #6e7781; border-bottom: 1px solid #ddd; padding-bottom: 12px; }}
::highlight(mdp-find) {{ background: #fff2a8; color: #24292f; }}
::highlight(mdp-current) {{ background: #ffcc4d; color: #24292f; }}
#preview-edit-hint {{ display: none; margin: 50px 16px 0; padding: 8px 12px; font-size: 12px; color: #57606a; background: rgba(9,105,218,.06); }}
body.preview-editing #preview-edit-hint, body.rich-edit-loading #preview-edit-hint, body.rich-edit-error #preview-edit-hint {{ display: block; }}
#preview-edit-hint.error {{ color: #cf222e; }}
#btn-preview-edit.on {{ color: #0969da; background: rgba(9,105,218,.1); }}
@media (prefers-color-scheme: dark) {{ #preview-edit-hint {{ color: #adbac7; }} }}
@media (max-width: 600px) {{
  .toolbar {{ right: 8px; gap: 3px; }}
  .toolbar button {{ width: 30px; height: 30px; }}
  #rich-editor .vditor-panel {{ left: 0 !important; max-width: 100%; box-sizing: border-box; overflow-x: auto; white-space: normal; }}
  #rich-editor .vditor-panel[style*="display: block"] {{ display: flex !important; flex-wrap: wrap; }}
  #rich-editor .vditor-panel > * {{ max-width: 100%; min-width: 0; }}
  #rich-editor .vditor-input {{ max-width: 100%; box-sizing: border-box; }}
}}
@media print {{
  body {{ overflow: visible !important; }}
  #preview-edit-hint {{ display: none !important; }}
  .mdp-edit-text {{ outline: none !important; background: none !important; }}
  .toolbar, .tabbar, #editor, #toc-panel, .tab-menu, .about-popover, .mdp-lightbox {{ display: none !important; }}
  #preview {{ display: block !important; }}
  #app {{ max-width: none; padding: 0; }}
  body.split-view #app {{ display: block !important; position: static; height: auto; }}
  body.split-view #preview {{ height: auto !important; overflow: visible; }}
  body.toc-open.has-toc #app {{ padding-left: 0; }}
  /* Wide tables must shrink to the page, not clip: force a fixed 100%
     layout and let every cell wrap. */
  #preview .mdp-table-wrap {{ width: 100% !important; margin: 1em 0 !important; transform: none !important; overflow: visible !important; }}
  #preview .mdp-table-wrap table, #preview table {{ width: 100% !important; table-layout: fixed; }}
  #preview table th {{ white-space: normal !important; }}
  #preview table td {{ min-width: 0 !important; max-width: none !important; word-break: break-word; overflow-wrap: anywhere; }}
  #preview pre {{ white-space: pre-wrap !important; word-break: break-all; overflow-wrap: anywhere; }}
  #preview .katex-display {{ overflow: visible; }}
}}
	</style></head><body class="{body_class}">
	<div class="tabbar" id="tabbar"><div class="tabs" id="tabs"></div><div class="doc-stats" id="doc-stats" aria-live="polite"></div><button class="tab-open" id="tab-open" type="button" title="{btn_new}" aria-label="{btn_new}">+</button><button class="tab-about" id="tab-about" type="button" title="{btn_about}" aria-label="{btn_about}">!</button><div class="about-popover" id="about-popover"><div class="about-row"><span class="about-label">{about_developer}</span><span class="about-value">{developer_name}</span></div><div class="about-row"><span class="about-label">{about_version}</span><span class="about-value">v{app_version}</span></div><div class="about-row"><span class="about-label">{about_updated}</span><span class="about-value">{release_date}</span></div></div></div>
	<div id="tab-menu" class="tab-menu" hidden><button id="tab-menu-reveal" type="button">{tab_reveal}</button></div>
	<div class="toolbar">
	  <button id="btn-toc" title="{btn_toc}" aria-label="{btn_toc}"></button>
	  <button id="btn-open" title="{btn_open}" aria-label="{btn_open}"></button>
	  <button id="btn-search" title="{btn_search}" aria-label="{btn_search}"></button>
	  <button id="btn-toggle" title="{btn_edit}" aria-label="{btn_edit}"></button>
	  <button id="btn-split" type="button" title="{btn_split}" aria-label="{btn_split}" aria-pressed="false"></button>
	  <button id="btn-preview-edit" type="button" title="{btn_preview_edit}" aria-label="{btn_preview_edit}" aria-pressed="false">✎</button>
	  <div class="zoom-control" id="print-control">
	    <button id="btn-print" title="{btn_print}" aria-label="{btn_print}"></button>
	    <div class="zoom-popover print-popover">
	      <button id="print-direct" class="print-action" type="button">{print_now}</button>
	      <button id="pdf-export" class="print-action" type="button">{pdf_export}</button>
	    </div>
	  </div>
	  <div class="zoom-control" id="zoom-control">
	    <button id="btn-zoom" title="{btn_zoom}" aria-label="{btn_zoom}"></button>
	    <div class="zoom-popover">
	      <button id="btn-zoom-out" title="{btn_zoom_out}" aria-label="{btn_zoom_out}">−</button>
	      <button id="btn-zoom-reset" class="zoom-reset" title="{btn_zoom_reset}" aria-label="{btn_zoom_reset}">100%</button>
	      <button id="btn-zoom-in" title="{btn_zoom_in}" aria-label="{btn_zoom_in}">+</button>
	    </div>
	  </div>
	  <button id="btn-save-as" title="{btn_save_as}" aria-label="{btn_save_as}"></button>
	  <div class="zoom-control" id="word-control">
	    <button id="btn-word" title="{btn_word}" aria-label="{btn_word}"></button>
	    <div class="zoom-popover word-popover">
	      <label><input type="checkbox" id="word-standardize"> <span>{export_standardize}</span></label>
	      <label><input type="checkbox" id="word-strip"> <span>{export_strip_numbers}</span></label>
	      <button id="word-export" class="word-export" type="button">{export_now}</button>
	    </div>
	  </div>
	  <button id="btn-update" class="update-btn" hidden title="{btn_update}" aria-label="{btn_update}"></button>
	</div>
	<div class="findbar" role="search">
	  <div class="find-row">
	    <input id="find-input" type="search" placeholder="{search_placeholder}" aria-label="{search_placeholder}">
	    <span id="find-state" aria-live="polite"></span>
	    <button id="find-prev" title="Previous" aria-label="Previous"></button>
	    <button id="find-next" title="Next" aria-label="Next"></button>
	    <button id="find-close" title="Close" aria-label="Close"></button>
	  </div>
	  <div class="replace-row">
	    <input id="replace-input" type="text" placeholder="{replace_placeholder}" aria-label="{replace_placeholder}">
	    <button id="replace-one" class="replace-action" type="button" title="{replace_one}" aria-label="{replace_one}">{replace_one}</button>
	    <button id="replace-all" class="replace-action" type="button" title="{replace_all}" aria-label="{replace_all}">{replace_all}</button>
	  </div>
	</div>
	<aside id="toc-panel" aria-label="{toc_title}"><div id="toc-header">{toc_title}</div><nav id="toc"></nav></aside>
	<div id="preview-edit-hint" role="status">{preview_edit_hint}</div>
	<div id="app">
  <div id="preview">{preview_html}</div>
  <div id="rich-editor"></div>
  <textarea id="editor" spellcheck="false">{raw_md_escaped}</textarea>
</div>
<script>
(function(){{
	  var ICON_EDIT = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"/></svg>';
	  var ICON_VIEW = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/></svg>';
	  var ICON_SPLIT = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M12 4v16"/></svg>';
	  var ICON_OPEN = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 14 1.45-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.55 6A2 2 0 0 1 18.45 20H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2"/></svg>';
	  var ICON_SEARCH = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/></svg>';
	  var ICON_PRINT = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 6 2 18 2 18 9"/><path d="M6 18H4a2 2 0 0 1-2-2v-5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2h-2"/><rect x="6" y="14" width="12" height="8"/></svg>';
	  var ICON_ZOOM = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/><path d="M8 11h6"/><path d="M11 8v6"/></svg>';
	  var ICON_UP = '<svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m18 15-6-6-6 6"/></svg>';
	  var ICON_DOWN = '<svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>';
	  var ICON_CLOSE = '<svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>';
	  var ICON_TOC = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 6h16"/><path d="M4 12h10"/><path d="M4 18h14"/></svg>';
	  var ICON_SAVE_AS = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2z"/><polyline points="17 21 17 13 7 13 7 21"/><polyline points="7 3 7 8 15 8"/></svg>';
	  var ICON_WORD = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><path d="M8.5 12l1.5 5 1.7-3.8L13.4 17l1.6-5"/></svg>';
	  var L_EDIT = '{btn_edit}', L_VIEW = '{btn_preview}';
	  var L_SPLIT = '{btn_split}', L_SPLIT_EXIT = '{btn_split_exit}';

	  var btnOpen = document.getElementById('btn-open');
	  var btnSearch = document.getElementById('btn-search');
	  var btnToggle = document.getElementById('btn-toggle');
	  var btnSplit = document.getElementById('btn-split');
	  var btnPrint = document.getElementById('btn-print');
	  var btnZoom = document.getElementById('btn-zoom');
	  var btnZoomOut = document.getElementById('btn-zoom-out');
	  var btnZoomReset = document.getElementById('btn-zoom-reset');
	  var btnZoomIn = document.getElementById('btn-zoom-in');
	  var zoomControl = document.getElementById('zoom-control');
	  var btnUpdate = document.getElementById('btn-update');
	  var findInput = document.getElementById('find-input');
	  var findState = document.getElementById('find-state');
	  var findPrev = document.getElementById('find-prev');
	  var findNext = document.getElementById('find-next');
	  var findClose = document.getElementById('find-close');
	  var replaceInput = document.getElementById('replace-input');
	  var replaceOne = document.getElementById('replace-one');
	  var replaceAll = document.getElementById('replace-all');
	  var tabsEl = document.getElementById('tabs');
	  var docStats = document.getElementById('doc-stats');
	  var tabOpen = document.getElementById('tab-open');
	  var btnToc = document.getElementById('btn-toc');
	  var tocPanel = document.getElementById('toc-panel');
	  var tocNav = document.getElementById('toc');
	  var btnSaveAs = document.getElementById('btn-save-as');
	  var btnWord = document.getElementById('btn-word');
	  var wordControl = document.getElementById('word-control');
	  var wordStandardize = document.getElementById('word-standardize');
	  var wordStrip = document.getElementById('word-strip');
	  var wordExport = document.getElementById('word-export');
	  var tabAbout = document.getElementById('tab-about');
	  var aboutPopover = document.getElementById('about-popover');
	  var tabMenu = document.getElementById('tab-menu');
	  var tabMenuReveal = document.getElementById('tab-menu-reveal');
	  var printControl = document.getElementById('print-control');
	  var printDirect = document.getElementById('print-direct');
	  var pdfExport = document.getElementById('pdf-export');
	  var tabMenuTabId = 0;
	  var WORD_OPTS_KEY = 'md-preview-word-export-opts-v1';
	  var tocActiveLink = null;
	  var tocSpyScheduled = false;
	  var TOC_STORAGE_KEY = 'md-preview-toc-open-v1';
	  var ta = document.getElementById('editor');
	  var previewEditor = null;
      var splitSync = window.__mdPreviewInstallSplitSync ? window.__mdPreviewInstallSplitSync({{ editor: ta, preview: document.getElementById('preview') }}) : null;
	  var richBackupRequests = new Map(), richBackupSeq = 0;
	  var sourcePreviewSeq = 0, pendingSourcePreview = null;
	  window.__mdPreviewRichEditPrepared = function(token, ok, detail) {{
	    var request = richBackupRequests.get(token);
	    if (!request) return;
	    richBackupRequests.delete(token);
	    if (ok) request.resolve(detail); else request.reject(new Error(detail));
	  }};
      function prepareStructuredEdit() {{
        return new Promise(function(resolve, reject) {{
          var token = ++richBackupSeq;
          richBackupRequests.set(token, {{ resolve: resolve, reject: reject }});
          window.ipc.postMessage('prepare-rich-edit:' + token);
        }});
      }}
	  function renderCurrentSource(done) {{
	    if (previewEditor) previewEditor.disable();
	    var token = ++sourcePreviewSeq;
    pendingSourcePreview = {{ token: token, raw: ta.value, tab: activeTabId, done: done }};
	    window.ipc.postMessage('render-source:' + token + '\n' + ta.value);
	  }}
	  window.__mdPreviewSourceRendered = function(token, html, raw, math, mermaid) {{
	    var request = pendingSourcePreview;
    if (!request || request.token !== token || request.tab !== activeTabId || request.raw !== ta.value || request.raw !== raw) return;
	    pendingSourcePreview = null;
	    window.__setPreview(html, math, mermaid, raw);
	    if (!request.done) return;
	    var started = Date.now();
	    function finish() {{
	      var pending = document.querySelector('#preview pre > code.language-mermaid, #preview .mdp-mermaid:not(:has(svg))');
	      if (!pending) {{ request.done(); return; }}
      if (Date.now() - started < 15000) {{ setTimeout(finish, 50); return; }}
      document.body.classList.add('rich-edit-error');
      document.getElementById('preview-edit-hint').textContent = '{preview_render_failed}';
	    }}
	    setTimeout(finish, 50);
	  }};
	  var dirty = false;
	  var activeTabId = 0;
	  var activeTabName = '';
	  var pendingAutosaveTimer = 0;
	  var autosavePaused = false;
	  var applyingReplacement = false;
	  var editorInputRevision = 0;
	  var editorComposing = false;
	  var editorImeFocusPrimed = false;
	  var WINDOWS_IME_FOCUS_FIX = {windows_ime_focus_fix};
	  var AUTOSAVE_DEBOUNCE_MS = 700;
	  var composingFind = false;
	  var pendingFindTimer = 0;
	  var FIND_DEBOUNCE_MS = 300;
	  var findHits = [];
	  var currentFindHit = -1;
	  var lastFindQuery = '';
	  var replaceMatches = [];
	  var currentReplaceHit = -1;
	  var lastReplaceQuery = '';
	  var lastReplaceSource = '';
	  var replaceStatus = '';
	  var REPLACE_DONE = '{replace_done_js}';
	  var STAT_WORDS = '{stat_words_js}';
	  var STAT_CHARS = '{stat_chars_js}';
	  var ZOOM_STORAGE_KEY = 'md-preview-content-zoom-v1';
	  var ZOOM_MIN = 70;
	  var ZOOM_MAX = 200;
	  var ZOOM_STEP = 10;
	  var zoomPercent = 100;

	  btnOpen.innerHTML = ICON_OPEN;
	  btnSearch.innerHTML = ICON_SEARCH;
	  btnToggle.innerHTML = ICON_EDIT;
	  btnSplit.innerHTML = ICON_SPLIT;
	  btnPrint.innerHTML = ICON_PRINT;
	  btnZoom.innerHTML = ICON_ZOOM;
	  btnUpdate.innerHTML = '<span class="update-mark">↻</span><span class="update-label">{btn_update}</span>';
	  findPrev.innerHTML = ICON_UP;
	  findNext.innerHTML = ICON_DOWN;
	  findClose.innerHTML = ICON_CLOSE;
	  replaceOne.disabled = true;
	  replaceAll.disabled = true;
	  btnToc.innerHTML = ICON_TOC;
	  btnSaveAs.innerHTML = ICON_SAVE_AS;
	  btnWord.innerHTML = ICON_WORD;

  btnSaveAs.addEventListener('click', function() {{
    if (previewEditor) previewEditor.flush();
    window.ipc.postMessage('save-as\n' + ta.value);
  }});
  btnWord.addEventListener('click', function(e) {{
    e.stopPropagation();
    wordControl.classList.toggle('open');
  }});
  try {{
    var savedWordOpts = localStorage.getItem(WORD_OPTS_KEY);
    if (savedWordOpts) {{
      wordStandardize.checked = savedWordOpts.charAt(0) === '1';
      wordStrip.checked = savedWordOpts.charAt(1) === '1';
    }}
  }} catch (_) {{}}
  // Render ```mermaid fenced blocks to PNG data URLs so the Word export
  // embeds real diagrams instead of source text. Blocks that fail to render
  // (or when mermaid is not loaded) keep their code-block form.
  function exportMermaidToImages(md) {{
    if (!window.mermaid || !window.mermaid.render) return Promise.resolve(md);
    var fence = /(^|\n)```mermaid[ \t]*\r?\n([\s\S]*?)\n?```/g;
    var jobs = [];
    md.replace(fence, function(match, lead, source) {{
      jobs.push({{ match: match, lead: lead, source: source }});
      return match;
    }});
    if (!jobs.length) return Promise.resolve(md);
    try {{
      window.mermaid.initialize({{ startOnLoad: false, securityLevel: 'strict', theme: 'default' }});
    }} catch (_) {{}}
    var seq = 0;
    var chain = Promise.resolve(md);
    jobs.forEach(function(job) {{
      chain = chain.then(function(current) {{
        var id = 'mdp-export-mermaid-' + (++seq) + '-' + Date.now();
        return Promise.resolve(window.mermaid.render(id, job.source)).then(function(result) {{
          var svg = result && result.svg ? result.svg : String(result || '');
          return svgToPngDataUrl(svg).then(function(image) {{
            if (!image) return current;
            var logicalSize = Math.max(1, Math.round(image.logicalWidth)) + 'x' +
              Math.max(1, Math.round(image.logicalHeight));
            return current.replace(
              job.match,
              job.lead + '\n![Mermaid diagram](' + image.dataUrl +
                ' "mdp-logical-size=' + logicalSize + '")\n'
            );
          }});
        }}).catch(function() {{ return current; }});
      }});
    }});
    return chain;
  }}

  // Mermaid emits HTML inside <foreignObject> labels (e.g. a bare <br>),
  // which is not well-formed XML; browsers then refuse to load the SVG as an
  // <img>. Round-trip through the live DOM so the HTML parser + XMLSerializer
  // normalize it into valid XML before rasterizing.
  function normalizeSvgForImage(svg, width, height) {{
    try {{
      if (window.__mdPreviewPrepareSvgForExport && width && height) {{
        var prepared = window.__mdPreviewPrepareSvgForExport(svg, width, height);
        if (prepared) return prepared;
      }}
      var host = document.createElement('div');
      host.innerHTML = svg;
      var el = host.querySelector('svg');
      if (el && width && height) {{
        el.setAttribute('xmlns', 'http://www.w3.org/2000/svg');
        el.setAttribute('width', String(Math.max(1, Math.round(width))));
        el.setAttribute('height', String(Math.max(1, Math.round(height))));
        el.style.width = Math.max(1, Math.round(width)) + 'px';
        el.style.height = Math.max(1, Math.round(height)) + 'px';
        el.style.maxWidth = 'none';
        el.style.maxHeight = 'none';
      }}
      if (el) return new XMLSerializer().serializeToString(el);
    }} catch (_) {{}}
    return svg;
  }}

  function svgToPngDataUrl(svg) {{
    svg = normalizeSvgForImage(svg, 0, 0);
    return new Promise(function(resolve) {{
      var w = 0, h = 0;
      var vb = /viewBox="([^"]+)"/.exec(svg);
      if (vb) {{
        var parts = vb[1].trim().split(/[\s,]+/);
        w = parseFloat(parts[2]); h = parseFloat(parts[3]);
      }}
      if (!w || !h) {{
        var wm = /width="([\d.]+)(?:px)?"/.exec(svg);
        var hm = /height="([\d.]+)(?:px)?"/.exec(svg);
        if (wm && hm) {{ w = parseFloat(wm[1]); h = parseFloat(hm[1]); }}
      }}
      if (!w || !h || w > 16000 || h > 16000) {{ resolve(null); return; }}
      var scale = 300 / 96; // 300 PPI at the SVG's 96-CSS-pixel logical size.
      var targetW = Math.max(1, Math.ceil(w * scale));
      var targetH = Math.max(1, Math.ceil(h * scale));
      var cap = Math.min(1, 8192 / targetW, 8192 / targetH,
        Math.sqrt(32000000 / (targetW * targetH)));
      if (cap < 1) {{
        targetW = Math.max(1, Math.floor(targetW * cap));
        targetH = Math.max(1, Math.floor(targetH * cap));
      }}
      var rasterSvg = normalizeSvgForImage(svg, targetW, targetH);
      if (!rasterSvg) {{ resolve(null); return; }}
      var img = new Image();
      img.onload = function() {{
        try {{
          var canvas = document.createElement('canvas');
          canvas.width = targetW;
          canvas.height = targetH;
          var ctx = canvas.getContext('2d');
          ctx.fillStyle = '#ffffff';
          ctx.fillRect(0, 0, canvas.width, canvas.height);
          ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
          resolve({{
            dataUrl: canvas.toDataURL('image/png'),
            logicalWidth: w,
            logicalHeight: h,
            pixelWidth: targetW,
            pixelHeight: targetH
          }});
        }} catch (_) {{ resolve(null); }}
      }};
      img.onerror = function() {{ resolve(null); }};
      img.src = 'data:image/svg+xml;base64,' + btoa(unescape(encodeURIComponent(rasterSvg)));
    }});
  }}

  wordExport.addEventListener('click', function() {{
    if (previewEditor) previewEditor.flush();
    var flags = (wordStandardize.checked ? '1' : '0') + (wordStrip.checked ? '1' : '0');
    try {{ localStorage.setItem(WORD_OPTS_KEY, flags); }} catch (_) {{}}
    wordControl.classList.remove('open');
    exportMermaidToImages(ta.value).then(function(content) {{
      window.ipc.postMessage('export-docx\n' + flags + '\n' + content);
    }});
  }});
  // Exposed for automated verification of the mermaid->PNG export path.
  window.__mdPreviewExportMermaidToImages = exportMermaidToImages;
  document.addEventListener('click', function(e) {{
    if (!wordControl.contains(e.target)) wordControl.classList.remove('open');
  }});

  function loadTocOpen() {{
    try {{ return localStorage.getItem(TOC_STORAGE_KEY) === '1'; }} catch (_) {{ return false; }}
  }}
  function setTocOpen(open, persist) {{
    document.body.classList.toggle('toc-open', open);
    btnToc.classList.toggle('on', open);
    if (persist) {{ try {{ localStorage.setItem(TOC_STORAGE_KEY, open ? '1' : '0'); }} catch (_) {{}} }}
  }}
  function buildToc() {{
    var readingRoot = previewEditor && previewEditor.active() ? previewEditor.root() : document.getElementById('preview');
    var headings = readingRoot.querySelectorAll('h1[id], h2[id], h3[id], h4[id], h5[id], h6[id]');
    tocNav.textContent = '';
    tocActiveLink = null;
    if (!headings.length) {{
      document.body.classList.remove('has-toc');
      btnToc.hidden = true;
      return;
    }}
    btnToc.hidden = false;
    var minLevel = 6;
    headings.forEach(function(h) {{
      var lvl = h.tagName.charCodeAt(1) - 48;
      if (lvl < minLevel) minLevel = lvl;
    }});
    var frag = document.createDocumentFragment();
    headings.forEach(function(h) {{
      var lvl = h.tagName.charCodeAt(1) - 48;
      var a = document.createElement('a');
      a.className = 'toc-item';
      a.textContent = h.textContent;
      a.title = h.textContent;
      a.setAttribute('data-target', h.id);
      a.setAttribute('data-depth', Math.min(lvl - minLevel, 5));
      frag.appendChild(a);
    }});
    tocNav.appendChild(frag);
    document.body.classList.add('has-toc');
    updateTocActive();
  }}
  function updateTocActive() {{
    if (!document.body.classList.contains('has-toc')) return;
    var links = tocNav.querySelectorAll('.toc-item');
    if (!links.length) return;
    var scrollY = window.scrollY || document.documentElement.scrollTop || 0;
    var active = null;
    for (var i = 0; i < links.length; i++) {{
      var el = document.getElementById(links[i].getAttribute('data-target'));
      if (!el) continue;
      if (el.getBoundingClientRect().top + scrollY <= scrollY + 88) active = links[i];
      else break;
    }}
    if (active === tocActiveLink) return;
    if (tocActiveLink) tocActiveLink.classList.remove('active');
    tocActiveLink = active;
    if (active) {{
      active.classList.add('active');
      var panelRect = tocPanel.getBoundingClientRect();
      var linkRect = active.getBoundingClientRect();
      if (linkRect.top < panelRect.top + 8 || linkRect.bottom > panelRect.bottom - 8) {{
        tocPanel.scrollTop += linkRect.top - panelRect.top - panelRect.height / 2;
      }}
    }}
  }}
  tocNav.addEventListener('click', function(e) {{
    var link = e.target && e.target.closest ? e.target.closest('.toc-item') : null;
    if (!link) return;
    e.preventDefault();
    var el = document.getElementById(link.getAttribute('data-target'));
    if (!el) return;
    var scrollY = window.scrollY || document.documentElement.scrollTop || 0;
    window.scrollTo(0, el.getBoundingClientRect().top + scrollY - 64);
  }});
  window.addEventListener('scroll', function() {{
    if (tocSpyScheduled) return;
    tocSpyScheduled = true;
    requestAnimationFrame(function() {{
      tocSpyScheduled = false;
      updateTocActive();
    }});
  }}, {{ passive: true }});
  btnToc.addEventListener('click', function() {{
    setTocOpen(!document.body.classList.contains('toc-open'), true);
  }});

  function inSplit() {{ return document.body.classList.contains('split-view'); }}
  function inEditOnly() {{ return document.body.classList.contains('editing'); }}
  function inEdit() {{ return inEditOnly() || inSplit(); }}
  function textSegments(text) {{
    if (typeof Intl !== 'undefined' && Intl.Segmenter) {{
      var segmenter = new Intl.Segmenter(undefined, {{ granularity: 'grapheme' }});
      return Array.from(segmenter.segment(text), function(item) {{ return item.segment; }});
    }}
    return Array.from(text);
  }}
  function updateDocumentStats(raw) {{
    var segments = textSegments(String(raw || ''));
    var words = segments.reduce(function(total, segment) {{
      return total + (/^\s+$/u.test(segment) ? 0 : 1);
    }}, 0);
    var number = new Intl.NumberFormat().format;
    docStats.textContent =
      number(words) + ' ' + STAT_WORDS + ' · ' + number(segments.length) + ' ' + STAT_CHARS;
  }}
  function loadZoomPercent() {{
    try {{
      var stored = Number(localStorage.getItem(ZOOM_STORAGE_KEY));
      if (Number.isFinite(stored) && stored >= ZOOM_MIN && stored <= ZOOM_MAX) return stored;
    }} catch (_) {{}}
    return 100;
  }}
  function applyZoom(percent, persist) {{
    zoomPercent = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, percent));
    document.documentElement.style.setProperty('--content-scale', String(zoomPercent / 100));
    btnZoomReset.textContent = zoomPercent + '%';
    if (persist) {{
      try {{ localStorage.setItem(ZOOM_STORAGE_KEY, String(zoomPercent)); }} catch (_) {{}}
    }}
    if (inEdit()) autoResize();
  }}
  function changeZoom(delta) {{
    applyZoom(zoomPercent + delta, true);
  }}
  function currentScrollProgress() {{
    var root = document.documentElement;
    var max = Math.max(root.scrollHeight - window.innerHeight, 0);
    var y = window.scrollY || root.scrollTop || 0;
    return max > 0 ? Math.min(1, Math.max(0, y / max)) : 0;
  }}
  function restoreScrollProgress(progress) {{
    function restore() {{
      var max = Math.max(document.documentElement.scrollHeight - window.innerHeight, 0);
      window.scrollTo(window.scrollX || 0, max * progress);
    }}
    restore();
    requestAnimationFrame(function() {{
      restore();
      requestAnimationFrame(restore);
    }});
  }}
  applyZoom(loadZoomPercent(), false);
  updateDocumentStats(ta.value);
  setTocOpen(loadTocOpen(), false);
  buildToc();
  document.addEventListener('contextmenu', function(e) {{
    if (previewEditor && previewEditor.active() && previewEditor.root().contains(e.target)) return;
    if (!inEdit() || e.target !== ta) e.preventDefault();
  }});
  function setDirty(d) {{
    if (dirty === d) return;
    dirty = d;
    window.ipc.postMessage(d ? 'dirty:1' : 'dirty:0');
  }}
	  function handleEditorInput(eventOrReplacement) {{
	    editorInputRevision += 1;
	    var replacementChange = eventOrReplacement === true || applyingReplacement;
	    var composing = editorComposing || !!(eventOrReplacement && eventOrReplacement.isComposing);
	    setDirty(true);
	    if (!composing) scheduleAutosave();
	    autoResize();
	    updateDocumentStats(ta.value);
	    if (previewEditor && previewEditor.active() && document.body.classList.contains('finding')) {{ clearFindHits(); scheduleFind(); }}
	    if (inReplaceMode() && !replacementChange) {{
	      clearReplaceMatches(true);
	      updateFindState();
	      scheduleFind();
	    }}
	  }}
	  function cancelPendingAutosave() {{
	    if (!pendingAutosaveTimer) return;
	    clearTimeout(pendingAutosaveTimer);
	    pendingAutosaveTimer = 0;
	  }}
	  function save() {{
	    if (previewEditor) previewEditor.flush(true);
	    cancelPendingAutosave();
	    if (!dirty && !window.__mdPreviewIsUntitled) {{
	      // Nothing to save: acknowledge so a pending window-close can proceed
	      // instead of being silently swallowed.
	      window.ipc.postMessage('quit-ack');
	      return;
	    }}
	    window.ipc.postMessage('save:' + ta.value);
	  }}
	  // Flush edits without forcing a Save As dialog: untitled tabs keep their
	  // content in Rust-side memory until the user explicitly saves.
	  function flushEdit() {{
	    if (previewEditor) previewEditor.flush();
	    cancelPendingAutosave();
	    if (!dirty) return;
	    if (window.__mdPreviewIsUntitled) {{
	      window.ipc.postMessage('store-content\n' + ta.value);
	    }} else {{
	      window.ipc.postMessage('save:' + ta.value);
	    }}
	  }}
	  function scheduleAutosave() {{
	    cancelPendingAutosave();
	    if (autosavePaused) return;
	    pendingAutosaveTimer = setTimeout(function() {{
	      pendingAutosaveTimer = 0;
	      if (dirty) flushEdit();
	    }}, AUTOSAVE_DEBOUNCE_MS);
	  }}
	  window.__mdPreviewSave = save;
	  // Untitled scratch tabs: load in-memory content straight into the editor.
	  window.__mdPreviewLoadUntitled = function(content, isDirty, previewHtml, needsMath, needsMermaid) {{
	    if (previewEditor) previewEditor.reset();
	    document.body.classList.remove('empty');
	    document.body.classList.remove('missing');
	    hideFind();
	    window.__setBaseHref('');
	    window.__mdPreviewIsUntitled = true;
	    ta.value = content || '';
	    window.__setPreview(previewHtml || '', !!needsMath, !!needsMermaid, ta.value);
	    updateDocumentStats(ta.value);
	    dirty = !!isDirty; // quiet: Rust already tracks the tab's dirty state
	    if (!inEdit()) enterEdit(); else autoResize();
	  }};
	  window.__mdPreviewSetDirtyQuiet = function(d) {{ dirty = !!d; }};
	  function requestTabAction(action, id) {{
	    if (previewEditor) previewEditor.flush(true);
	    cancelPendingAutosave();
	    var message = 'tab-action:' + action + ':' + id;
	    if (dirty) message += '\n' + ta.value;
	    window.ipc.postMessage(message);
	  }}
	  function openFile() {{
	    if (previewEditor && previewEditor.active()) {{ previewEditor.disable(); flushEdit(); }}
	    if (inEdit()) leaveEdit();
	    window.ipc.postMessage('open');
	  }}
	  window.__mdPreviewOpenFile = openFile;
	  function newFile() {{
	    if (previewEditor && previewEditor.active()) {{ previewEditor.disable(); flushEdit(); }}
	    if (inEdit()) leaveEdit();
	    window.ipc.postMessage('new-file');
	  }}
	  window.__mdPreviewNewFile = newFile;
	  function showFind() {{
        if (previewEditor && previewEditor.active()) previewEditor.remember_selection();
	    if (document.body.classList.contains('empty')) return;
	    if (inEditOnly() || (inSplit() && document.activeElement === ta)) return;
	    document.body.classList.remove('replacing');
	    replaceInput.value = '';
	    clearReplaceMatches(true);
	    document.body.classList.add('finding');
	    updateFindState();
	    setTimeout(function(){{ findInput.focus(); findInput.select(); }}, 0);
	  }}
	  window.__mdPreviewShowFind = showFind;
	  function showReplace() {{
	    if (document.body.classList.contains('empty') || document.body.classList.contains('missing')) return;
	    var seed = '';
	    var anchor = ta.selectionStart || 0;
	    if (inEdit() && ta.selectionEnd > ta.selectionStart) {{
	      seed = ta.value.slice(ta.selectionStart, ta.selectionEnd);
	      anchor = ta.selectionStart;
	    }} else if (!inEdit() && window.getSelection) {{
	      seed = String(window.getSelection() || '');
	    }}
	    if (pendingFindTimer) {{ clearTimeout(pendingFindTimer); pendingFindTimer = 0; }}
	    clearFindHits();
	    if (!inEdit()) enterEdit();
	    document.body.classList.add('finding');
	    document.body.classList.add('replacing');
	    replaceStatus = '';
	    clearReplaceMatches(false);
	    if (!findInput.value && seed && seed.length <= 200 && !/[\r\n]/.test(seed)) {{
	      findInput.value = seed;
	    }}
	    runReplaceQuery(findInput.value, anchor);
	    setTimeout(function() {{ findInput.focus(); findInput.select(); }}, 0);
	  }}
	  window.__mdPreviewShowReplace = showReplace;
	  function hideFind(restoreEditorFocus) {{
	    var wasReplacing = document.body.classList.contains('replacing');
	    document.body.classList.remove('finding');
	    document.body.classList.remove('replacing');
	    findInput.value = '';
	    replaceInput.value = '';
	    clearFindHits();
	    clearReplaceMatches(true);
	    updateFindState();
	    if (pendingFindTimer) {{ clearTimeout(pendingFindTimer); pendingFindTimer = 0; }}
	    var sel = window.getSelection && window.getSelection();
	    if (sel && sel.removeAllRanges) sel.removeAllRanges();
	    if (wasReplacing && inEdit() && restoreEditorFocus !== false) {{
	      setTimeout(function() {{ if (inEdit()) ta.focus(); }}, 0);
	    }}
        if (previewEditor && previewEditor.active() && restoreEditorFocus !== false) previewEditor.restore_selection();
	  }}
	  function inReplaceMode() {{
	    return document.body.classList.contains('replacing');
	  }}
	  function updateFindState() {{
	    if (inReplaceMode()) {{
	      if (replaceStatus) {{
	        findState.textContent = replaceStatus;
	      }} else if (!findInput.value) {{
	        findState.textContent = '';
	      }} else {{
	        findState.textContent = replaceMatches.length
	          ? (currentReplaceHit + 1) + '/' + replaceMatches.length
	          : '0/0';
	      }}
	      var enabled = replaceMatches.length > 0 && !replaceStatus;
	      replaceOne.disabled = !enabled;
	      replaceAll.disabled = !enabled;
	      return;
	    }}
	    findState.textContent = findHits.length ? (currentFindHit + 1) + '/' + findHits.length : '';
	    replaceOne.disabled = true;
	    replaceAll.disabled = true;
	  }}
	  function clearFindHits() {{
	    findHits.forEach(function(mark) {{
	      if (mark instanceof Range) return;
	      var parent = mark.parentNode;
	      if (!parent) return;
	      parent.replaceChild(document.createTextNode(mark.textContent), mark);
	      parent.normalize();
	    }});
	    if (window.CSS && CSS.highlights) {{ CSS.highlights.delete('mdp-find'); CSS.highlights.delete('mdp-current'); }}
	    findHits = [];
	    currentFindHit = -1;
	    lastFindQuery = '';
	    updateFindState();
	  }}
	  function clearReplaceMatches(clearStatus) {{
	    replaceMatches = [];
	    currentReplaceHit = -1;
	    lastReplaceQuery = '';
	    lastReplaceSource = '';
	    if (clearStatus) replaceStatus = '';
	  }}
	  function focusFindInput(selectionStart, selectionEnd) {{
	    if (!document.body.classList.contains('finding')) return;
	    try {{
	      findInput.focus({{ preventScroll: true }});
	    }} catch (_) {{
	      findInput.focus();
	    }}
	    if (typeof selectionStart === 'number' && typeof selectionEnd === 'number') {{
	      try {{ findInput.setSelectionRange(selectionStart, selectionEnd); }} catch (_) {{}}
	    }}
	  }}
	  function restoreFindInput(selectionStart, selectionEnd) {{
	    setTimeout(function() {{ focusFindInput(selectionStart, selectionEnd); }}, 0);
	    requestAnimationFrame(function() {{
	      focusFindInput(selectionStart, selectionEnd);
	      setTimeout(function() {{ focusFindInput(selectionStart, selectionEnd); }}, 80);
	    }});
	  }}
	  function selectFindHit(index) {{
	    if (!findHits.length) {{
	      currentFindHit = -1;
	      updateFindState();
	      return;
	    }}
	    if (currentFindHit >= 0 && findHits[currentFindHit] && !(findHits[currentFindHit] instanceof Range)) {{
	      findHits[currentFindHit].classList.remove('current');
	    }}
	    currentFindHit = (index + findHits.length) % findHits.length;
	    var hit = findHits[currentFindHit];
	    if (hit instanceof Range) {{
	      if (window.CSS && CSS.highlights) CSS.highlights.set('mdp-current', new Highlight(hit));
	      hit.startContainer.parentElement.scrollIntoView({{ block: 'center', inline: 'nearest' }});
	      updateFindState(); return;
	    }}
	    hit.classList.add('current');
	    hit.scrollIntoView({{ block: 'center', inline: 'nearest' }});
	    updateFindState();
	  }}
	  function runFindQuery(query) {{
	    clearFindHits();
	    query = String(query || '').trim();
	    if (!query) return;
	    lastFindQuery = query;
	    var needle = query.toLowerCase();
	    var previewEl = document.getElementById('preview');
	    var richSearch = previewEditor && previewEditor.active();
	    if (richSearch) previewEl = previewEditor.root();
	    var walker = document.createTreeWalker(previewEl, NodeFilter.SHOW_TEXT, {{
	      acceptNode: function(node) {{
	        if (!node.nodeValue || node.nodeValue.toLowerCase().indexOf(needle) < 0) {{
	          return NodeFilter.FILTER_REJECT;
	        }}
	        var parent = node.parentElement;
	        if (!parent || parent.closest('script,style,svg,mark.search-hit,.katex,.mdp-mermaid')) {{
	          return NodeFilter.FILTER_REJECT;
	        }}
	        return NodeFilter.FILTER_ACCEPT;
	      }}
	    }});
	    var nodes = [];
	    while (walker.nextNode()) nodes.push(walker.currentNode);
	    nodes.forEach(function(node) {{
	      var text = node.nodeValue;
	      var lower = text.toLowerCase();
	      var fragment = document.createDocumentFragment();
	      var start = 0;
	      var index;
	      while ((index = lower.indexOf(needle, start)) >= 0) {{
	        if (richSearch) {{
	          var range = document.createRange(); range.setStart(node, index); range.setEnd(node, index + query.length);
	          findHits.push(range); start = index + query.length; continue;
	        }}
	        if (index > start) fragment.appendChild(document.createTextNode(text.slice(start, index)));
	        var mark = document.createElement('mark');
	        mark.className = 'search-hit';
	        mark.textContent = text.slice(index, index + query.length);
	        findHits.push(mark);
	        fragment.appendChild(mark);
	        start = index + query.length;
	      }}
	      if (start < text.length) fragment.appendChild(document.createTextNode(text.slice(start)));
	      if (!richSearch) node.parentNode.replaceChild(fragment, node);
	    }});
	    if (richSearch && window.CSS && CSS.highlights) CSS.highlights.set('mdp-find', new Highlight(...findHits));
	    selectFindHit(0);
	  }}
	  function runFind(backward) {{
	    if (inReplaceMode()) {{
	      runReplaceFind(backward);
	      return;
	    }}
	    var q = findInput.value;
	    if (!q) {{ clearFindHits(); return; }}
	    var hadFocus = document.activeElement === findInput;
	    var selectionStart = findInput.selectionStart;
	    var selectionEnd = findInput.selectionEnd;
	    var normalized = String(q || '').trim();
	    if (!normalized) {{
	      clearFindHits();
	    }} else if (normalized !== lastFindQuery) {{
	      runFindQuery(normalized);
	    }} else {{
	      selectFindHit(currentFindHit + (backward ? -1 : 1));
	    }}
	    if (hadFocus) restoreFindInput(selectionStart, selectionEnd);
	  }}
	  function escapeRegExp(value) {{
	    return String(value).replace(/[.*+?^${{}}()|[\]\\]/g, '\\$&');
	  }}
	  function collectReplaceMatches(query) {{
	    if (!query) return [];
	    var matches = [];
	    var pattern = new RegExp(escapeRegExp(query), 'giu');
	    var match;
	    while ((match = pattern.exec(ta.value)) !== null) {{
	      matches.push({{ start: match.index, end: match.index + match[0].length }});
	    }}
	    return matches;
	  }}
	  function scrollReplaceHitIntoView(start) {{
	    requestAnimationFrame(function() {{
          if (inSplit() && splitSync) {{ splitSync.seek(start, true); return; }}
	      var before = ta.value.slice(0, start);
	      var line = (before.match(/\n/g) || []).length;
	      var total = Math.max(1, (ta.value.match(/\n/g) || []).length);
	      var ratio = Math.min(1, Math.max(0, line / total));
	      var editorTop = ta.getBoundingClientRect().top + (window.scrollY || 0);
	      var target = editorTop + ta.offsetHeight * ratio - window.innerHeight * 0.45;
	      window.scrollTo(window.scrollX || 0, Math.max(0, target));
	    }});
	  }}
	  function selectReplaceHit(index) {{
	    if (!replaceMatches.length) {{
	      currentReplaceHit = -1;
	      updateFindState();
	      return;
	    }}
	    currentReplaceHit = (index + replaceMatches.length) % replaceMatches.length;
	    var match = replaceMatches[currentReplaceHit];
	    ta.setSelectionRange(match.start, match.end);
	    scrollReplaceHitIntoView(match.start);
	    updateFindState();
	  }}
	  function runReplaceQuery(query, preferredOffset) {{
	    query = String(query || '');
	    replaceStatus = '';
	    replaceMatches = collectReplaceMatches(query);
	    lastReplaceQuery = query;
	    lastReplaceSource = ta.value;
	    currentReplaceHit = -1;
	    if (!replaceMatches.length) {{ updateFindState(); return; }}
	    var index = 0;
	    if (typeof preferredOffset === 'number') {{
	      var candidate = replaceMatches.findIndex(function(match) {{ return match.start >= preferredOffset; }});
	      if (candidate >= 0) index = candidate;
	    }}
	    selectReplaceHit(index);
	  }}
	  function runReplaceFind(backward) {{
	    var query = String(findInput.value || '');
	    if (!query) {{ clearReplaceMatches(true); updateFindState(); return; }}
	    if (query !== lastReplaceQuery || lastReplaceSource !== ta.value || replaceStatus) {{
	      runReplaceQuery(query);
	    }} else {{
	      selectReplaceHit(currentReplaceHit + (backward ? -1 : 1));
	    }}
	  }}
	  function commitReplacement(value, selectionStart, selectionEnd) {{
	    var previous = ta.value;
	    if (previous === value) {{
	      ta.setSelectionRange(selectionStart, selectionEnd);
	      return false;
	    }}
	    var revisionBefore = editorInputRevision;
	    applyingReplacement = true;
	    try {{
	      try {{ ta.focus({{ preventScroll: true }}); }} catch (_) {{ ta.focus(); }}
	      ta.setSelectionRange(0, previous.length);
	      document.execCommand('insertText', false, value);
	    }} catch (_) {{}}
	    applyingReplacement = false;
	    if (ta.value !== value) {{
	      ta.value = value;
	      handleEditorInput(true);
	    }} else if (editorInputRevision === revisionBefore) {{
	      handleEditorInput(true);
	    }}
	    ta.setSelectionRange(selectionStart, selectionEnd);
	    return true;
	  }}
	  function replaceCurrentMatch() {{
	    var query = String(findInput.value || '');
	    if (!query) return;
	    if (query !== lastReplaceQuery || lastReplaceSource !== ta.value || replaceStatus || !replaceMatches.length) runReplaceQuery(query);
	    if (!replaceMatches.length) return;
	    var returnFocus = document.activeElement;
	    var match = replaceMatches[Math.max(0, currentReplaceHit)];
	    var replacement = replaceInput.value;
	    var value = ta.value.slice(0, match.start) + replacement + ta.value.slice(match.end);
	    var nextOffset = match.start + replacement.length;
	    commitReplacement(value, match.start, nextOffset);
	    runReplaceQuery(query, nextOffset);
	    if (returnFocus && returnFocus.closest && returnFocus.closest('.findbar')) {{
	      try {{ returnFocus.focus({{ preventScroll: true }}); }} catch (_) {{ returnFocus.focus(); }}
	    }}
	  }}
	  function replaceAllMatches() {{
	    var query = String(findInput.value || '');
	    if (!query) return;
	    if (query !== lastReplaceQuery || lastReplaceSource !== ta.value || replaceStatus || !replaceMatches.length) runReplaceQuery(query);
	    if (!replaceMatches.length) return;
	    var matches = replaceMatches.slice();
	    var replacement = replaceInput.value;
	    var source = ta.value;
	    var chunks = [];
	    var cursor = 0;
	    matches.forEach(function(match) {{
	      chunks.push(source.slice(cursor, match.start));
	      chunks.push(replacement);
	      cursor = match.end;
	    }});
	    chunks.push(source.slice(cursor));
	    var firstStart = matches[0].start;
	    commitReplacement(chunks.join(''), firstStart, firstStart + replacement.length);
	    clearReplaceMatches(false);
	    replaceStatus = REPLACE_DONE.replace('{{count}}', String(matches.length));
	    updateFindState();
	    try {{ replaceInput.focus({{ preventScroll: true }}); }} catch (_) {{ replaceInput.focus(); }}
	  }}
	  function scheduleFind() {{
	    if (pendingFindTimer) clearTimeout(pendingFindTimer);
	    pendingFindTimer = setTimeout(function() {{
	      pendingFindTimer = 0;
	      if (!composingFind) {{
	        if (inReplaceMode()) runReplaceQuery(findInput.value);
	        else runFindQuery(findInput.value);
	      }}
	    }}, FIND_DEBOUNCE_MS);
	  }}
  // Grow textarea height to its content so the page (html) owns the sole
  // scrollbar; avoids the double-scrollbar you see if textarea keeps its
  // own internal scroll.
  function autoResize() {{
    if (inSplit()) {{ ta.style.height = '100%'; if (splitSync) splitSync.refresh(); return; }}
    var x = window.scrollX || document.documentElement.scrollLeft || 0;
    var y = window.scrollY || document.documentElement.scrollTop || 0;
    ta.style.height = 'auto';
    var h = Math.max(ta.scrollHeight, window.innerHeight);
    ta.style.height = h + 'px';
    window.scrollTo(x, y);
  }}
  function syncViewControls() {{
    if (splitSync) splitSync.active(inSplit());
    var hasEditor = inEdit();
    btnToggle.innerHTML = hasEditor ? ICON_VIEW : ICON_EDIT;
    btnToggle.title = hasEditor ? L_VIEW : L_EDIT;
    btnToggle.setAttribute('aria-label', hasEditor ? L_VIEW : L_EDIT);
    btnSplit.classList.toggle('on', inSplit());
    btnSplit.title = inSplit() ? L_SPLIT_EXIT : L_SPLIT;
    btnSplit.setAttribute('aria-label', inSplit() ? L_SPLIT_EXIT : L_SPLIT);
    btnSplit.setAttribute('aria-pressed', inSplit() ? 'true' : 'false');
  }}
  function focusEditorForIme() {{
    var selectionStart = ta.selectionStart;
    var selectionEnd = ta.selectionEnd;
    var selectionDirection = ta.selectionDirection || 'none';
    var scrollTop = ta.scrollTop;
    var scrollLeft = ta.scrollLeft;
    if (WINDOWS_IME_FOCUS_FIX && !editorImeFocusPrimed && ta.value) {{
      // WebView2/TSF can deadlock CJK composition when the first focused
      // textarea is already populated. Focus the real textarea while empty,
      // then restore its value and exact selection without emitting input.
      var raw = ta.value;
      ta.value = '';
      try {{ ta.focus({{ preventScroll: true }}); }} catch (_) {{ ta.focus(); }}
      ta.value = raw;
      try {{ ta.setSelectionRange(selectionStart, selectionEnd, selectionDirection); }} catch (_) {{}}
      ta.scrollTop = scrollTop;
      ta.scrollLeft = scrollLeft;
      editorImeFocusPrimed = true;
    }}
    try {{ ta.focus({{ preventScroll: true }}); }} catch (_) {{ ta.focus(); }}
  }}
  function enterEdit() {{
	    if (previewEditor) previewEditor.disable();
	    var progress = currentScrollProgress();
	    document.body.classList.remove('split-view');
	    document.body.classList.add('editing');
	    syncViewControls();
	    autoResize();
	    focusEditorForIme();
	    restoreScrollProgress(progress);
	  }}
  function enterSplit() {{
    if (previewEditor) previewEditor.disable();
    if (document.body.classList.contains('empty') || document.body.classList.contains('missing')) return;
    var progress = currentScrollProgress();
    var sourcePosition = splitSync ? splitSync.capture() : null;
    document.body.classList.remove('editing');
    document.body.classList.add('split-view');
    syncViewControls();
    autoResize();
    focusEditorForIme();
    restoreScrollProgress(progress);
    if (splitSync && sourcePosition !== null) requestAnimationFrame(function() {{ splitSync.seek(sourcePosition); }});
  }}
  function leaveEdit() {{
    var sourcePosition = inSplit() && splitSync ? splitSync.capture() : null;
    var progress = currentScrollProgress();
	    if (inReplaceMode()) hideFind(false);
    if (dirty) flushEdit();
    document.body.classList.remove('editing');
    document.body.classList.remove('split-view');
    syncViewControls();
    restoreScrollProgress(progress);
    if (splitSync && sourcePosition !== null) splitSync.restore_reading(sourcePosition);
  }}
  window.__mdPreviewToggleEdit = function() {{
    if (inEdit()) leaveEdit(); else enterEdit();
  }};
	window.__mdPreviewToggleSplit = function() {{
	  if (inSplit()) leaveEdit(); else enterSplit();
	}};
	window.__mdPreviewEnterEdit = function() {{
	  if (!inEdit()) enterEdit();
	}};
	window.__mdPreviewCloseActiveTab = function() {{
	  if (activeTabId) requestTabAction('close', activeTabId);
	}};
  window.__mdPreviewCheckUpdates = function() {{
    if (btnUpdate) btnUpdate.click();
  }};

	  btnOpen.addEventListener('click', openFile);
	  tabOpen.addEventListener('click', newFile);
	  btnSearch.addEventListener('click', showFind);
	  document.addEventListener('click', function(e) {{
	    var closeTab = e.target && e.target.closest ? e.target.closest('[data-close-tab]') : null;
	    if (closeTab) {{
	      e.preventDefault();
	      e.stopPropagation();
	      requestTabAction('close', closeTab.getAttribute('data-close-tab'));
	      return;
	    }}
	    var locateTab = e.target && e.target.closest ? e.target.closest('[data-locate-tab]') : null;
	    if (locateTab) {{
	      e.preventDefault();
	      window.ipc.postMessage('locate-tab:' + locateTab.getAttribute('data-locate-tab'));
	      return;
	    }}
	    var tab = e.target && e.target.closest ? e.target.closest('[data-tab-id]') : null;
	    if (tab) {{
	      e.preventDefault();
	      requestTabAction('activate', tab.getAttribute('data-tab-id'));
	      return;
	    }}
	    var openBtn = e.target && e.target.closest ? e.target.closest('[data-open-file]') : null;
	    if (openBtn) {{
	      e.preventDefault();
	      openFile();
	      return;
	    }}
	    var recentBtn = e.target && e.target.closest ? e.target.closest('[data-recent-index]') : null;
	    if (recentBtn) {{
	      e.preventDefault();
	      window.ipc.postMessage('open-recent:' + recentBtn.getAttribute('data-recent-index'));
	    }}
	  }});
	  findInput.addEventListener('compositionstart', function() {{ composingFind = true; }});
	  findInput.addEventListener('compositionend', function() {{ composingFind = false; scheduleFind(); }});
	  findInput.addEventListener('input', function(e) {{
	    if (composingFind || e.isComposing) return;
	    scheduleFind();
	  }});
	  findInput.addEventListener('keydown', function(e) {{
	    if (composingFind || e.isComposing) return;
	    if (e.key === 'Enter') {{ e.preventDefault(); runFind(e.shiftKey); }}
	    if (e.key === 'Escape') {{ e.preventDefault(); e.stopPropagation(); hideFind(); }}
	  }});
	  replaceInput.addEventListener('keydown', function(e) {{
	    if (e.key === 'Enter') {{ e.preventDefault(); replaceCurrentMatch(); }}
	    if (e.key === 'Escape') {{ e.preventDefault(); e.stopPropagation(); hideFind(); }}
	  }});
	  findPrev.addEventListener('click', function() {{ runFind(true); }});
	  findNext.addEventListener('click', function() {{ runFind(false); }});
	  findClose.addEventListener('click', hideFind);
	  replaceOne.addEventListener('click', replaceCurrentMatch);
	  replaceAll.addEventListener('click', replaceAllMatches);

	  btnToggle.addEventListener('click', function() {{
	    window.__mdPreviewToggleEdit();
	  }});
	  btnSplit.addEventListener('click', function() {{
	    window.__mdPreviewToggleSplit();
	  }});
	  btnZoom.addEventListener('click', function(e) {{
	    e.stopPropagation();
	    zoomControl.classList.toggle('open');
	  }});
	  btnZoomOut.addEventListener('click', function() {{ changeZoom(-ZOOM_STEP); }});
	  btnZoomReset.addEventListener('click', function() {{ applyZoom(100, true); }});
	  btnZoomIn.addEventListener('click', function() {{ changeZoom(ZOOM_STEP); }});

	  // Print / PDF popover (top-right, like the Word export control).
	  btnPrint.addEventListener('click', function(e) {{
	    e.stopPropagation();
	    printControl.classList.toggle('open');
	  }});
	  function doPrint() {{
	    printControl.classList.remove('open');
	    if (previewEditor && previewEditor.active()) {{
	      document.title = activeTabName || 'Markdown Studio';
	      renderCurrentSource(function() {{ window.ipc.postMessage('print'); }});
	      if (dirty) flushEdit();
	      return;
	    }}
	    if (inEdit()) leaveEdit();
	    // Give the print header a meaningful title instead of the empty
	    // NavigateToString title (which shows as about:blank).
	    document.title = activeTabName || 'Markdown Studio';
	    // Route through Rust: WKWebView ignores window.print(); wry's
	    // WebView::print() calls the right native API on each platform.
	    setTimeout(function(){{ window.ipc.postMessage('print'); }}, 0);
	  }}
	  printDirect.addEventListener('click', doPrint);
	  // Serialize the current preview into a standalone, self-contained HTML
	  // document for the Rust side to print to PDF via a headless browser
	  // (no header/footer, so no about:blank line like the system dialog).
	  function buildPdfHtml() {{
	    var styles = Array.prototype.map.call(document.querySelectorAll('style'), function(s) {{ return s.outerHTML; }}).join('');
	    var base = document.getElementById('base-href');
	    var extra = '<style>@page {{ size: A4; margin: 12mm; }}</style>';
	    return '<!DOCTYPE html><html><head><meta charset="utf-8"><title></title>' +
	      (base ? base.outerHTML : '') + styles + extra +
	      '</head><body><div id="app"><div id="preview">' +
	      document.getElementById('preview').innerHTML +
	      '</div></div></body></html>';
	  }}
	  pdfExport.addEventListener('click', function() {{
	    printControl.classList.remove('open');
	    if (previewEditor && previewEditor.active()) {{
	      renderCurrentSource(function() {{ window.ipc.postMessage('export-pdf\n' + buildPdfHtml()); }});
	      if (dirty) flushEdit();
	      return;
	    }}
	    if (inEdit()) leaveEdit();
	    setTimeout(function(){{ window.ipc.postMessage('export-pdf\n' + buildPdfHtml()); }}, 0);
	  }});

	  // About popover next to the "+" button.
	  tabAbout.addEventListener('click', function(e) {{
	    e.stopPropagation();
	    aboutPopover.classList.toggle('open');
	    tabAbout.classList.toggle('open', aboutPopover.classList.contains('open'));
	  }});

	  // Tab context menu: reveal the file in its folder.
	  function hideTabMenu() {{ tabMenu.hidden = true; tabMenuTabId = 0; }}
	  tabsEl.addEventListener('contextmenu', function(e) {{
	    var tab = e.target && e.target.closest ? e.target.closest('[data-tab-id]') : null;
	    if (!tab) return;
	    e.preventDefault();
	    e.stopPropagation();
	    var id = parseInt(tab.getAttribute('data-tab-id'), 10) || 0;
	    var path = tab.title || '';
	    // Untitled scratch tabs have no real file on disk to reveal.
	    if (!id || !path || tab.classList.contains('untitled')) return;
	    tabMenuTabId = id;
	    tabMenu.hidden = false;
	    var mw = tabMenu.offsetWidth, mh = tabMenu.offsetHeight;
	    var x = Math.min(e.clientX, window.innerWidth - mw - 8);
	    var y = Math.min(e.clientY, window.innerHeight - mh - 8);
	    tabMenu.style.left = Math.max(4, x) + 'px';
	    tabMenu.style.top = Math.max(4, y) + 'px';
	  }});
	  tabMenuReveal.addEventListener('click', function() {{
	    if (tabMenuTabId) window.ipc.postMessage('reveal-tab:' + tabMenuTabId);
	    hideTabMenu();
	  }});
	  window.addEventListener('blur', hideTabMenu);
	  window.addEventListener('resize', hideTabMenu);
	  ta.addEventListener('compositionstart', function() {{
	    editorComposing = true;
	    cancelPendingAutosave();
	  }});
	  ta.addEventListener('compositionend', function() {{
	    editorComposing = false;
	    if (dirty) scheduleAutosave();
	  }});
	  ta.addEventListener('input', handleEditorInput);
  window.addEventListener('resize', function() {{ if (inEdit()) autoResize(); }});
  document.addEventListener('click', function(e) {{
    if (!zoomControl.contains(e.target)) zoomControl.classList.remove('open');
    if (!printControl.contains(e.target)) printControl.classList.remove('open');
    if (!aboutPopover.contains(e.target) && e.target !== tabAbout) {{
      aboutPopover.classList.remove('open');
      tabAbout.classList.remove('open');
    }}
    if (!tabMenu.contains(e.target)) hideTabMenu();
  }});

  document.addEventListener('keydown', function(e) {{
	if (e.isComposing || editorComposing || e.keyCode === 229) return;
	if ((e.metaKey || e.ctrlKey) && (e.key === 'w' || e.key === 'W')) {{
	  if (activeTabId) {{
	    e.preventDefault();
	    requestTabAction('close', activeTabId);
	  }}
	  return;
	}}
	if ((e.metaKey || e.ctrlKey) && (e.key === 'n' || e.key === 'N')) {{
	  e.preventDefault();
	  newFile();
	  return;
	}}
    if ((e.metaKey || e.ctrlKey) && (e.key === '+' || e.key === '=' || e.code === 'NumpadAdd')) {{
      e.preventDefault();
      changeZoom(ZOOM_STEP);
      return;
    }}
    if ((e.metaKey || e.ctrlKey) && (e.key === '-' || e.code === 'NumpadSubtract')) {{
      e.preventDefault();
      changeZoom(-ZOOM_STEP);
      return;
    }}
    if ((e.metaKey || e.ctrlKey) && (e.key === '0' || e.code === 'Numpad0')) {{
      e.preventDefault();
      applyZoom(100, true);
      return;
    }}
    if ((e.metaKey || e.ctrlKey) && (e.key === 'r' || e.key === 'R')) {{
      e.preventDefault();
      if (!inEdit()) window.ipc.postMessage('refresh');
      return;
    }}
	    if ((e.metaKey || e.ctrlKey) && (e.key === 'o' || e.key === 'O')) {{
	      e.preventDefault();
	      openFile();
	      return;
	    }}
	    if (e.ctrlKey && !e.metaKey && (e.key === 'h' || e.key === 'H' || e.code === 'KeyH')) {{
	      e.preventDefault();
	      showReplace();
	      return;
	    }}
	    if ((e.metaKey || e.ctrlKey) && (e.key === 'f' || e.key === 'F')) {{
	      if (inEditOnly() || (inSplit() && document.activeElement === ta)) {{
	        if (inReplaceMode()) hideFind();
	        return;
	      }}
	      e.preventDefault();
	      showFind();
	      return;
	    }}
    if ((e.metaKey || e.ctrlKey) && (e.key === 'e' || e.key === 'E')) {{
      e.preventDefault();
      if (inEdit()) leaveEdit(); else enterEdit();
      return;
    }}
    if ((e.metaKey || e.ctrlKey) && (e.key === 't' || e.key === 'T')) {{
      if (inEdit()) return;
      e.preventDefault();
      setTocOpen(!document.body.classList.contains('toc-open'), true);
      return;
    }}
    if ((e.metaKey || e.ctrlKey) && e.shiftKey && (e.key === 's' || e.key === 'S')) {{
      e.preventDefault();
      if (previewEditor) previewEditor.flush();
      window.ipc.postMessage('save-as\n' + ta.value);
      return;
    }}
    if ((e.metaKey || e.ctrlKey) && (e.key === 's' || e.key === 'S')) {{
      if (inEdit() || (previewEditor && previewEditor.active())) {{ e.preventDefault(); save(); }}
      return;
    }}
    if ((e.metaKey || e.ctrlKey) && (e.key === 'p' || e.key === 'P')) {{
      e.preventDefault();
      doPrint();
      return;
    }}
	    if (e.key === 'Escape' && document.body.classList.contains('finding')) {{ hideFind(); return; }}
	    if (e.key === 'Escape') {{ hideTabMenu(); }}
	    if (e.key === 'Escape' && inEdit()) {{ leaveEdit(); }}
  }});

  // Called by Rust after a save (only preview is refreshed) or after an
  // external file change (both preview + textarea are refreshed).
  window.__setPreview = function(previewHtml, needsMath, needsMermaid, previewRaw) {{
    if (previewEditor && previewEditor.active()) previewEditor.flush();
    if (previewEditor) previewEditor.update_images(previewHtml);
    if (typeof previewRaw === 'string') previewRaw = previewRaw.replace(/\r\n?/g, '\n');
    // A save acknowledgement must never detach an active contenteditable/IME
    // or install HTML generated from an older source revision.
    if (typeof previewRaw === 'string' && previewRaw !== ta.value) return;
    if (previewEditor && previewEditor.active()) return;
    if (arguments.length > 1 && window.__setFeatureFlags) {{
      window.__setFeatureFlags(needsMath, needsMermaid);
    }}
    document.getElementById('preview').innerHTML = previewHtml;
    if (previewEditor) previewEditor.mount(typeof previewRaw === 'string' ? previewRaw : ta.value);
    buildToc();
    (window.requestIdleCallback || function(fn){{ return setTimeout(fn, 0); }})(function() {{
      if (typeof hljs !== 'undefined') hljs.highlightAll();
      if (window.__enhancePreview) window.__enhancePreview();
    }});
  }};
  window.__setBaseHref = function(baseHref) {{
    var base = document.getElementById('base-href');
    if (!base) {{
      base = document.createElement('base');
      base.id = 'base-href';
      document.head.insertBefore(base, document.head.firstChild);
    }}
    if (baseHref) base.setAttribute('href', baseHref);
    else base.removeAttribute('href');
  }};
	window.__markSaved = function(savedRaw) {{
	  if (previewEditor && previewEditor.active()) {{
	    previewEditor.flush();
	    if (editorComposing) {{ window.ipc.postMessage('dirty:1'); return; }}
	  }}
	  if (typeof savedRaw === 'string') savedRaw = savedRaw.replace(/\r\n?/g, '\n');
	  if (typeof savedRaw === 'string' && ta.value !== savedRaw) {{
	    window.ipc.postMessage('dirty:1');
	    return;
	  }}
	  autosavePaused = false;
	  setDirty(false);
	}};
	window.__mdPreviewPauseAutosave = function() {{
	  cancelPendingAutosave();
	  autosavePaused = true;
	}};
	window.__mdPreviewRestoreEditorFocus = function() {{
      if (document.body.classList.contains('finding')) {{ findInput.focus(); return; }}
	  if (inEdit()) focusEditorForIme(); else if (previewEditor && previewEditor.active()) previewEditor.focus();
	}};
	window.__mdPreviewResolveExternalChange = function() {{
	  if (previewEditor && previewEditor.active()) previewEditor.flush();
	  cancelPendingAutosave();
	  window.ipc.postMessage('external-change:' + (dirty ? 'dirty' : 'clean'));
	}};
	window.__setTabs = function(tabs) {{
	  tabs = Array.isArray(tabs) ? tabs : [];
	  tabsEl.textContent = '';
	  activeTabId = 0;
	  document.body.classList.toggle('has-tabs', tabs.length > 0);
	  tabs.forEach(function(tab) {{
	    var item = document.createElement('div');
	    item.className = 'tab' + (tab.active ? ' active' : '') + (tab.missing ? ' missing' : '') + (tab.dirty ? ' dirty' : '') + (tab.untitled ? ' untitled' : '');
	    item.setAttribute('data-tab-id', tab.id);
	    item.setAttribute('role', 'button');
	    item.setAttribute('tabindex', '0');
	    item.title = tab.path;
	    if (tab.active) {{ activeTabId = tab.id; activeTabName = tab.name || ''; }}
	    var status = document.createElement('span');
	    status.className = 'tab-status';
	    status.textContent = tab.missing ? '!' : '';
	    var name = document.createElement('span');
	    name.className = 'tab-name';
	    name.textContent = tab.name;
	    var close = document.createElement('button');
	    close.className = 'tab-close';
	    close.type = 'button';
	    close.setAttribute('data-close-tab', tab.id);
	    close.setAttribute('aria-label', 'Close ' + tab.name);
	    close.textContent = '×';
	    item.appendChild(status);
	    item.appendChild(name);
	    item.appendChild(close);
	    tabsEl.appendChild(item);
	    if (tab.active) requestAnimationFrame(function() {{ item.scrollIntoView({{ block: 'nearest', inline: 'nearest' }}); }});
	  }});
	}};
	tabsEl.addEventListener('keydown', function(e) {{
	  if (e.key !== 'Enter' && e.key !== ' ') return;
	  var tab = e.target && e.target.closest ? e.target.closest('[data-tab-id]') : null;
	  if (!tab) return;
	  e.preventDefault();
	  requestTabAction('activate', tab.getAttribute('data-tab-id'));
	}});
	  window.__setContent = function(previewHtml, rawMd, baseHref, needsMath, needsMermaid) {{
	    if (previewEditor) previewEditor.reset();
	    document.body.classList.remove('empty');
	    document.body.classList.remove('missing');
	    window.__mdPreviewIsUntitled = false;
	    hideFind();
	    window.__setBaseHref(baseHref);
    if (!inEdit() || !dirty) {{
	      autosavePaused = false;
	      ta.value = rawMd;
	      updateDocumentStats(rawMd);
      setDirty(false);
      if (inEdit()) autoResize();
    }}
    window.__setPreview(previewHtml, needsMath, needsMermaid, rawMd);
  }};
	  window.__setEmptyPreview = function(previewHtml) {{
	    if (previewEditor) previewEditor.reset();
	    document.body.classList.add('empty');
	    document.body.classList.remove('missing');
	    window.__mdPreviewIsUntitled = false;
	    document.body.classList.remove('editing');
	    document.body.classList.remove('split-view');
	    syncViewControls();
	    cancelPendingAutosave();
	    autosavePaused = false;
	    hideFind();
	    window.__setBaseHref('');
	    document.getElementById('preview').innerHTML = previewHtml;
	    buildToc();
	    ta.value = '';
	    updateDocumentStats('');
	    setDirty(false);
	    window.scrollTo(0, 0);
	  }};
	  window.__setMissing = function(previewHtml) {{
	    if (previewEditor) previewEditor.reset();
	    document.body.classList.remove('empty');
	    document.body.classList.add('missing');
	    window.__mdPreviewIsUntitled = false;
	    document.body.classList.remove('editing');
	    document.body.classList.remove('split-view');
	    syncViewControls();
	    cancelPendingAutosave();
	    autosavePaused = false;
	    hideFind();
	    window.__setBaseHref('');
	    document.getElementById('preview').innerHTML = previewHtml;
	    buildToc();
	    ta.value = '';
	    updateDocumentStats('');
	    setDirty(false);
	    window.scrollTo(0, 0);
	  }};

  if (window.__mdPreviewInstallPreviewEdit) {{
    previewEditor = window.__mdPreviewInstallPreviewEdit({{
      root: document.getElementById('preview'), textarea: ta,
      host: document.getElementById('rich-editor'), hint: document.getElementById('preview-edit-hint'),
      asset_base: '{editor_asset_base}',
      button: document.getElementById('btn-preview-edit'), zh: {preview_edit_zh},
      on_input: handleEditorInput,
      on_dirty: function() {{ setDirty(true); }},
      prepare: prepareStructuredEdit,
      on_composition: function(value) {{
        editorComposing = value;
        if (value) cancelPendingAutosave(); else if (dirty) scheduleAutosave();
      }},
      on_enter: function() {{
        if (inEdit()) leaveEdit();
        hideFind(false);
      }},
      on_ready: function() {{ buildToc(); }},
      on_exit: function() {{
        renderCurrentSource();
        if (dirty) flushEdit();
      }}
    }});
    previewEditor.mount(ta.value);
  }}
  if (window.__mdPreviewInstallDiagramEditor) window.__mdPreviewInstallDiagramEditor({{
    zh: {preview_edit_zh},
    bind: function(box) {{
      if (previewEditor) previewEditor.flush();
      var scope = previewEditor && previewEditor.active() ? previewEditor.root() : document.getElementById('preview');
      var boxes = Array.from(scope.querySelectorAll('.mdp-mermaid,.mdp-mermaid-error'));
      var index = boxes.indexOf(box), blocks = window.__mdPreviewMermaidTools.fences(ta.value);
      var block = blocks[index];
      if (!block || !box.dataset.mdpDiagramSource || block.source.trim() !== box.dataset.mdpDiagramSource.trim()) {{
        throw new Error({preview_edit_zh} ? '无法准确对应原始流程图，请重新打开文档或使用源码编辑。' : 'Could not match the source diagram. Reopen the file or use source editing.');
      }}
      return {{ raw: ta.value, source: block.source, index: index, tab: activeTabId }};
    }},
    apply: async function(binding, source) {{
      if (previewEditor) previewEditor.flush();
      if (binding.tab !== activeTabId || binding.raw !== ta.value) throw new Error({preview_edit_zh} ? '文档已变化，请重新打开流程图后修改。' : 'Document changed. Reopen the diagram.');
      await prepareStructuredEdit();
      if (binding.tab !== activeTabId || binding.raw !== ta.value) throw new Error({preview_edit_zh} ? '备份期间文档已变化，操作已取消。' : 'Document changed during backup. Action cancelled.');
      var block = window.__mdPreviewMermaidTools.fences(ta.value)[binding.index];
      if (!block || block.source !== binding.source) throw new Error('Diagram source changed');
      source = source.replace(/\n?$/, '\n');
      var next = ta.value.slice(0, block.start) + source + ta.value.slice(block.end);
      if (previewEditor && previewEditor.active()) previewEditor.replace_markdown(next);
      else {{ ta.setRangeText(source, block.start, block.end, 'preserve'); handleEditorInput(); renderCurrentSource(); }}
      return {{ raw: next, source: source, index: binding.index, tab: binding.tab }};
    }}
  }});
  // Defer hljs parse + initial highlight to idle time.
  // hljs itself is NOT inlined in this page — Rust injects it via
  // evaluate_script once we tell it we're painted. Until that injection
  // runs, typeof hljs === 'undefined' and highlightAll is skipped; once
  // it lands, hljs.highlightAll() gets called by the injected bootstrap
  // and __setPreview.

  // Signal Rust after first paint (triggers hljs inject; bench mode exits).
  requestAnimationFrame(function() {{
    requestAnimationFrame(function() {{
      if (window.ipc) window.ipc.postMessage('ready');
    }});
  }});
}})();
window.__mdPreviewFeatureFlags = {{ math: {needs_math}, mermaid: {needs_mermaid} }};
window.__mdPreviewLightboxLabels = {{
  zoomOut: '{lb_zoom_out_js}', zoomIn: '{lb_zoom_in_js}', fit: '{lb_fit_js}',
  pan: '{lb_pan_js}', png: '{lb_png_js}', svg: '{lb_svg_js}', close: '{lb_close_js}'
}};
{preview_enhance_js}
if(window.__enhancePreview)window.__enhancePreview();
{update_check_js}
window.__mdPreviewInstallUpdateCheck({{
  currentVersion: '{app_version}',
  buttonLabel: '{btn_update_js}',
  nativeUpdater: {native_updater},
  apiUrl: 'https://api.github.com/repos/galen2027/md-studio/releases?per_page=20',
  latestUrl: 'https://github.com/galen2027/md-studio/releases/latest'
}});
{test_update_release_js}
if ({test_export}) {{
  // Automated end-to-end export check (MD_PREVIEW_TEST_EXPORT is set): wait
  // for lazy enhancers (mermaid/katex) to be injected, then trigger the Word
  // export exactly like the toolbar button does.
  setTimeout(function() {{ document.getElementById('word-export').click(); }}, 6000);
}}
if ({test_pdf}) {{
  // Same idea for the PDF export (MD_PREVIEW_TEST_EXPORT_PDF is set).
  setTimeout(function() {{ document.getElementById('pdf-export').click(); }}, 6000);
}}
</script>
</body></html>"#,
        css_light = HLJS_LIGHT,
        css_dark = HLJS_DARK,
        base_tag = base_tag,
        preview_html = preview_html,
        raw_md_escaped = html_escape_ta(raw_md),
        btn_open = s.btn_open,
        btn_new = s.btn_new,
        btn_search = s.btn_search,
        btn_edit = s.btn_edit,
        btn_preview = s.btn_preview,
        btn_split = s.btn_split,
        btn_split_exit = s.btn_split_exit,
        btn_print = s.btn_print,
        btn_update = s.btn_update,
        btn_zoom = s.btn_zoom,
        btn_zoom_out = s.btn_zoom_out,
        btn_zoom_reset = s.btn_zoom_reset,
        btn_zoom_in = s.btn_zoom_in,
        search_placeholder = s.search_placeholder,
        replace_placeholder = s.replace_placeholder,
        replace_one = s.replace_one,
        replace_all = s.replace_all,
        replace_done_js = escape_js(s.replace_done),
        btn_update_js = escape_js(s.btn_update),
        stat_words_js = escape_js(s.stat_words),
        stat_chars_js = escape_js(s.stat_chars),
        btn_toc = s.btn_toc,
        toc_title = s.toc_title,
        btn_save_as = s.btn_save_as,
        btn_word = s.btn_word,
        export_standardize = s.export_standardize,
        export_strip_numbers = s.export_strip_numbers,
        export_now = s.export_now,
        btn_about = s.btn_about,
        about_developer = s.about_developer,
        about_version = s.about_version,
        about_updated = s.about_updated,
        developer_name = DEVELOPER_NAME,
        release_date = RELEASE_DATE,
        print_now = s.print_now,
        pdf_export = s.pdf_export,
        tab_reveal = s.tab_reveal,
        lb_zoom_out_js = escape_js(s.lb_zoom_out),
        lb_zoom_in_js = escape_js(s.lb_zoom_in),
        lb_fit_js = escape_js(s.lb_fit),
        lb_pan_js = escape_js(s.lb_pan),
        lb_png_js = escape_js(s.lb_png),
        lb_svg_js = escape_js(s.lb_svg),
        lb_close_js = escape_js(s.lb_close),
        app_version = update_current_version(),
        test_update_release_js = test_update_release_js(),
        test_export = std::env::var_os("MD_PREVIEW_TEST_EXPORT").is_some(),
        test_pdf = std::env::var_os("MD_PREVIEW_TEST_EXPORT_PDF").is_some(),
        native_updater = native_updater,
        body_class = body_class,
        needs_math = flags.math,
        needs_mermaid = flags.mermaid,
        preview_enhance_js = PREVIEW_ENHANCE_JS,
        preview_edit_js = PREVIEW_EDIT_JS,
        split_sync_js = SPLIT_SYNC_JS,
        mermaid_edit_js = MERMAID_EDIT_JS,
        editor_asset_base = editor_assets::base_url(),
        preview_edit_zh = s.preview_edit_zh,
        btn_preview_edit = s.btn_preview_edit,
        preview_edit_hint = s.preview_edit_hint,
        preview_render_failed = if s.preview_edit_zh {
            "图表尚未完成渲染，导出未开始，请稍后重试。"
        } else {
            "Diagram rendering did not complete. Export was not started; please retry."
        },
        update_check_js = UPDATE_CHECK_JS,
        windows_ime_focus_fix = cfg!(target_os = "windows"),
    )
}

fn escape_js(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn is_allowed_update_url(url: &str) -> bool {
    url == "https://github.com/galen2027/md-studio/releases/latest"
        || url.starts_with("https://github.com/galen2027/md-studio/releases/tag/")
        || url.starts_with("https://github.com/galen2027/md-studio/releases/download/")
}

fn update_current_version() -> String {
    #[cfg(debug_assertions)]
    {
        if let Ok(version) = std::env::var("MD_PREVIEW_TEST_CURRENT_VERSION") {
            return version;
        }
    }

    env!("CARGO_PKG_VERSION").to_string()
}

#[derive(Debug, PartialEq, Eq)]
struct UpdateRelease {
    tag: String,
    url: String,
    digest: Option<String>,
}

fn parse_version(value: &str) -> Option<Vec<u64>> {
    let cleaned = value
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['+', '-'])
        .next()?;
    if cleaned.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    for part in cleaned.split('.') {
        if part.is_empty() || !part.chars().all(|ch| ch.is_ascii_digit()) {
            return None;
        }
        parts.push(part.parse().ok()?);
    }
    Some(parts)
}

fn is_newer_version(candidate: &str, current: &str) -> bool {
    let Some(next) = parse_version(candidate) else {
        return false;
    };
    let Some(now) = parse_version(current) else {
        return false;
    };
    let len = next.len().max(now.len());
    for index in 0..len {
        let a = *next.get(index).unwrap_or(&0);
        let b = *now.get(index).unwrap_or(&0);
        if a > b {
            return true;
        }
        if a < b {
            return false;
        }
    }
    false
}

fn is_desktop_release_tag(tag: &str) -> bool {
    let Some(version) = tag.trim().strip_prefix('v') else {
        return false;
    };
    version.contains('.')
        && version.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
        && parse_version(tag).is_some()
}

fn preferred_update_asset_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "MD-Preview-macOS-universal.dmg"
    } else if cfg!(target_os = "windows") {
        "MD-Preview-windows-x64.exe"
    } else {
        "MD-Preview-linux-x64.tar.gz"
    }
}

fn select_update_release(payload: &str, current_version: &str) -> Option<UpdateRelease> {
    let releases: serde_json::Value = serde_json::from_str(payload).ok()?;
    let releases = releases.as_array()?;
    let asset_name = preferred_update_asset_name();
    let mut best: Option<UpdateRelease> = None;

    for release in releases {
        if release
            .get("draft")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
            || release
                .get("prerelease")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        {
            continue;
        }
        let Some(tag) = release.get("tag_name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if !is_desktop_release_tag(tag) || !is_newer_version(tag, current_version) {
            continue;
        }

        let html_url = release
            .get("html_url")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(GITHUB_URL);
        let mut url = html_url;
        let mut digest = None;
        if let Some(assets) = release.get("assets").and_then(serde_json::Value::as_array) {
            for asset in assets {
                if asset.get("name").and_then(serde_json::Value::as_str) == Some(asset_name) {
                    if let Some(download_url) = asset
                        .get("browser_download_url")
                        .and_then(serde_json::Value::as_str)
                    {
                        url = download_url;
                    }
                    digest = asset
                        .get("digest")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    break;
                }
            }
        }

        let candidate = UpdateRelease {
            tag: tag.to_string(),
            url: url.to_string(),
            digest,
        };
        if best
            .as_ref()
            .map(|current| is_newer_version(&candidate.tag, &current.tag))
            .unwrap_or(true)
        {
            best = Some(candidate);
        }
    }

    best
}

fn check_github_updates() -> UpdateCheckResult {
    let output = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "--connect-timeout",
            "5",
            "--max-time",
            "10",
            "-H",
            "Accept: application/vnd.github+json",
            "https://api.github.com/repos/galen2027/md-studio/releases?per_page=20",
        ])
        .output();
    let Ok(output) = output else {
        return UpdateCheckResult::Failed;
    };
    if !output.status.success() {
        return UpdateCheckResult::Failed;
    }
    let Ok(payload) = String::from_utf8(output.stdout) else {
        return UpdateCheckResult::Failed;
    };
    match select_update_release(&payload, env!("CARGO_PKG_VERSION")) {
        Some(release) => UpdateCheckResult::Available {
            tag: release.tag,
            url: release.url,
            digest: release.digest,
        },
        None => UpdateCheckResult::UpToDate,
    }
}

fn test_update_release_js() -> String {
    #[cfg(debug_assertions)]
    {
        let Ok(tag) = std::env::var("MD_PREVIEW_TEST_UPDATE_TAG") else {
            return String::new();
        };
        let tag = tag.trim();
        if tag.is_empty() {
            return String::new();
        }
        let escaped_tag = escape_js(tag);
        return format!(
            r#"if(window.__mdPreviewApplyUpdateRelease)window.__mdPreviewApplyUpdateRelease({{
  tag_name: '{tag}',
  html_url: 'https://github.com/galen2027/md-studio/releases/tag/{tag}',
  download_url: 'https://github.com/galen2027/md-studio/releases/download/{tag}/MD-Preview-macOS-universal.dmg'
}});"#,
            tag = escaped_tag
        );
    }

    #[cfg(not(debug_assertions))]
    {
        String::new()
    }
}

fn watch_scope_for_file(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(path)
}

fn event_path_matches_file(event_path: &Path, target: &Path) -> bool {
    event_path == target
        || (target.file_name().is_some()
            && event_path.file_name() == target.file_name()
            && event_path.parent() == target.parent())
}

fn event_should_reload_file(ev: &Event, target: &Path) -> bool {
    if ev.need_rescan() {
        return true;
    }

    if !(ev.kind.is_modify() || ev.kind.is_create() || ev.kind.is_remove()) {
        return false;
    }

    ev.paths
        .iter()
        .any(|event_path| event_path_matches_file(event_path, target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::EventKind;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_test_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("md-preview-{name}-{}-{unique}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn save_failure_dialog_maps_save_discard_and_cancel() {
        assert_eq!(
            classify_save_failure_choice(
                rfd::MessageDialogResult::Custom("另存为…".to_string()),
                "另存为…",
                "放弃修改并退出",
            ),
            SaveFailureChoice::SaveAs
        );
        assert_eq!(
            classify_save_failure_choice(
                rfd::MessageDialogResult::Custom("放弃修改并退出".to_string()),
                "另存为…",
                "放弃修改并退出",
            ),
            SaveFailureChoice::Discard
        );
        assert_eq!(
            classify_save_failure_choice(
                rfd::MessageDialogResult::Custom("取消".to_string()),
                "另存为…",
                "放弃修改并退出",
            ),
            SaveFailureChoice::Cancel
        );
        assert_eq!(
            classify_save_failure_choice(
                rfd::MessageDialogResult::Yes,
                "Save As…",
                "Discard and Exit",
            ),
            SaveFailureChoice::SaveAs
        );
        assert_eq!(
            classify_save_failure_choice(
                rfd::MessageDialogResult::No,
                "Save As…",
                "Discard and Exit",
            ),
            SaveFailureChoice::Discard
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn webview_startup_failure_detects_missing_runtime_hresult() {
        assert_eq!(
            classify_windows_webview_startup_failure(
                "WebView2Error(WindowsError(Error { code: HRESULT(0x80070002) }))"
            ),
            WindowsWebViewStartupFailure::MissingRuntime
        );
        assert_eq!(
            classify_windows_webview_startup_failure(
                "Could not find a compatible WebView2 Runtime"
            ),
            WindowsWebViewStartupFailure::MissingRuntime
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn webview_startup_failure_keeps_unrelated_errors_generic() {
        assert_eq!(
            classify_windows_webview_startup_failure(
                "WebView2Error(WindowsError(Error { code: HRESULT(0x80070005) }))"
            ),
            WindowsWebViewStartupFailure::Other
        );
        assert!(WEBVIEW2_DOWNLOAD_URL.starts_with("https://developer.microsoft.com/"));
    }

    #[test]
    fn dollar_math_is_protected_from_markdown_escapes() {
        let html = md_to_html(r"$\{x\}$");

        assert!(html.contains(r#"<span class="math math-inline">\{x\}</span>"#));
    }

    #[test]
    fn dollar_math_is_protected_from_markdown_emphasis() {
        let html = md_to_html(r"$\bar{\mu}_{n}$ and $x_{n}$");

        assert!(html.contains(r#"<span class="math math-inline">\bar{\mu}_{n}</span>"#));
        assert!(html.contains(r#"<span class="math math-inline">x_{n}</span>"#));
        assert!(!html.contains("<em>"));
    }

    #[test]
    fn github_alerts_render_as_markdown_alert_blockquotes() {
        let html = md_to_html("> [!IMPORTANT]\n> This is an alert");

        assert!(html.contains(r#"class="markdown-alert-important">"#));
        assert!(html.contains("<p>This is an alert</p>"));
        assert!(!html.contains("[!IMPORTANT]"));
    }

    #[test]
    fn double_equals_highlight_renders_mark_without_touching_code() {
        let html = md_to_html("Use ==highlight & tag== here and `==literal==` there.");

        assert!(html.contains(r#"Use <mark class="mdp-mark">highlight &amp; tag</mark> here"#));
        assert!(html.contains("<code>==literal==</code>"));
    }

    #[test]
    fn local_relative_images_are_streamed_from_markdown_directory() {
        let dir = temp_test_dir("local-image");
        let assets = dir.join("assets");
        fs::create_dir_all(&assets).unwrap();
        fs::write(assets.join("pixel.png"), b"abc").unwrap();
        let local_images = Arc::new(Mutex::new(LocalImageRegistry::default()));

        let html =
            md_to_html_with_registry("![pixel](assets/pixel.png)", Some(&dir), &local_images);

        assert!(html.contains(&format!(
            r#"<img src="{}" data-mdp-source="assets/pixel.png" alt="pixel" />"#,
            local_image_transport_url("1/1")
        )));
        assert!(!html.contains("data:image"));
        let response = local_image_protocol_response(&local_images, "/1/1");
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers()[wry::http::header::CONTENT_TYPE],
            "image/png"
        );
        assert_eq!(
            response.headers()[wry::http::header::CACHE_CONTROL],
            "no-store"
        );
        assert_eq!(response.body().as_ref(), b"abc");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn local_image_transport_url_matches_the_platform_webview_origin() {
        let url = local_image_transport_url("7/11");
        #[cfg(target_os = "windows")]
        assert_eq!(url, "http://mdpreview-image.localhost/7/11");
        #[cfg(not(target_os = "windows"))]
        assert_eq!(url, "mdpreview-image://localhost/7/11");
    }

    #[test]
    fn local_image_mime_type_accepts_common_jpeg_extensions() {
        for extension in ["jpg", "jpeg", "jpe", "jfif", "JPG"] {
            assert_eq!(
                image_mime_type(Path::new(&format!("photo.{extension}"))),
                Some("image/jpeg")
            );
        }
    }

    #[test]
    fn large_chinese_and_percent_encoded_image_paths_do_not_expand_html() {
        let dir = temp_test_dir("large-chinese-image");
        let assets = dir.join("材料 图");
        fs::create_dir_all(&assets).unwrap();
        let bytes = vec![0x5a; 5_000_000];
        fs::write(assets.join("电视 回看.png"), &bytes).unwrap();
        let local_images = Arc::new(Mutex::new(LocalImageRegistry::default()));

        let html = md_to_html_with_registry(
            "![回看](材料%20图/电视%20回看.png)",
            Some(&dir),
            &local_images,
        );

        assert!(html.contains(&local_image_transport_url("1/1")));
        assert!(
            html.len() < 256,
            "HTML should not contain image bytes: {}",
            html.len()
        );
        let response = local_image_protocol_response(&local_images, "/1/1");
        assert_eq!(response.status(), 200);
        assert_eq!(response.body().len(), bytes.len());
        assert_eq!(response.body().as_ref(), bytes.as_slice());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn local_relative_images_keep_original_src_when_unreadable() {
        let dir = temp_test_dir("missing-image");

        let html = md_to_html_with_base("![missing](assets/missing.png)", Some(&dir));

        assert!(html.contains(r#"<img src="assets/missing.png" alt="missing" />"#));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn local_images_support_exact_parent_backslash_absolute_and_file_urls() {
        let root = temp_test_dir("portable-image-paths");
        let docs = root.join("任意 文档目录");
        let assets = root.join("共享 图片");
        fs::create_dir_all(&docs).unwrap();
        fs::create_dir_all(&assets).unwrap();
        let image = assets.join("示意 图.png");
        fs::write(&image, b"portable").unwrap();
        let file_url = url::Url::from_file_path(&image).unwrap().to_string();
        let absolute = image.to_string_lossy().replace('\\', "/");
        let markdown = format!(
            "![parent](../共享%20图片/示意%20图.png)\n![backslash](<..\\共享 图片\\示意 图.png>)\n![absolute](<{absolute}>)\n![file]({file_url})"
        );
        let local_images = Arc::new(Mutex::new(LocalImageRegistry::default()));

        let html = md_to_html_with_registry(&markdown, Some(&docs), &local_images);

        assert_eq!(html.matches("mdpreview-image").count(), 4);
        let registered = local_images.lock().unwrap().assets.clone();
        assert_eq!(registered.len(), 4);
        let canonical_image = fs::canonicalize(&image).unwrap();
        assert!(registered
            .values()
            .all(|asset| asset.path == canonical_image));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn local_images_reject_remote_urls_and_network_shares() {
        let dir = temp_test_dir("reject-network-images");

        assert!(resolve_local_image_path(&dir, "https://example.com/image.png").is_none());
        assert!(resolve_local_image_path(&dir, r"\\server\share\image.png").is_none());
        assert!(resolve_local_image_path(&dir, "//server/share/image.png").is_none());
        assert!(resolve_local_image_path(&dir, "file://server/share/image.png").is_none());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn local_image_protocol_rejects_unregistered_assets() {
        let local_images = Arc::new(Mutex::new(LocalImageRegistry::default()));
        let response = local_image_protocol_response(&local_images, "/999/999");
        assert_eq!(response.status(), 404);
        assert_eq!(
            response.headers()[wry::http::header::CACHE_CONTROL],
            "no-store"
        );
    }

    #[test]
    fn local_image_urls_survive_the_next_document_render() {
        let dir = temp_test_dir("image-generation");
        fs::write(dir.join("first.png"), b"first").unwrap();
        fs::write(dir.join("second.png"), b"second").unwrap();
        let local_images = Arc::new(Mutex::new(LocalImageRegistry::default()));

        let first = md_to_html_with_registry("![](first.png)", Some(&dir), &local_images);
        assert!(first.contains(&local_image_transport_url("1/1")));
        let second = md_to_html_with_registry("![](second.png)", Some(&dir), &local_images);
        assert!(second.contains(&local_image_transport_url("2/1")));
        assert_eq!(
            local_image_protocol_response(&local_images, "/1/1")
                .body()
                .as_ref(),
            b"first"
        );
        assert_eq!(
            local_image_protocol_response(&local_images, "/2/1")
                .body()
                .as_ref(),
            b"second"
        );
        for _ in 0..LOCAL_IMAGE_GENERATIONS_TO_KEEP {
            let _ = md_to_html_with_registry("![](second.png)", Some(&dir), &local_images);
        }
        assert_eq!(
            local_image_protocol_response(&local_images, "/1/1").status(),
            404
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn yaml_front_matter_keeps_metadata_readable_without_blank_lines() {
        let html = md_to_html(
            "---\nname: skill-creator-lite\ndescription: Skill 创建封装工具。\n---\n# Body",
        );

        assert!(html.contains(r#"<aside class="front-matter">"#));
        assert!(html.contains("name: skill-creator-lite"));
        assert!(html.contains("description: Skill 创建封装工具。"));
        assert!(html.contains("id=\"body\">Body</h1>"));
        assert!(!html.contains("<h2"));
    }

    #[test]
    fn yaml_front_matter_accepts_dots_and_escapes_html() {
        let html = md_to_html("---\ntitle: <script>alert(1)</script>\n...\nParagraph");

        assert!(html.contains("title: &lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains(">Paragraph</p>"));
    }

    #[test]
    fn horizontal_rules_and_setext_headings_are_not_front_matter() {
        let html = md_to_html("Title\n---\n\nParagraph\n\n---");

        assert!(!html.contains(r#"<aside class="front-matter">"#));
        assert!(html.contains(r#"id="title">Title</h2>"#));
        assert!(html.contains("<hr data-source-start="));
    }

    #[test]
    fn file_urls_only_open_existing_supported_documents() {
        let dir = temp_test_dir("document-links");
        let linked = dir.join("含 空格.md");
        let unsupported = dir.join("page.html");
        fs::write(&linked, "# Linked").unwrap();
        fs::write(&unsupported, "<h1>Page</h1>").unwrap();

        let linked_url = url::Url::from_file_path(&linked).unwrap().to_string();
        let unsupported_url = url::Url::from_file_path(&unsupported).unwrap().to_string();
        let missing_url = url::Url::from_file_path(dir.join("missing.md"))
            .unwrap()
            .to_string();

        assert_eq!(
            local_document_path_from_url(&format!("{linked_url}#section")),
            Some(fs::canonicalize(&linked).unwrap())
        );
        assert_eq!(local_document_path_from_url(&unsupported_url), None);
        assert_eq!(local_document_path_from_url(&missing_url), None);
        assert_eq!(
            local_document_path_from_url("https://example.com/readme.md"),
            None
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn linux_nvidia_compat_env_only_sets_dmabuf_when_unconfigured() {
        assert_eq!(
            linux_webkit_compat_env(None, None, true),
            Some(("WEBKIT_DISABLE_DMABUF_RENDERER", "1"))
        );
        assert_eq!(linux_webkit_compat_env(Some("0"), None, true), None);
        assert_eq!(linux_webkit_compat_env(None, Some("1"), true), None);
        assert_eq!(linux_webkit_compat_env(None, None, false), None);
    }

    #[test]
    fn generated_heading_ids_support_cjk_anchor_links() {
        let html = md_to_html("1. [需求概述](#需求概述)\n\n## 需求概述");

        assert!(html.contains(r##"<a href="#%E9%9C%80%E6%B1%82%E6%A6%82%E8%BF%B0">需求概述</a>"##));
        assert!(html.contains(r#"id="需求概述">需求概述</h2>"#));
    }

    #[test]
    fn generated_heading_ids_are_unique_and_keep_explicit_ids() {
        let html = md_to_html("## Intro\n## Intro\n## Custom {#fixed}\n## Fixed");

        assert!(html.contains(r#"id="intro">Intro</h2>"#));
        assert!(html.contains(r#"id="intro-1">Intro</h2>"#));
        assert!(html.contains(r#"id="fixed">Custom</h2>"#));
        assert!(html.contains(r#"id="fixed-1">Fixed</h2>"#));
    }

    #[test]
    fn help_flags_are_recognized() {
        assert!(is_help_arg("-h"));
        assert!(is_help_arg("--help"));
        assert!(!is_help_arg("--edit"));
    }

    #[test]
    fn version_flags_are_recognized() {
        assert!(is_version_arg("-V"));
        assert!(is_version_arg("--version"));
        assert!(!is_version_arg("--edit"));
    }

    #[test]
    fn theme_choice_parses_menu_values() {
        assert_eq!(ThemeChoice::from_str("system"), ThemeChoice::System);
        assert_eq!(ThemeChoice::from_str("light"), ThemeChoice::Light);
        assert_eq!(ThemeChoice::from_str("dark"), ThemeChoice::Dark);
        assert_eq!(ThemeChoice::from_str("unexpected"), ThemeChoice::System);
        assert_eq!(ThemeChoice::Dark.as_str(), "dark");
    }

    #[test]
    fn dump_demo_page_when_requested() {
        let Some(out) = std::env::var_os("MD_PREVIEW_DUMP_PAGE") else {
            return;
        };
        let md = include_str!("../../verify/demo.md");
        let strings = Strings::for_lang(Lang::Zh);
        let page = build_page(
            &md_to_html(md),
            md,
            None,
            EnhanceFlags::default(),
            &strings,
            false,
            true,
        );
        std::fs::write(out, page).unwrap();
    }

    #[test]
    fn dump_mermaid_page_when_requested() {
        let Some(out) = std::env::var_os("MD_PREVIEW_DUMP_PAGE_MERMAID") else {
            return;
        };
        let md = include_str!("../../verify/mermaid-demo.md");
        let strings = Strings::for_lang(Lang::Zh);
        let page = build_page(
            &md_to_html(md),
            md,
            None,
            EnhanceFlags::default(),
            &strings,
            false,
            true,
        );
        std::fs::write(out, page).unwrap();
    }

    #[test]
    fn dump_preview_edit_page_when_requested() {
        let Some(out) = std::env::var_os("MD_PREVIEW_DUMP_PAGE_EDIT") else {
            return;
        };
        let md = include_str!("../tests/fixtures/structured-edit.md");
        let page = build_page(
            &md_to_html(md),
            md,
            None,
            enhance_flags_for(md),
            &Strings::for_lang(Lang::Zh),
            false,
            false,
        );
        fs::write(out, page).unwrap();
    }

    #[test]
    fn validate_local_image_document_when_requested() {
        let Some(path) = std::env::var_os("MD_PREVIEW_VALIDATE_IMAGE_DOCUMENT") else {
            return;
        };
        let path = PathBuf::from(path);
        let markdown = fs::read_to_string(&path).expect("validation Markdown should be readable");
        let local_images = Arc::new(Mutex::new(LocalImageRegistry::default()));
        let html = md_to_html_with_registry(&markdown, path.parent(), &local_images);
        let assets = local_images.lock().unwrap().assets.clone();

        assert!(
            !assets.is_empty(),
            "validation document should register local images"
        );
        assert!(
            !html.contains("data:image/"),
            "preview HTML must not inline local image bytes"
        );
        let mut total_bytes = 0usize;
        for key in assets.keys() {
            let response = local_image_protocol_response(&local_images, &format!("/{key}"));
            assert_eq!(response.status(), 200, "registered image {key} should load");
            total_bytes += response.body().len();
            let decoded = image::load_from_memory(response.body().as_ref())
                .expect("registered image should be decodable");
            use image::GenericImageView;
            let (width, height) = decoded.dimensions();
            assert!(
                width > 0 && height > 0,
                "registered image {key} has no pixels"
            );
        }
        eprintln!(
            "validated {} local images ({} bytes); rendered HTML is {} bytes",
            assets.len(),
            total_bytes,
            html.len()
        );
    }

    #[test]
    fn page_includes_outline_sidebar_navigation() {
        let strings = Strings::for_lang(Lang::Zh);
        let page = build_page(
            &md_to_html("# 标题\n\n## 小节"),
            "# 标题\n\n## 小节",
            None,
            EnhanceFlags::default(),
            &strings,
            false,
            true,
        );

        assert!(page.contains(r#"<aside id="toc-panel""#));
        assert!(page.contains(r#"<nav id="toc"></nav>"#));
        assert!(page.contains(r#"id="btn-toc""#));
        assert!(page.contains("body.toc-open.has-toc #toc-panel"));
        assert!(page.contains("function buildToc()"));
        assert!(page.contains("function updateTocActive()"));
        assert!(page.contains("readingRoot.querySelectorAll('h1[id]"));
        assert!(page.contains("md-preview-toc-open-v1"));
        assert!(page.contains(">目录</div>"));
        assert!(page.contains("章节导航"));
        // Print and editing modes must not leave the sidebar visible.
        assert!(page.contains(".toolbar, .tabbar, #editor, #toc-panel"));
        assert!(page.contains("body.editing #toc-panel { display: none !important; }"));
    }

    #[test]
    fn page_includes_about_popover_tab_menu_and_pdf_export() {
        let strings = Strings::for_lang(Lang::Zh);
        let page = build_page(
            &md_to_html("# 标题"),
            "# 标题",
            None,
            EnhanceFlags::default(),
            &strings,
            false,
            true,
        );

        // About popover next to "+" with developer/version/release date.
        assert!(page.contains(r#"id="tab-about""#));
        assert!(page.contains(r#"id="about-popover""#));
        assert!(page.contains("Rowin"));
        assert!(page.contains(RELEASE_DATE));
        assert!(page.contains("开发者"));
        // Tab context menu with "reveal in folder".
        assert!(page.contains(r#"id="tab-menu""#));
        assert!(page.contains("打开文件所在文件夹"));
        assert!(page.contains("reveal-tab:"));
        // Print popover with direct print + PDF export actions.
        assert!(page.contains(r#"id="print-control""#));
        assert!(page.contains(r#"id="pdf-export""#));
        assert!(page.contains(r#"export-pdf\n"#));
        // Print styles: tables shrink to the page instead of clipping.
        assert!(page.contains("table-layout: fixed"));
        // Editing mode hides the whole print control, not just the button.
        assert!(page.contains("body.editing #print-control"));
    }

    #[test]
    fn page_includes_mermaid_lightbox_viewer() {
        let strings = Strings::for_lang(Lang::Zh);
        let page = build_page(
            &md_to_html("# 标题"),
            "# 标题",
            None,
            EnhanceFlags::default(),
            &strings,
            false,
            true,
        );

        // Lightbox viewer (created lazily by preview-enhance.js).
        assert!(page.contains("mdp-lightbox"));
        assert!(page.contains("__mdPreviewLightboxLabels"));
        assert!(page.contains("下载 PNG（高清 3×）"));
        assert!(page.contains("下载 SVG（矢量原图）"));
        assert!(page.contains("适应窗口"));
        assert!(page.contains("按住空格键并拖动"));
        // Diagrams advertise zoomability and downloads route through IPC.
        assert!(page.contains("cursor: zoom-in"));
        assert!(page.contains("cursor: grab"));
        assert!(page.contains("setPointerCapture"));
        assert!(page.contains(r#"download-svg\n"#));
        assert!(page.contains(r#"download-png\n"#));
        // Word fallback is sampled at 300 PPI while retaining logical SVG size.
        assert!(page.contains("var scale = 300 / 96"));
        assert!(page.contains("mdp-logical-size="));
        // The lightbox never leaks into print/PDF output.
        assert!(
            page.contains(".mdp-lightbox { display: none !important; }")
                || page.contains(", .mdp-lightbox { display: none !important; }")
        );
    }

    #[test]
    fn page_blocks_native_preview_reload_paths() {
        let strings = Strings::for_lang(Lang::En);
        let page = build_page(
            &md_to_html("# Hello"),
            "# Hello",
            None,
            EnhanceFlags::default(),
            &strings,
            false,
            true,
        );

        assert!(page.contains("document.addEventListener('contextmenu'"));
        assert!(page.contains("window.ipc.postMessage('refresh')"));
        assert!(page.contains("id=\"btn-open\""));
        assert!(page.contains("id=\"btn-search\""));
        assert!(page.contains("id=\"replace-input\""));
        assert!(page.contains("id=\"replace-one\""));
        assert!(page.contains("id=\"replace-all\""));
        assert!(page.contains("window.ipc.postMessage('open')"));
        assert!(page.contains("window.__mdPreviewOpenFile = openFile"));
        assert!(page.contains("window.__mdPreviewShowFind = showFind"));
        assert!(page.contains("window.__mdPreviewShowReplace = showReplace"));
        assert!(page.contains("window.__mdPreviewToggleEdit"));
        assert!(page.contains("window.__mdPreviewCheckUpdates"));
        assert!(page.contains("update-check-result:available"));
        assert!(page.contains("update-check-result:"));
        assert!(page.contains("Cmd/Ctrl+F"));
        assert!(page.contains("e.ctrlKey && !e.metaKey"));
        assert!(page.contains("showReplace();"));
        assert!(page.contains("if (inReplaceMode()) hideFind();"));
        assert!(page.contains("body.editing #btn-open"));
        assert!(page.contains("ta.focus({ preventScroll: true })"));
        assert!(page.contains("window.__setEmptyPreview"));
        assert!(page.contains("id=\"tabbar\""));
        assert!(page.contains("window.__setTabs"));
        assert!(page.contains("tab-action:'));") || page.contains("'tab-action:' + action"));
        assert!(page.contains("window.__markSaved"));
        assert!(
            !page.contains("window.ipc.postMessage('save:' + ta.value);\n\t    setDirty(false);")
        );
        assert!(page.contains("releases?per_page=20"));
        assert!(page.contains("nativeUpdater: true"));
        assert!(page.contains("compositionstart"));
        assert!(page.contains("e.isComposing"));
        assert!(page.contains("focusFindInput"));
        assert!(page.contains("FIND_DEBOUNCE_MS = 300"));
        assert!(page.contains("document.createTreeWalker(previewEl, NodeFilter.SHOW_TEXT"));
        assert!(page.contains("mark.className = 'search-hit'"));
        assert!(page.contains("selectFindHit(currentFindHit + (backward ? -1 : 1))"));
        assert!(page.contains("#preview mark.search-hit.current"));
        assert!(page.contains("restoreFindInput(selectionStart, selectionEnd)"));
        assert!(page.contains("findInput.setSelectionRange(selectionStart, selectionEnd)"));
        assert!(page.contains("chunks.push(replacement)"));
        assert!(page.contains("lastReplaceSource !== ta.value"));
        assert!(page.contains("handleEditorInput(true)"));
        assert!(page.contains("body.empty .toolbar.has-update"));
        assert!(page.contains("bindAnchorNavigation"));
        assert!(page.contains("event.target.closest('#preview a[href]')"));
        assert!(page.contains("window.ipc.postMessage('open-local-link:' + resolved)"));
        assert!(page.contains(".markdown-alert-important"));
        assert!(page.contains(".markdown-alert-title"));
        let light_alert = page.find("background: #dafbe1").unwrap();
        let dark_alert = page.rfind("background: #161b22; color: #d4d4d4").unwrap();
        assert!(dark_alert > light_alert);
        assert!(page.contains(".mdp-mark"));
        assert!(!page.contains("Local-first Markdown preview for AI-generated docs"));
    }

    // Verification helper: dump the generated page to DUMP_PAGE so the
    // embedded JS can be syntax-checked with `node --check` externally.
    #[test]
    fn dump_page_for_js_syntax_check() {
        let Ok(path) = std::env::var("DUMP_PAGE") else {
            return;
        };
        let strings = Strings::for_lang(Lang::Zh);
        let page = build_page(
            &md_to_html("# Hello"),
            "# Hello",
            None,
            EnhanceFlags {
                math: true,
                mermaid: true,
            },
            &strings,
            false,
            true,
        );
        std::fs::write(path, page).unwrap();
    }

    #[test]
    fn close_flow_acknowledges_when_nothing_to_save() {
        let strings = Strings::for_lang(Lang::En);
        let page = build_page(
            &md_to_html("# Hello"),
            "# Hello",
            None,
            EnhanceFlags::default(),
            &strings,
            false,
            true,
        );
        // JS save() must answer a pending window close even when clean,
        // otherwise the Rust side swallows CloseRequested forever.
        assert!(page.contains("window.ipc.postMessage('quit-ack')"));
    }

    #[test]
    fn utc_now_string_formats_civil_time() {
        let stamp = utc_now_string();
        assert_eq!(stamp.len(), 27); // "YYYY-MM-DD HH:MM:SS.mmm UTC"
        assert!(stamp.ends_with(" UTC"));
        assert!(stamp.starts_with("20"));
    }

    #[test]
    fn page_separates_new_file_from_open_and_debounces_autosave() {
        let strings = Strings::for_lang(Lang::En);
        let page = build_page(
            &md_to_html("# Hello"),
            "# Hello",
            None,
            EnhanceFlags::default(),
            &strings,
            false,
            true,
        );

        assert!(page.contains("window.ipc.postMessage('new-file')"));
        assert!(page.contains("tabOpen.addEventListener('click', newFile)"));
        assert!(!page.contains("tabOpen.addEventListener('click', openFile)"));
        assert!(page.contains("AUTOSAVE_DEBOUNCE_MS = 700"));
        assert!(page.contains("pendingAutosaveTimer = setTimeout"));
        assert!(page.contains("window.__markSaved = function(savedRaw)"));
        assert!(page.contains("window.__mdPreviewResolveExternalChange"));
        assert!(page.contains("'external-change:' + (dirty ? 'dirty' : 'clean')"));
        assert!(page.contains("(e.key === 'n' || e.key === 'N')"));
        assert!(page.contains("id=\"doc-stats\""));
        assert!(page.contains("updateDocumentStats"));
        assert!(page.contains("restoreScrollProgress"));
        assert!(page.contains("md-preview-content-zoom-v1"));
        assert!(page.contains("id=\"btn-zoom-in\""));
        assert!(page.contains("id=\"btn-zoom-out\""));
        assert!(page.contains("id=\"btn-zoom-reset\""));
        assert!(page.contains("id=\"btn-split\""));
        assert!(page.contains("body.split-view #app"));
        assert!(page.contains("window.__mdPreviewToggleSplit"));
    }

    #[test]
    fn closing_the_last_editing_tab_restores_the_empty_preview() {
        let strings = Strings::for_lang(Lang::En);
        let page = build_page(
            &md_to_html("# Hello"),
            "# Hello",
            None,
            EnhanceFlags::default(),
            &strings,
            false,
            true,
        );
        let empty_start = page.find("window.__setEmptyPreview = function").unwrap();
        let missing_start = page.find("window.__setMissing = function").unwrap();
        let empty_handler = &page[empty_start..missing_start];

        assert!(empty_handler.contains("document.body.classList.remove('editing')"));
        assert!(empty_handler.contains("document.body.classList.remove('split-view')"));
        assert!(empty_handler.contains("syncViewControls()"));
    }

    #[test]
    fn new_markdown_path_keeps_markdown_extensions_and_replaces_other_extensions() {
        assert_eq!(
            normalize_new_markdown_path(PathBuf::from("/tmp/plan")),
            PathBuf::from("/tmp/plan.md")
        );
        assert_eq!(
            normalize_new_markdown_path(PathBuf::from("/tmp/plan.markdown")),
            PathBuf::from("/tmp/plan.markdown")
        );
        assert_eq!(
            normalize_new_markdown_path(PathBuf::from("/tmp/plan.txt")),
            PathBuf::from("/tmp/plan.md")
        );
    }

    #[test]
    fn page_expands_multi_column_tables() {
        let strings = Strings::for_lang(Lang::En);
        let page = build_page(
            &md_to_html("| A | B | C | D |\n|---|---|---|---|\n| 1 | 2 | 3 | 4 |"),
            "",
            None,
            EnhanceFlags::default(),
            &strings,
            false,
            false,
        );

        assert!(page.contains("mdp-table-wrap"));
        assert!(page.contains("width: min(calc(100vw - 64px), 1280px)"));
        assert!(page.contains("if(window.__enhancePreview)window.__enhancePreview();"));
        assert!(page.contains("nativeUpdater: false"));
    }

    #[test]
    fn update_download_urls_are_allowed() {
        assert!(is_allowed_update_url(
            "https://github.com/galen2027/md-studio/releases/download/v1.1.9/MD-Preview-macOS-universal.dmg"
        ));
        assert!(!is_allowed_update_url(
            "https://github.com/other/project/releases/download/v1.0.0/app.dmg"
        ));
        assert!(!is_allowed_update_url(
            "https://github.com/vorojar/md-preview/releases/download/v9.0.0/MD-Preview-windows-x64.exe"
        ));
    }

    #[test]
    fn update_versions_compare_semver_tags() {
        assert!(is_newer_version("v1.1.21", "1.1.20"));
        assert!(is_newer_version("v1.2.0", "1.1.99"));
        assert!(!is_newer_version("v1.1.20", "1.1.21"));
        assert!(!is_newer_version("v1.1.21", "1.1.21"));
        assert!(!is_newer_version("not-a-version", "1.1.21"));
    }

    #[test]
    fn update_release_selection_ignores_older_and_prerelease_versions() {
        let payload = r#"[
          {"tag_name":"v1.1.22-beta","draft":false,"prerelease":true,"html_url":"https://example.invalid/beta","assets":[]},
          {"tag_name":"v1.1.20","draft":false,"prerelease":false,"html_url":"https://example.invalid/old","assets":[]},
          {"tag_name":"v1.1.22","draft":false,"prerelease":false,"html_url":"https://example.invalid/new","assets":[
            {"name":"MD-Preview-macOS-universal.dmg","browser_download_url":"https://example.invalid/app.dmg","digest":"sha256:abc"}
          ]}
        ]"#;

        let release = select_update_release(payload, "1.1.21").unwrap();
        assert_eq!(release.tag, "v1.1.22");
        if cfg!(target_os = "macos") {
            assert_eq!(release.url, "https://example.invalid/app.dmg");
            assert_eq!(release.digest.as_deref(), Some("sha256:abc"));
        } else {
            assert_eq!(release.url, "https://example.invalid/new");
        }
        assert!(select_update_release(payload, "1.1.22").is_none());
    }

    #[test]
    fn empty_state_exposes_open_and_recent_files() {
        let strings = Strings::for_lang(Lang::En);
        let recent = vec![PathBuf::from("/tmp/example.md")];
        let html = empty_preview_html(&strings, &recent);

        assert!(html.contains("Open File"));
        assert!(html.contains("Recent"));
        assert!(html.contains("example.md"));
        assert!(html.contains("data-recent-index=\"0\""));
        assert!(html.contains(r#"<div class="empty has-recent">"#));
        assert!(html.contains(r#"<div class="icon">#</div>"#));
        assert!(!html.contains("empty-mark"));

        let page = build_page(
            &html,
            "",
            None,
            EnhanceFlags::default(),
            &strings,
            true,
            false,
        );
        assert!(page.contains(".empty.has-recent"));
        assert!(!page.contains(".empty.has-recent .recent { max-height"));
    }

    #[test]
    fn vim_style_target_rewrite_events_reload_current_file() {
        let target = PathBuf::from("/tmp/note.md");
        let ev = Event::new(EventKind::Create(notify::event::CreateKind::File))
            .add_path(PathBuf::from("/tmp/note.md"));

        assert!(event_should_reload_file(&ev, &target));
    }

    #[test]
    fn sibling_file_events_do_not_reload_current_file() {
        let target = PathBuf::from("/tmp/note.md");
        let ev = Event::new(EventKind::Modify(notify::event::ModifyKind::Data(
            notify::event::DataChange::Any,
        )))
        .add_path(PathBuf::from("/tmp/other.md"));

        assert!(!event_should_reload_file(&ev, &target));
    }

    #[test]
    fn self_write_suppression_checks_disk_content_not_only_time() {
        let dir = temp_test_dir("self-write-suppression");
        let path = dir.join("note.md");
        fs::write(&path, "saved by app").unwrap();
        let record = SelfWriteRecord {
            path: path.clone(),
            content: "saved by app".to_string(),
        };

        assert!(self_write_still_matches_disk(Some(&record), &path));

        fs::write(&path, "external edit").unwrap();
        assert!(!self_write_still_matches_disk(Some(&record), &path));
    }

    #[test]
    fn external_change_protection_uses_dirty_state_from_either_side() {
        assert!(!should_protect_external_change(false, false));
        assert!(should_protect_external_change(true, false));
        assert!(should_protect_external_change(false, true));
        assert!(should_protect_external_change(true, true));
    }

    #[test]
    fn file_watch_scope_is_parent_directory() {
        let target = PathBuf::from("/tmp/note.md");

        assert_eq!(watch_scope_for_file(&target), Path::new("/tmp"));
    }

    #[test]
    fn finder_action_parses_encoded_folder_and_kind() {
        assert_eq!(
            parse_finder_action(
                "mdpreview://finder?action=create&path=%2Ftmp%2FMy%20Notes&kind=md"
            ),
            Some(FinderAction::Create {
                folder: PathBuf::from("/tmp/My Notes"),
                kind: "md".to_string(),
            })
        );
        assert!(parse_finder_action("https://example.com/").is_none());
    }

    #[test]
    fn finder_create_uses_non_conflicting_markdown_name() {
        let dir = temp_test_dir("finder-create");
        fs::write(dir.join("新建.md"), "existing").unwrap();

        let created = create_finder_file(&dir, "md").unwrap();

        assert_eq!(created.file_name().unwrap(), "新建 2.md");
        assert_eq!(fs::read_to_string(created).unwrap(), "");
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_association_shell_path_removes_verbatim_prefixes() {
        assert_eq!(
            windows_shell_path(Path::new(r"\\?\C:\Tools\中文 目录\Markdown Studio.exe")),
            r"C:\Tools\中文 目录\Markdown Studio.exe"
        );
        assert_eq!(
            windows_shell_path(Path::new(r"\\?\UNC\server\共享 目录\md-preview.exe")),
            r"\\server\共享 目录\md-preview.exe"
        );
        assert_eq!(
            windows_shell_path(Path::new(r"C:\Tools\md-preview.exe")),
            r"C:\Tools\md-preview.exe"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_association_plan_quotes_paths_and_keeps_legacy_default_compatible() {
        let plan = windows_association_plan(Path::new(
            r"\\?\C:\Tools\中文 目录\MD-Preview-windows-x64.exe",
        ))
        .unwrap();

        assert_eq!(
            plan.executable_path,
            r"C:\Tools\中文 目录\MD-Preview-windows-x64.exe"
        );
        assert_eq!(
            plan.open_command,
            r#""C:\Tools\中文 目录\MD-Preview-windows-x64.exe" "%1""#
        );
        assert_eq!(
            plan.icon_value,
            r#""C:\Tools\中文 目录\MD-Preview-windows-x64.exe",0"#
        );
        assert_eq!(
            plan.application_names,
            vec![
                "MD-Preview-windows-x64.exe".to_string(),
                "md-preview.exe".to_string()
            ]
        );
        assert!(!plan.open_command.contains(r"\\?\"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_association_registration_requires_marker_and_every_command() {
        let plan = windows_association_plan(Path::new(r"C:\Tools\md-preview.exe")).unwrap();
        let expected = plan.open_command.as_str();

        assert!(windows_association_values_are_current(
            &plan,
            Some(concat!(r"C:\Tools\md-preview.exe", "\r\n")),
            Some(expected),
            &[Some(expected)]
        ));
        assert!(!windows_association_values_are_current(
            &plan,
            Some(r"C:\Tools\md-preview.exe"),
            Some(expected),
            &[Some(r#""C:\Old\md-preview.exe" "%1""#)]
        ));
        assert!(!windows_association_values_are_current(
            &plan,
            Some(r"C:\Tools\md-preview.exe"),
            None,
            &[Some(expected)]
        ));

        let renamed =
            windows_association_plan(Path::new(r"C:\Tools\MD-Preview-windows-x64.exe")).unwrap();
        let renamed_command = renamed.open_command.as_str();
        assert!(!windows_association_values_are_current(
            &renamed,
            Some(r"C:\Tools\MD-Preview-windows-x64.exe"),
            Some(renamed_command),
            &[Some(renamed_command), None]
        ));
        assert!(windows_association_values_are_current(
            &renamed,
            Some(r"C:\Tools\MD-Preview-windows-x64.exe"),
            Some(renamed_command),
            &[Some(renamed_command), Some(renamed_command)]
        ));
    }
}

/// Decode embedded icon.ico to an RGBA tao Icon for the window chrome.
fn load_window_icon() -> Option<tao::window::Icon> {
    let img = image::load_from_memory_with_format(ICON_BYTES, image::ImageFormat::Ico).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    tao::window::Icon::from_rgba(rgba.into_raw(), w, h).ok()
}

#[cfg(target_os = "windows")]
const WINDOWS_COMPAT_APPLICATION_EXE: &str = "md-preview.exe";

#[cfg(target_os = "windows")]
#[derive(Debug, PartialEq, Eq)]
struct WindowsAssociationPlan {
    executable_path: String,
    open_command: String,
    icon_value: String,
    application_names: Vec<String>,
}

/// Convert a filesystem path into the ordinary Win32 form accepted by the
/// Windows shell. `fs::canonicalize` commonly produces a verbatim `\\?\`
/// path, but Explorer rejects that form when it appears in a file-association
/// command even though CreateProcess can launch it directly.
#[cfg(target_os = "windows")]
fn windows_shell_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        raw.into_owned()
    }
}

#[cfg(target_os = "windows")]
fn windows_association_plan(executable: &Path) -> Option<WindowsAssociationPlan> {
    let executable_path = windows_shell_path(executable);
    let executable_name = executable.file_name()?.to_string_lossy().into_owned();
    let mut application_names = vec![executable_name.clone()];
    if !executable_name.eq_ignore_ascii_case(WINDOWS_COMPAT_APPLICATION_EXE) {
        application_names.push(WINDOWS_COMPAT_APPLICATION_EXE.to_string());
    }

    Some(WindowsAssociationPlan {
        open_command: format!(r#""{executable_path}" "%1""#),
        icon_value: format!(r#""{executable_path}",0"#),
        executable_path,
        application_names,
    })
}

#[cfg(target_os = "windows")]
fn windows_association_value_matches(actual: Option<&str>, expected: &str) -> bool {
    actual.is_some_and(|value| value.trim().eq_ignore_ascii_case(expected))
}

#[cfg(target_os = "windows")]
fn windows_association_values_are_current(
    plan: &WindowsAssociationPlan,
    marker_value: Option<&str>,
    progid_open_command: Option<&str>,
    application_open_commands: &[Option<&str>],
) -> bool {
    windows_association_value_matches(marker_value, &plan.executable_path)
        && windows_association_value_matches(progid_open_command, &plan.open_command)
        && application_open_commands.len() == plan.application_names.len()
        && application_open_commands
            .iter()
            .all(|command| windows_association_value_matches(*command, &plan.open_command))
}

#[cfg(target_os = "windows")]
fn windows_registry_default_value(root: &winreg::RegKey, path: &str) -> Option<String> {
    root.open_subkey(path).ok()?.get_value("").ok()
}

#[cfg(target_os = "windows")]
fn windows_association_registry_is_current(
    hkcu: &winreg::RegKey,
    marker_value: Option<&str>,
    plan: &WindowsAssociationPlan,
    progid: &str,
) -> bool {
    let progid_root = format!(r"Software\Classes\{progid}");
    let progid_command =
        windows_registry_default_value(hkcu, &format!(r"{progid_root}\shell\open\command"));
    let icon_value = windows_registry_default_value(hkcu, &format!(r"{progid_root}\DefaultIcon"));
    let application_commands = plan
        .application_names
        .iter()
        .map(|name| {
            windows_registry_default_value(
                hkcu,
                &format!(r"Software\Classes\Applications\{name}\shell\open\command"),
            )
        })
        .collect::<Vec<_>>();
    let application_command_refs = application_commands
        .iter()
        .map(|command| command.as_deref())
        .collect::<Vec<_>>();
    let advertised_for_every_extension =
        [".md", ".markdown", ".mdown", ".mkd"]
            .iter()
            .all(|extension| {
                hkcu.open_subkey(format!(r"Software\Classes\{extension}\OpenWithProgids"))
                    .ok()
                    .is_some_and(|key: winreg::RegKey| key.get_raw_value(progid).is_ok())
            });

    windows_association_values_are_current(
        plan,
        marker_value,
        progid_command.as_deref(),
        &application_command_refs,
    ) && windows_association_value_matches(icon_value.as_deref(), &plan.icon_value)
        && advertised_for_every_extension
}

#[cfg(target_os = "macos")]
fn register_as_default(_lang: Lang) {
    use std::process::Command;
    let marker = config_dir().join(".md-preview-registered");
    if marker.exists() {
        return;
    }
    let _ = Command::new("swift")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                let _ = stdin.write_all(b"import Foundation\nimport CoreServices\nlet _ = LSSetDefaultRoleHandlerForContentType(\"net.daringfireball.markdown\" as NSString, .viewer, \"com.mdpreview.app\" as NSString)\n");
            }
            child.wait()
        });
    let _ = fs::create_dir_all(marker.parent().unwrap());
    let _ = fs::write(&marker, "");
}

/// Windows: write HKCU registry so .md shows up in the "Open with" list, then
/// prompt the user once to finish wiring the default app (Win8+ blocks silent
/// default-handler changes — only the Settings app can confirm it).
#[cfg(target_os = "windows")]
fn register_as_default(_lang: Lang) {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let marker_dir = config_dir();
    let marker = marker_dir.join(".md-preview-registered");
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(plan) = windows_association_plan(&exe) else {
        return;
    };
    let progid = "MDPreview.md";
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let marker_value = fs::read_to_string(&marker).ok();
    // A copied/moved executable must refresh the file-association command;
    // the marker is only a cache and cannot override changed or missing
    // registry values. Validate the stable legacy application key as well as
    // the current executable name because Windows may keep either as the
    // protected UserChoice ProgID after an upgrade or rename.
    if windows_association_registry_is_current(&hkcu, marker_value.as_deref(), &plan, progid) {
        return;
    }

    // Advertise Markdown Studio as a choice for these extensions.
    for ext in [".md", ".markdown", ".mdown", ".mkd"] {
        let path = format!(r"Software\Classes\{ext}\OpenWithProgids");
        if let Ok((key, _)) = hkcu.create_subkey(&path) {
            let _ = key.set_value::<String, _>(progid, &String::new());
        }
    }

    // ProgID definition: description, icon, open command.
    let progid_root = format!(r"Software\Classes\{progid}");
    if let Ok((k, _)) = hkcu.create_subkey(&progid_root) {
        let _ = k.set_value("", &"Markdown Document".to_string());
        let _ = k.set_value("FriendlyTypeName", &"Markdown Document".to_string());
    }
    if let Ok((k, _)) = hkcu.create_subkey(format!(r"{progid_root}\DefaultIcon")) {
        let _ = k.set_value("", &plan.icon_value);
    }
    if let Ok((k, _)) = hkcu.create_subkey(format!(r"{progid_root}\shell\open\command")) {
        let _ = k.set_value("", &plan.open_command);
    }

    // Keep both the current executable basename and the historical stable
    // basename. Windows protects UserChoice, so an existing
    // `Applications\md-preview.exe` selection must remain launchable even when
    // the distributed binary is named `MD-Preview-windows-x64.exe`.
    for exe_name in &plan.application_names {
        let app_root = format!(r"Software\Classes\Applications\{exe_name}");
        if let Ok((k, _)) = hkcu.create_subkey(&app_root) {
            let _ = k.set_value("FriendlyAppName", &"Markdown Studio".to_string());
        }
        if let Ok((k, _)) = hkcu.create_subkey(format!(r"{app_root}\shell\open\command")) {
            let _ = k.set_value("", &plan.open_command);
        }
        if let Ok((k, _)) = hkcu.create_subkey(format!(r"{app_root}\SupportedTypes")) {
            for ext in [".md", ".markdown", ".mdown", ".mkd"] {
                let _ = k.set_value::<String, _>(ext, &String::new());
            }
        }
    }

    let _ = fs::create_dir_all(&marker_dir);
    // Only refresh the marker after the registry reflects the complete plan.
    // If policy blocks any write, the next launch will retry instead of
    // trusting a stale marker forever.
    if windows_association_registry_is_current(
        &hkcu,
        Some(plan.executable_path.as_str()),
        &plan,
        progid,
    ) {
        let _ = fs::write(&marker, &plan.executable_path);
    }
    // Intentionally no dialog: users can pick Markdown Studio via "Open with"
    // whenever they want, and Win10+ blocks silent default-handler changes
    // anyway — asking them to click through Settings on first launch is noise.
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn register_as_default(_lang: Lang) {}

const WEBSITE_URL: &str = "https://github.com/galen2027/md-studio#readme";
const GITHUB_URL: &str = "https://github.com/galen2027/md-studio";
#[cfg(target_os = "macos")]
const RELEASES_URL: &str = "https://github.com/galen2027/md-studio/releases/latest";

#[cfg(target_os = "macos")]
thread_local! {
    static MACOS_MENU_PROXY: std::cell::RefCell<Option<EventLoopProxy<UserEvent>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(target_os = "macos")]
fn send_macos_menu_event(event: UserEvent) {
    MACOS_MENU_PROXY.with(|cell| {
        if let Some(proxy) = cell.borrow().as_ref() {
            let _ = proxy.send_event(event);
        }
    });
}

#[cfg(target_os = "macos")]
fn macos_menu_controller_class() -> &'static objc2::runtime::AnyClass {
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, NSObject, Sel};
    use objc2::{sel, ClassType, MainThreadOnly};
    use objc2_app_kit::{
        NSAlert, NSAlertStyle, NSButton, NSControlStateValueOff, NSControlStateValueOn, NSImage,
        NSMenuItem, NSView,
    };
    use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};
    use std::sync::Once;

    extern "C" fn open_file(_: &AnyObject, _: Sel, _: &AnyObject) {
        send_macos_menu_event(UserEvent::OpenFile);
    }

    extern "C" fn new_file(_: &AnyObject, _: Sel, _: &AnyObject) {
        send_macos_menu_event(UserEvent::NewFile);
    }

    extern "C" fn close_tab(_: &AnyObject, _: Sel, _: &AnyObject) {
        send_macos_menu_event(UserEvent::CloseActiveTab);
    }

    extern "C" fn show_find(_: &AnyObject, _: Sel, _: &AnyObject) {
        send_macos_menu_event(UserEvent::ShowFind);
    }

    extern "C" fn toggle_edit(_: &AnyObject, _: Sel, _: &AnyObject) {
        send_macos_menu_event(UserEvent::ToggleEdit);
    }

    extern "C" fn print(_: &AnyObject, _: Sel, _: &AnyObject) {
        send_macos_menu_event(UserEvent::Print);
    }

    extern "C" fn check_updates(_: &AnyObject, _: Sel, _: &AnyObject) {
        send_macos_menu_event(UserEvent::CheckUpdates);
    }

    extern "C" fn quit(_: &AnyObject, _: Sel, _: &AnyObject) {
        send_macos_menu_event(UserEvent::Quit);
    }

    extern "C" fn open_website(_: &AnyObject, _: Sel, _: &AnyObject) {
        send_macos_menu_event(UserEvent::OpenUrl(WEBSITE_URL));
    }

    extern "C" fn open_github(_: &AnyObject, _: Sel, _: &AnyObject) {
        send_macos_menu_event(UserEvent::OpenUrl(GITHUB_URL));
    }

    extern "C" fn open_releases(_: &AnyObject, _: Sel, _: &AnyObject) {
        send_macos_menu_event(UserEvent::OpenUrl(RELEASES_URL));
    }

    extern "C" fn set_theme(_: &AnyObject, _: Sel, sender: &NSMenuItem) {
        let choice = match sender.tag() {
            102 => ThemeChoice::Light,
            103 => ThemeChoice::Dark,
            _ => ThemeChoice::System,
        };
        let selected = sender.tag();
        if let Some(menu) = unsafe { sender.menu() } {
            for index in 0..menu.numberOfItems() {
                if let Some(item) = menu.itemAtIndex(index) {
                    let tag = item.tag();
                    if (101..=103).contains(&tag) {
                        item.setState(if tag == selected {
                            NSControlStateValueOn
                        } else {
                            NSControlStateValueOff
                        });
                    }
                }
            }
        }
        send_macos_menu_event(UserEvent::SetTheme(choice));
    }

    fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
    }

    fn symbol_button(
        symbol: &str,
        fallback_title: &str,
        tooltip: &str,
        action: Sel,
        target: &AnyObject,
        mtm: MainThreadMarker,
    ) -> objc2::rc::Retained<NSButton> {
        let accessibility = NSString::from_str(tooltip);
        let symbol_name = NSString::from_str(symbol);
        let button = if let Some(image) =
            NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &symbol_name,
                Some(&accessibility),
            ) {
            unsafe {
                NSButton::buttonWithImage_target_action(&image, Some(target), Some(action), mtm)
            }
        } else {
            unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(fallback_title),
                    Some(target),
                    Some(action),
                    mtm,
                )
            }
        };
        button.setBordered(false);
        button.setToolTip(Some(&accessibility));
        button
    }

    extern "C" fn show_about(controller: &AnyObject, _: Sel, _: &AnyObject) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };

        let alert = NSAlert::new(mtm);
        alert.setAlertStyle(NSAlertStyle::Informational);
        alert.setMessageText(&NSString::from_str("Markdown Studio"));
        alert.setInformativeText(&NSString::from_str(&format!(
            "Version {}\n\nFollow local document links between lightweight tabs, keep your reading position between preview and source, and inspect character counts or zoom the content without changing the app chrome.",
            env!("CARGO_PKG_VERSION")
        )));

        let accessory = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 94.0, 28.0));
        let home = symbol_button(
            "house",
            "Home",
            "Home",
            sel!(mdPreviewOpenWebsite:),
            controller,
            mtm,
        );
        home.setFrame(rect(4.0, 1.0, 26.0, 26.0));
        accessory.addSubview(&home);

        let github = symbol_button(
            "chevron.left.forwardslash.chevron.right",
            "GitHub",
            "GitHub",
            sel!(mdPreviewOpenGitHub:),
            controller,
            mtm,
        );
        github.setFrame(rect(34.0, 1.0, 26.0, 26.0));
        accessory.addSubview(&github);

        let releases = symbol_button(
            "sparkles",
            "New",
            "What's New",
            sel!(mdPreviewOpenReleases:),
            controller,
            mtm,
        );
        releases.setFrame(rect(64.0, 1.0, 26.0, 26.0));
        accessory.addSubview(&releases);
        alert.setAccessoryView(Some(&accessory));
        alert.addButtonWithTitle(&NSString::from_str("OK"));

        alert.runModal();
    }

    static REGISTER_CLASS: Once = Once::new();
    REGISTER_CLASS.call_once(|| {
        let mut builder = ClassBuilder::new(c"MDPreviewMenuController", NSObject::class()).unwrap();
        unsafe {
            builder.add_method(
                sel!(mdPreviewOpenFile:),
                open_file as extern "C" fn(_, _, _),
            );
            builder.add_method(sel!(mdPreviewNewFile:), new_file as extern "C" fn(_, _, _));
            builder.add_method(
                sel!(mdPreviewCloseTab:),
                close_tab as extern "C" fn(_, _, _),
            );
            builder.add_method(
                sel!(mdPreviewShowFind:),
                show_find as extern "C" fn(_, _, _),
            );
            builder.add_method(
                sel!(mdPreviewToggleEdit:),
                toggle_edit as extern "C" fn(_, _, _),
            );
            builder.add_method(sel!(mdPreviewPrint:), print as extern "C" fn(_, _, _));
            builder.add_method(
                sel!(mdPreviewCheckUpdates:),
                check_updates as extern "C" fn(_, _, _),
            );
            builder.add_method(sel!(mdPreviewQuit:), quit as extern "C" fn(_, _, _));
            builder.add_method(
                sel!(mdPreviewOpenWebsite:),
                open_website as extern "C" fn(_, _, _),
            );
            builder.add_method(
                sel!(mdPreviewOpenGitHub:),
                open_github as extern "C" fn(_, _, _),
            );
            builder.add_method(
                sel!(mdPreviewOpenReleases:),
                open_releases as extern "C" fn(_, _, _),
            );
            builder.add_method(
                sel!(mdPreviewSetTheme:),
                set_theme as extern "C" fn(_, _, _),
            );
            builder.add_method(
                sel!(mdPreviewShowAbout:),
                show_about as extern "C" fn(_, _, _),
            );
        }
        let _ = builder.register();
    });

    AnyClass::get(c"MDPreviewMenuController").unwrap()
}

#[cfg(target_os = "macos")]
fn install_macos_menu(proxy: EventLoopProxy<UserEvent>, theme: ThemeChoice) {
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Sel};
    use objc2::{msg_send, sel, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSControlStateValueOff, NSControlStateValueOn, NSEventModifierFlags, NSMenu,
        NSMenuItem,
    };
    use objc2_foundation::{MainThreadMarker, NSString};

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    MACOS_MENU_PROXY.with(|cell| {
        *cell.borrow_mut() = Some(proxy);
    });

    fn menu(title: &str, mtm: MainThreadMarker) -> objc2::rc::Retained<NSMenu> {
        NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title))
    }

    fn item(
        title: &str,
        action: Option<Sel>,
        key: &str,
        modifiers: NSEventModifierFlags,
        mtm: MainThreadMarker,
    ) -> objc2::rc::Retained<NSMenuItem> {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                action,
                &NSString::from_str(key),
            )
        };
        item.setKeyEquivalentModifierMask(modifiers);
        item
    }

    fn command_item(
        title: &str,
        action: Sel,
        key: &str,
        modifiers: NSEventModifierFlags,
        target: &AnyObject,
        mtm: MainThreadMarker,
    ) -> objc2::rc::Retained<NSMenuItem> {
        let item = item(title, Some(action), key, modifiers, mtm);
        unsafe {
            item.setTarget(Some(target));
        }
        item
    }

    let app = NSApplication::sharedApplication(mtm);
    let main_menu = menu("", mtm);
    let controller: Retained<AnyObject> = unsafe { msg_send![macos_menu_controller_class(), new] };
    let controller_ptr = Retained::into_raw(controller);
    let controller = unsafe { &*controller_ptr };

    let app_menu = menu("Markdown Studio", mtm);
    app_menu.setAutoenablesItems(false);
    app_menu.addItem(&command_item(
        "About Markdown Studio",
        sel!(mdPreviewShowAbout:),
        "",
        NSEventModifierFlags::empty(),
        controller,
        mtm,
    ));
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    app_menu.addItem(&command_item(
        "Markdown Studio Website",
        sel!(mdPreviewOpenWebsite:),
        "",
        NSEventModifierFlags::empty(),
        controller,
        mtm,
    ));
    app_menu.addItem(&command_item(
        "GitHub Repository",
        sel!(mdPreviewOpenGitHub:),
        "",
        NSEventModifierFlags::empty(),
        controller,
        mtm,
    ));
    app_menu.addItem(&command_item(
        "Check for Updates...",
        sel!(mdPreviewCheckUpdates:),
        "",
        NSEventModifierFlags::empty(),
        controller,
        mtm,
    ));
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    app_menu.addItem(&command_item(
        "Quit Markdown Studio",
        sel!(mdPreviewQuit:),
        "q",
        NSEventModifierFlags::Command,
        controller,
        mtm,
    ));
    let app_menu_item = item(
        "Markdown Studio",
        None,
        "",
        NSEventModifierFlags::empty(),
        mtm,
    );
    app_menu_item.setSubmenu(Some(&app_menu));
    main_menu.addItem(&app_menu_item);

    let file_menu = menu("File", mtm);
    file_menu.setAutoenablesItems(false);
    file_menu.addItem(&command_item(
        "New Markdown...",
        sel!(mdPreviewNewFile:),
        "n",
        NSEventModifierFlags::Command,
        controller,
        mtm,
    ));
    file_menu.addItem(&command_item(
        "Open...",
        sel!(mdPreviewOpenFile:),
        "o",
        NSEventModifierFlags::Command,
        controller,
        mtm,
    ));
    file_menu.addItem(&command_item(
        "Close Tab",
        sel!(mdPreviewCloseTab:),
        "w",
        NSEventModifierFlags::Command,
        controller,
        mtm,
    ));
    file_menu.addItem(&NSMenuItem::separatorItem(mtm));
    file_menu.addItem(&command_item(
        "Print...",
        sel!(mdPreviewPrint:),
        "p",
        NSEventModifierFlags::Command,
        controller,
        mtm,
    ));
    let file_menu_item = item("File", None, "", NSEventModifierFlags::empty(), mtm);
    file_menu_item.setSubmenu(Some(&file_menu));
    main_menu.addItem(&file_menu_item);

    let edit_menu = menu("Edit", mtm);
    edit_menu.addItem(&item(
        "Undo",
        Some(sel!(undo:)),
        "z",
        NSEventModifierFlags::Command,
        mtm,
    ));
    edit_menu.addItem(&item(
        "Redo",
        Some(sel!(redo:)),
        "z",
        NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        mtm,
    ));
    edit_menu.addItem(&NSMenuItem::separatorItem(mtm));
    edit_menu.addItem(&item(
        "Cut",
        Some(sel!(cut:)),
        "x",
        NSEventModifierFlags::Command,
        mtm,
    ));
    edit_menu.addItem(&item(
        "Copy",
        Some(sel!(copy:)),
        "c",
        NSEventModifierFlags::Command,
        mtm,
    ));
    edit_menu.addItem(&item(
        "Paste",
        Some(sel!(paste:)),
        "v",
        NSEventModifierFlags::Command,
        mtm,
    ));
    edit_menu.addItem(&item(
        "Select All",
        Some(sel!(selectAll:)),
        "a",
        NSEventModifierFlags::Command,
        mtm,
    ));
    let edit_menu_item = item("Edit", None, "", NSEventModifierFlags::empty(), mtm);
    edit_menu_item.setSubmenu(Some(&edit_menu));
    main_menu.addItem(&edit_menu_item);

    let view_menu = menu("View", mtm);
    view_menu.setAutoenablesItems(false);
    view_menu.addItem(&command_item(
        "Find",
        sel!(mdPreviewShowFind:),
        "",
        NSEventModifierFlags::empty(),
        controller,
        mtm,
    ));
    view_menu.addItem(&command_item(
        "Toggle Edit Mode",
        sel!(mdPreviewToggleEdit:),
        "e",
        NSEventModifierFlags::Command,
        controller,
        mtm,
    ));
    view_menu.addItem(&NSMenuItem::separatorItem(mtm));
    let theme_menu = menu("Theme", mtm);
    theme_menu.setAutoenablesItems(false);
    for (label, choice, tag) in [
        ("System", ThemeChoice::System, 101),
        ("Light", ThemeChoice::Light, 102),
        ("Dark", ThemeChoice::Dark, 103),
    ] {
        let theme_item = command_item(
            label,
            sel!(mdPreviewSetTheme:),
            "",
            NSEventModifierFlags::empty(),
            controller,
            mtm,
        );
        theme_item.setTag(tag);
        theme_item.setState(if choice == theme {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        theme_menu.addItem(&theme_item);
    }
    let theme_menu_item = item("Theme", None, "", NSEventModifierFlags::empty(), mtm);
    theme_menu_item.setSubmenu(Some(&theme_menu));
    view_menu.addItem(&theme_menu_item);
    let view_menu_item = item("View", None, "", NSEventModifierFlags::empty(), mtm);
    view_menu_item.setSubmenu(Some(&view_menu));
    main_menu.addItem(&view_menu_item);

    app.setMainMenu(Some(&main_menu));
}

#[cfg(not(target_os = "macos"))]
fn install_macos_menu(_proxy: EventLoopProxy<UserEvent>, _theme: ThemeChoice) {}

#[cfg(target_os = "macos")]
mod macos_updater {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use std::ffi::{c_char, c_void, CStr, CString};
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    static CONTROLLER: OnceLock<usize> = OnceLock::new();
    static FRAMEWORK_HANDLE: OnceLock<usize> = OnceLock::new();

    const RTLD_NOW: i32 = 0x2;
    const RTLD_GLOBAL: i32 = 0x8;

    unsafe extern "C" {
        fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    }

    fn bundled_framework_path() -> Option<CString> {
        let exe = std::env::current_exe().ok()?;
        let path: PathBuf = exe
            .parent()?
            .join("../Frameworks/Sparkle.framework/Sparkle");
        if !path.exists() {
            return None;
        }
        CString::new(path.to_string_lossy().as_bytes()).ok()
    }

    fn app_bundle_for_exe_path(exe: &Path) -> Option<PathBuf> {
        exe.ancestors()
            .find(|path| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| ext.eq_ignore_ascii_case("app"))
                    .unwrap_or(false)
            })
            .map(Path::to_path_buf)
    }

    fn is_applications_bundle(bundle: &Path, home: Option<&Path>) -> bool {
        let Some(parent) = bundle.parent() else {
            return false;
        };
        if parent == Path::new("/Applications") {
            return true;
        }
        home.map(|home| parent == home.join("Applications"))
            .unwrap_or(false)
    }

    fn allow_non_applications_updater() -> bool {
        std::env::var("MD_PREVIEW_ALLOW_NON_APPLICATIONS_UPDATER")
            .map(|value| value == "1")
            .unwrap_or(false)
    }

    pub fn installer_enabled() -> bool {
        std::env::var("MD_PREVIEW_ENABLE_SPARKLE_INSTALLER")
            .map(|value| value == "1")
            .unwrap_or(false)
    }

    pub fn can_install_updates() -> bool {
        if !installer_enabled() {
            return false;
        }
        if bundled_framework_path().is_none() {
            return false;
        }
        if allow_non_applications_updater() {
            return true;
        }

        let Some(exe) = std::env::current_exe().ok() else {
            return false;
        };
        let Some(bundle) = app_bundle_for_exe_path(&exe) else {
            return false;
        };
        let home = std::env::var_os("HOME").map(PathBuf::from);
        is_applications_bundle(&bundle, home.as_deref())
    }

    fn load_framework() -> bool {
        if FRAMEWORK_HANDLE.get().is_some() {
            return true;
        }
        let Some(path) = bundled_framework_path() else {
            return false;
        };
        let handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW | RTLD_GLOBAL) };
        if handle.is_null() {
            return false;
        }
        let _ = FRAMEWORK_HANDLE.set(handle as usize);
        true
    }

    pub fn start() -> bool {
        if CONTROLLER.get().is_some() {
            return true;
        }
        if !can_install_updates() {
            return false;
        }
        if !load_framework() {
            return false;
        }

        let Some(controller_class) =
            AnyClass::get(CStr::from_bytes_with_nul(b"SPUStandardUpdaterController\0").unwrap())
        else {
            return false;
        };

        let controller: *mut AnyObject = unsafe {
            let allocated: *mut AnyObject = msg_send![controller_class, alloc];
            msg_send![
                allocated,
                initWithStartingUpdater: Bool::YES,
                updaterDelegate: Option::<&AnyObject>::None,
                userDriverDelegate: Option::<&AnyObject>::None
            ]
        };
        if controller.is_null() {
            return false;
        }
        let _ = CONTROLLER.set(controller as usize);
        true
    }

    pub fn check_for_updates() -> bool {
        if !start() {
            return false;
        }
        let Some(ptr) = CONTROLLER.get().copied() else {
            return false;
        };
        let controller = ptr as *mut AnyObject;
        unsafe {
            let _: () = msg_send![controller, checkForUpdates: Option::<&AnyObject>::None];
        }
        true
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::path::Path;

        #[test]
        fn finds_bundle_from_executable_path() {
            let bundle = app_bundle_for_exe_path(Path::new(
                "/Applications/Markdown Studio.app/Contents/MacOS/md-preview",
            ));

            assert_eq!(
                bundle,
                Some(PathBuf::from("/Applications/Markdown Studio.app"))
            );
        }

        #[test]
        fn allows_system_and_user_applications_locations() {
            assert!(is_applications_bundle(
                Path::new("/Applications/Markdown Studio.app"),
                None,
            ));
            assert!(is_applications_bundle(
                Path::new("/Users/me/Applications/Markdown Studio.app"),
                Some(Path::new("/Users/me")),
            ));
            assert!(!is_applications_bundle(
                Path::new("/Volumes/Markdown Studio/Markdown Studio.app"),
                Some(Path::new("/Users/me")),
            ));
            assert!(!is_applications_bundle(
                Path::new("/Users/me/Downloads/Markdown Studio.app"),
                Some(Path::new("/Users/me")),
            ));
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_updater {
    use super::{config_dir, is_allowed_update_url, APP_DIRTY};
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::Ordering;

    pub fn start() -> bool {
        true
    }

    fn ps_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }

    fn valid_digest(digest: &str) -> bool {
        digest
            .strip_prefix("sha256:")
            .map(|hash| hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
            .unwrap_or(false)
    }

    fn relaunch_args(file: Option<PathBuf>) -> String {
        let Some(path) = file else {
            return "@()".to_string();
        };
        format!("@({})", ps_quote(&path.to_string_lossy()))
    }

    pub fn check_for_updates(
        download_url: Option<&str>,
        digest: Option<&str>,
        relaunch_file: Option<PathBuf>,
    ) -> bool {
        if APP_DIRTY.load(Ordering::SeqCst) {
            return false;
        }

        let Some(url) = download_url.filter(|url| is_allowed_update_url(url)) else {
            return false;
        };
        if !url.ends_with("/MD-Preview-windows-x64.exe") {
            return false;
        }
        let Some(expected_digest) = digest.filter(|digest| valid_digest(digest)) else {
            return false;
        };

        let Ok(target) = std::env::current_exe() else {
            return false;
        };
        let pid = std::process::id();
        let update_dir = config_dir().join("updates");
        if fs::create_dir_all(&update_dir).is_err() {
            return false;
        }
        let script_path = update_dir.join(format!("update-{pid}.ps1"));
        let script_path_s = script_path.to_string_lossy();
        let target_s = target.to_string_lossy();
        let args = relaunch_args(relaunch_file);
        let script = format!(
            r#"$ErrorActionPreference = 'Stop'
$target = {target}
$url = {url}
$expected = {expected}
$script = {script}
$pidToWait = {pid}
$tmp = Join-Path ([IO.Path]::GetTempPath()) ('md-preview-update-' + [guid]::NewGuid().ToString() + '.exe')
Invoke-WebRequest -Uri $url -OutFile $tmp -UseBasicParsing
$actual = 'sha256:' + (Get-FileHash -LiteralPath $tmp -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actual -ne $expected.ToLowerInvariant()) {{
  Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
  exit 2
}}
Wait-Process -Id $pidToWait -Timeout 30 -ErrorAction SilentlyContinue
$done = $false
for ($i = 0; $i -lt 80; $i++) {{
  try {{
    Copy-Item -LiteralPath $tmp -Destination $target -Force
    $done = $true
    break
  }} catch {{
    Start-Sleep -Milliseconds 250
  }}
}}
Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
if (-not $done) {{ exit 3 }}
Start-Process -FilePath $target -ArgumentList {args}
Remove-Item -LiteralPath $script -Force -ErrorAction SilentlyContinue
"#,
            target = ps_quote(&target_s),
            url = ps_quote(url),
            expected = ps_quote(expected_digest),
            script = ps_quote(&script_path_s),
            pid = pid,
            args = args,
        );
        if fs::write(&script_path, script).is_err() {
            return false;
        }

        let spawned = Command::new("powershell.exe")
            .arg("-NoProfile")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-WindowStyle")
            .arg("Hidden")
            .arg("-File")
            .arg(&script_path)
            .spawn()
            .is_ok();
        if spawned {
            std::process::exit(0);
        }
        false
    }
}

#[cfg(target_os = "macos")]
fn start_native_updater() -> bool {
    macos_updater::start()
}

#[cfg(target_os = "windows")]
fn start_native_updater() -> bool {
    windows_updater::start()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn start_native_updater() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn native_updater_enabled() -> bool {
    macos_updater::can_install_updates()
}

#[cfg(target_os = "windows")]
fn native_updater_enabled() -> bool {
    true
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn native_updater_enabled() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn check_native_updates(
    _download_url: Option<&str>,
    _digest: Option<&str>,
    _relaunch_file: Option<PathBuf>,
) -> bool {
    macos_updater::check_for_updates()
}

#[cfg(target_os = "windows")]
fn check_native_updates(
    download_url: Option<&str>,
    digest: Option<&str>,
    relaunch_file: Option<PathBuf>,
) -> bool {
    windows_updater::check_for_updates(download_url, digest, relaunch_file)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn check_native_updates(
    _download_url: Option<&str>,
    _digest: Option<&str>,
    _relaunch_file: Option<PathBuf>,
) -> bool {
    false
}

#[cfg(any(target_os = "linux", test))]
fn linux_webkit_compat_env(
    disable_dmabuf: Option<&str>,
    disable_compositing: Option<&str>,
    nvidia_driver_present: bool,
) -> Option<(&'static str, &'static str)> {
    if disable_dmabuf.is_some() || disable_compositing.is_some() || !nvidia_driver_present {
        return None;
    }

    Some(("WEBKIT_DISABLE_DMABUF_RENDERER", "1"))
}

#[cfg(target_os = "linux")]
fn apply_linux_webkit_compat_env() {
    let nvidia_driver_present = Path::new("/proc/driver/nvidia/version").exists();
    if let Some((key, value)) = linux_webkit_compat_env(
        std::env::var("WEBKIT_DISABLE_DMABUF_RENDERER")
            .ok()
            .as_deref(),
        std::env::var("WEBKIT_DISABLE_COMPOSITING_MODE")
            .ok()
            .as_deref(),
        nvidia_driver_present,
    ) {
        std::env::set_var(key, value);
    }
}

#[cfg(not(target_os = "linux"))]
fn apply_linux_webkit_compat_env() {}

fn is_supported_document(path: &Path) -> bool {
    path.extension()
        .map(|extension| {
            matches!(
                extension.to_string_lossy().to_ascii_lowercase().as_str(),
                "md" | "markdown" | "mdown" | "mkd" | "txt"
            )
        })
        .unwrap_or(false)
}

fn local_document_path_from_url(value: &str) -> Option<PathBuf> {
    let url = url::Url::parse(value).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    let path = url.to_file_path().ok()?;
    if !path.is_file() || !is_supported_document(&path) {
        return None;
    }
    fs::canonicalize(path).ok()
}

fn watch_path_for(session: &DocumentSession) -> Option<PathBuf> {
    session
        .active()
        .filter(|tab| !tab.untitled)
        .map(|tab| tab.path.clone())
}

fn install_file_watcher(
    holder: &Arc<Mutex<Option<notify::RecommendedWatcher>>>,
    proxy: &EventLoopProxy<UserEvent>,
    last_self_write: &Arc<Mutex<Option<SelfWriteRecord>>>,
    path: Option<PathBuf>,
) {
    let mut current = holder.lock().unwrap();
    *current = None;
    let Some(path) = path else {
        return;
    };
    // Watch notifications describe filesystem activity, not a new document
    // revision. Keep the loaded disk snapshot, including before the first save.
    if let Ok(content) = fs::read_to_string(&path) {
        *last_self_write.lock().unwrap() = Some(SelfWriteRecord {
            path: path.clone(),
            content,
        });
    }
    let target_path = path.clone();
    let callback_path = path.clone();
    let proxy = proxy.clone();
    let last_self_write = Arc::clone(last_self_write);
    if let Ok(mut watcher) = notify::recommended_watcher(move |result: Result<Event, _>| {
        if let Ok(event) = result {
            if event_should_reload_file(&event, &callback_path) {
                let last_self_write = last_self_write.lock().unwrap();
                if !self_write_still_matches_disk(last_self_write.as_ref(), &callback_path) {
                    let _ = proxy.send_event(UserEvent::FileChanged(callback_path.clone()));
                }
            }
        }
    }) {
        let scope = watch_scope_for_file(&target_path);
        if scope.exists() && watcher.watch(scope, RecursiveMode::NonRecursive).is_ok() {
            *current = Some(watcher);
        }
    }
}

fn self_write_still_matches_disk(record: Option<&SelfWriteRecord>, path: &Path) -> bool {
    let Some(record) = record else {
        return false;
    };
    record.path == path
        && fs::read(path)
            .map(|content| content == record.content.as_bytes())
            .unwrap_or(false)
}

fn should_protect_external_change(webview_dirty: bool, session_dirty: bool) -> bool {
    webview_dirty || session_dirty
}

#[derive(Debug, PartialEq, Eq)]
enum FinderAction {
    Create { folder: PathBuf, kind: String },
    Terminal { folder: PathBuf },
}

fn parse_finder_action(value: &str) -> Option<FinderAction> {
    let url = url::Url::parse(value).ok()?;
    if url.scheme() != "mdpreview" || url.host_str() != Some("finder") {
        return None;
    }
    let query = url.query_pairs().collect::<HashMap<_, _>>();
    let folder = PathBuf::from(query.get("path")?.as_ref());
    match query.get("action")?.as_ref() {
        "create" => Some(FinderAction::Create {
            folder,
            kind: query
                .get("kind")
                .map(|value| value.to_string())
                .unwrap_or_else(|| "md".to_string()),
        }),
        "terminal" => Some(FinderAction::Terminal { folder }),
        _ => None,
    }
}

fn create_finder_file(folder: &Path, kind: &str) -> std::io::Result<PathBuf> {
    if !folder.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Finder target folder does not exist",
        ));
    }
    let (extension, contents): (&str, &[u8]) = match kind {
        "txt" => ("txt", b""),
        "json" => ("json", b"{}\n"),
        "html" => (
            "html",
            b"<!doctype html>\n<html lang=\"zh-CN\">\n<head>\n  <meta charset=\"utf-8\">\n  <title></title>\n</head>\n<body>\n</body>\n</html>\n",
        ),
        _ => ("md", b""),
    };
    let mut path = folder.join(format!("新建.{extension}"));
    let mut index = 2;
    while path.exists() {
        path = folder.join(format!("新建 {index}.{extension}"));
        index += 1;
    }
    fs::write(&path, contents)?;
    Ok(path)
}

fn normalize_new_markdown_path(mut path: PathBuf) -> PathBuf {
    let is_markdown = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "md" | "markdown" | "mdown" | "mkd"
            )
        })
        .unwrap_or(false);
    if !is_markdown {
        path.set_extension("md");
    }
    path
}

fn open_terminal(folder: &Path) -> bool {
    std::process::Command::new("open")
        .args(["-a", "Terminal"])
        .arg(folder)
        .spawn()
        .is_ok()
}

#[cfg(target_os = "macos")]
fn register_finder_extension() {
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let Some(contents) = executable.parent().and_then(Path::parent) else {
        return;
    };
    let Some(bundle) = contents.parent() else {
        return;
    };
    let extension = bundle.join("Contents/PlugIns/MDPreviewFinderExtension.appex");
    if bundle.extension().and_then(|value| value.to_str()) != Some("app") || !extension.exists() {
        return;
    }

    let marker = config_dir().join(".finder-extension-onboarded-v1");
    let lsregister = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
    let _ = std::process::Command::new(lsregister)
        .args(["-f", "-R", "-trusted"])
        .arg(bundle)
        .status();
    let _ = std::process::Command::new("pluginkit")
        .arg("-a")
        .arg(bundle)
        .status();
    let _ = std::process::Command::new("pluginkit")
        .args(["-e", "use", "-i", "com.mdpreview.app.FinderExtension"])
        .status();

    if marker.exists() {
        return;
    }
    let active = std::process::Command::new("pluginkit")
        .args(["-m", "-A", "-p", "com.apple.FinderSync"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .map(|output| {
            output.lines().any(|line| {
                line.trim_start().starts_with('+')
                    && line.contains("com.mdpreview.app.FinderExtension")
            })
        })
        .unwrap_or(false);
    if !active {
        show_info_dialog(
            "Enable the Finder Extension",
            "Open System Settings > General > Login Items & Extensions > Finder, then enable Markdown Studio.",
        );
    }
    let _ = fs::create_dir_all(config_dir());
    let _ = fs::write(marker, "1");
}

#[cfg(not(target_os = "macos"))]
fn register_finder_extension() {}

fn persist_session(session: &DocumentSession) {
    if let Err(error) = session.save(&session_path()) {
        eprintln!("Could not save tab session: {error}");
    }
}

fn update_tabs(webview: &WebView, session: &DocumentSession) {
    let state = tabs_json(session);
    let _ = webview.evaluate_script(&format!("if(window.__setTabs)window.__setTabs({state});"));
}

fn update_window_title(window: &Window, session: &DocumentSession) {
    let title = session
        .active()
        .map(|tab| {
            let name = if tab.untitled {
                tab.path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Untitled".to_string())
            } else {
                tab.path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| tab.path.to_string_lossy().to_string())
            };
            format!(
                "{}{} — Markdown Studio",
                if tab.dirty { "• " } else { "" },
                name
            )
        })
        .unwrap_or_else(|| "Markdown Studio".to_string());
    window.set_title(&title);
}

fn render_active_document(
    webview: &WebView,
    window: &Window,
    session: &mut DocumentSession,
    recent_files: &Arc<Mutex<Vec<PathBuf>>>,
    local_images: &SharedLocalImageRegistry,
    enhance_flags: &Arc<Mutex<EnhanceFlags>>,
    loaded_enhancers: &mut EnhanceFlags,
    strings: &Strings,
) {
    let Some(active) = session.active().cloned() else {
        local_images.lock().unwrap().begin_render();
        APP_DIRTY.store(false, Ordering::SeqCst);
        let html = empty_preview_html(strings, &recent_files.lock().unwrap());
        let _ = webview.evaluate_script(&format!(
            "if(window.__setEmptyPreview)window.__setEmptyPreview('{}');",
            escape_js(&html)
        ));
        update_tabs(webview, session);
        update_window_title(window, session);
        return;
    };

    if active.untitled {
        // Untitled scratch tab: no file on disk. Load the in-memory content
        // into the editor directly (bypasses the dirty guard in __setContent).
        APP_DIRTY.store(active.dirty, Ordering::SeqCst);
        let content = active.content.clone().unwrap_or_default();
        let html = md_to_html_with_registry(&content, None, local_images);
        let flags = enhance_flags_for(&content);
        *enhance_flags.lock().unwrap() = flags;
        let _ = webview.evaluate_script(&format!(
            "if(window.__mdPreviewLoadUntitled)window.__mdPreviewLoadUntitled('{}', {}, '{}', {}, {});",
            escape_js(&content),
            active.dirty,
            escape_js(&html),
            flags.math,
            flags.mermaid
        ));
        for script in build_enhancer_bootstrap(flags, *loaded_enhancers) {
            let _ = webview.evaluate_script(&script);
        }
        loaded_enhancers.math |= flags.math;
        loaded_enhancers.mermaid |= flags.mermaid;
        update_tabs(webview, session);
        update_window_title(window, session);
        return;
    }

    match fs::read_to_string(&active.path) {
        Ok(raw) => {
            if let Some(tab) = session.get_mut(active.id) {
                tab.missing = false;
            }
            remember_recent_file(recent_files, &active.path);
            let html = md_to_html_with_registry(&raw, active.path.parent(), local_images);
            let base_href = base_href_for_file(&active.path).unwrap_or_default();
            let flags = enhance_flags_for(&raw);
            *enhance_flags.lock().unwrap() = flags;
            let render_script = format!(
                "if(window.__setContent)window.__setContent('{}', '{}', '{}', {}, {});",
                escape_js(&html),
                escape_js(&raw),
                escape_js(&base_href),
                flags.math,
                flags.mermaid
            );
            if let Err(error) = webview.evaluate_script(&render_script) {
                log_event(&format!(
                    "set content failed: script_bytes={} error={error}",
                    render_script.len()
                ));
            }
            for script in build_enhancer_bootstrap(flags, *loaded_enhancers) {
                let _ = webview.evaluate_script(&script);
            }
            loaded_enhancers.math |= flags.math;
            loaded_enhancers.mermaid |= flags.mermaid;
            if active.edit_on_open {
                if let Some(tab) = session.get_mut(active.id) {
                    tab.edit_on_open = false;
                }
                let _ = webview.evaluate_script(
                    "if(window.__mdPreviewEnterEdit)window.__mdPreviewEnterEdit();",
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            local_images.lock().unwrap().begin_render();
            if let Some(tab) = session.get_mut(active.id) {
                tab.missing = true;
            }
            if active.dirty {
                show_warning_dialog(
                    strings.missing_title,
                    "The file disappeared while it still has unsaved edits. The editor content has been kept.",
                );
            } else {
                let html = missing_preview_html(active.id, &active.path, strings);
                let _ = webview.evaluate_script(&format!(
                    "if(window.__setMissing)window.__setMissing('{}');",
                    escape_js(&html)
                ));
            }
        }
        Err(error) => {
            local_images.lock().unwrap().begin_render();
            show_warning_dialog(strings.cannot_read, &error.to_string());
        }
    }

    APP_DIRTY.store(
        session.active().map(|tab| tab.dirty).unwrap_or(false),
        Ordering::SeqCst,
    );
    update_tabs(webview, session);
    update_window_title(window, session);
}

fn main() {
    apply_linux_webkit_compat_env();

    // Bench instrumentation: MD_PREVIEW_BENCH=1 makes the app print
    // cold-start timings to stderr and exit as soon as the first paint
    // lands. Costs nothing outside bench mode.
    let bench = std::env::var("MD_PREVIEW_BENCH").is_ok();
    let t0 = Instant::now();
    let bench_log = |label: &str| {
        if bench {
            eprintln!("[bench] +{}ms {}", t0.elapsed().as_millis(), label);
        }
    };
    bench_log("main_start");

    // CLI: md-preview [--edit] [file.md ...]
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|arg| is_help_arg(arg)) {
        print_help();
        return;
    }
    if args.iter().any(|arg| is_version_arg(arg)) {
        print_version();
        return;
    }
    let edit_from_cli = args.iter().any(|arg| arg == "--edit");
    let cli_paths = args
        .into_iter()
        .filter(|arg| arg != "--edit")
        .map(PathBuf::from)
        .map(|path| {
            if path.is_relative() {
                std::env::current_dir().unwrap_or_default().join(path)
            } else {
                path
            }
        })
        .filter(|path| {
            if path.exists() && is_supported_document(path) {
                true
            } else {
                eprintln!("File not found or unsupported: {}", path.display());
                false
            }
        })
        .collect::<Vec<_>>();

    // Build the event proxy before slower registration/window setup so a
    // concurrent second launch can enqueue its files immediately.
    let event_loop: EventLoop<UserEvent> = EventLoopBuilder::with_user_event().build();
    let proxy = event_loop.create_proxy();

    #[cfg(target_os = "windows")]
    match single_instance::acquire(&config_dir(), &cli_paths, edit_from_cli) {
        single_instance::InstanceRole::Forwarded => return,
        single_instance::InstanceRole::Primary(server) => {
            let instance_proxy = proxy.clone();
            server.start(move |request| {
                let paths = request
                    .paths
                    .into_iter()
                    .map(PathBuf::from)
                    .filter(|path| path.is_file() && is_supported_document(path))
                    .filter_map(|path| fs::canonicalize(path).ok())
                    .collect::<Vec<_>>();
                instance_proxy
                    .send_event(UserEvent::OpenPaths(paths, request.edit))
                    .is_ok()
            });
        }
        single_instance::InstanceRole::Standalone => {
            log_event("single-instance ports unavailable; continuing with an independent window");
        }
    }

    let lang = detect_lang();
    let strings = Strings::for_lang(lang);
    // Isolated debug WebView tests must not replace the user's file associations.
    if !cfg!(debug_assertions) || std::env::var_os("MD_PREVIEW_TEST_SKIP_REGISTRATION").is_none() {
        register_as_default(lang);
    }
    register_finder_extension();
    bench_log("after_register");

    let mut initial_session = DocumentSession::load(&session_path());
    for path in cli_paths {
        initial_session.open(path, edit_from_cli);
    }

    let initial_theme = load_theme_choice();
    install_macos_menu(proxy.clone(), initial_theme);
    let native_updater_enabled = native_updater_enabled();

    let title = initial_session
        .active()
        .and_then(|tab| tab.path.file_name())
        .map(|name| format!("{} — Markdown Studio", name.to_string_lossy()))
        .unwrap_or_else(|| "Markdown Studio".to_string());

    let geom = load_window_geom()
        .filter(|g| geom_visible(g, &event_loop))
        .unwrap_or_else(|| centered_geom(&event_loop));

    let mut window_builder = WindowBuilder::new()
        .with_title(&title)
        .with_inner_size(LogicalSize::new(geom.w, geom.h))
        .with_position(LogicalPosition::new(geom.x, geom.y))
        .with_theme(initial_theme.tao_theme());
    if let Some(icon) = load_window_icon() {
        window_builder = window_builder.with_window_icon(Some(icon));
    }
    let window = window_builder
        .build(&event_loop)
        .expect("failed to build window");
    bench_log("window_built");
    capture_main_window_parent(&window);
    log_event("app started");
    let native_updater_available = if native_updater_enabled {
        start_native_updater()
    } else {
        false
    };
    if bench && native_updater_available {
        bench_log("native_updater_started");
    }

    let recent_files: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(load_recent_files()));
    let local_images: SharedLocalImageRegistry =
        Arc::new(Mutex::new(LocalImageRegistry::default()));

    let mut initial_flags = EnhanceFlags::default();
    let initial_page = match initial_session.active().cloned() {
        Some(tab) => match fs::read_to_string(&tab.path) {
            Ok(raw) => {
                remember_recent_file(&recent_files, &tab.path);
                let html_body = md_to_html_with_registry(&raw, tab.path.parent(), &local_images);
                let base_href = base_href_for_file(&tab.path);
                initial_flags = enhance_flags_for(&raw);
                build_page(
                    &html_body,
                    &raw,
                    base_href.as_deref(),
                    initial_flags,
                    &strings,
                    false,
                    native_updater_enabled,
                )
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Some(active) = initial_session.active_mut() {
                    active.missing = true;
                }
                build_page(
                    &missing_preview_html(tab.id, &tab.path, &strings),
                    "",
                    None,
                    EnhanceFlags::default(),
                    &strings,
                    false,
                    native_updater_enabled,
                )
            }
            Err(error) => build_page(
                &format!(
                    r#"<div class="empty"><div class="icon">#</div><div>{}: {}</div><button class="empty-open" type="button" data-open-file>{}</button></div>"#,
                    html_escape_text(strings.cannot_read),
                    html_escape_text(&error.to_string()),
                    html_escape_text(strings.open_file)
                ),
                "",
                None,
                EnhanceFlags::default(),
                &strings,
                true,
                native_updater_enabled,
            ),
        },
        None => build_page(
            &empty_preview_html(&strings, &recent_files.lock().unwrap()),
            "",
            None,
            EnhanceFlags::default(),
            &strings,
            true,
            native_updater_enabled,
        ),
    };

    persist_session(&initial_session);
    let document_session = Arc::new(Mutex::new(initial_session));
    let enhance_flags: Arc<Mutex<EnhanceFlags>> = Arc::new(Mutex::new(initial_flags));
    let last_self_write: Arc<Mutex<Option<SelfWriteRecord>>> = Arc::new(Mutex::new(None));
    let session_for_ipc = Arc::clone(&document_session);
    let recent_files_for_ipc = Arc::clone(&recent_files);
    let last_self_write_for_ipc = Arc::clone(&last_self_write);
    let proxy_for_ipc = proxy.clone();

    // Windows: steer WebView2's cache/cookie tree into %LOCALAPPDATA% instead of
    // letting it drop next to the exe. Other platforms: use default (None).
    let data_dir: Option<PathBuf> = {
        #[cfg(target_os = "windows")]
        {
            let d = config_dir().join("WebView2");
            let _ = fs::create_dir_all(&d);
            Some(d)
        }
        #[cfg(not(target_os = "windows"))]
        {
            None
        }
    };
    let mut web_context = wry::WebContext::new(data_dir);

    let proxy_for_navigation = proxy.clone();
    let local_images_for_protocol = Arc::clone(&local_images);
    let builder = WebViewBuilder::with_web_context(&mut web_context)
        .with_custom_protocol(LOCAL_IMAGE_SCHEME.into(), move |_webview_id, request| {
            local_image_protocol_response(&local_images_for_protocol, request.uri().path())
        })
        .with_custom_protocol(editor_assets::SCHEME.into(), move |_webview_id, request| {
            editor_assets::response(request.uri().path())
        })
        .with_html(&initial_page)
        .with_navigation_handler(move |url: String| {
            // Let wry load the initial in-memory document; route any real URL click
            // to the app's tab model or the system default handler.
            if url.starts_with("http://")
                || url.starts_with("https://")
                || url.starts_with("mailto:")
            {
                let _ = open::that(&url);
                false
            } else if let Some(path) = local_document_path_from_url(&url) {
                let _ = proxy_for_navigation.send_event(UserEvent::OpenPaths(vec![path], false));
                false
            } else if url.starts_with("file:") {
                false
            } else {
                true
            }
        })
        .with_ipc_handler(move |msg| {
            let body = msg.body();
            if body == "new-file" {
                let _ = proxy_for_ipc.send_event(UserEvent::NewFile);
            } else if body == "open" {
                let _ = proxy_for_ipc.send_event(UserEvent::OpenFile);
            } else if let Some(index) = body.strip_prefix("open-recent:") {
                if let Ok(index) = index.parse::<usize>() {
                    let path = recent_files_for_ipc.lock().unwrap().get(index).cloned();
                    if let Some(path) = path {
                        if path.exists() {
                            let _ =
                                proxy_for_ipc.send_event(UserEvent::OpenPaths(vec![path], false));
                        } else if forget_recent_file(&recent_files_for_ipc, &path) {
                            let _ = proxy_for_ipc.send_event(UserEvent::RecentChanged);
                        }
                    }
                }
            } else if let Some(url) = body.strip_prefix("open-local-link:") {
                if let Some(path) = local_document_path_from_url(url) {
                    let _ = proxy_for_ipc.send_event(UserEvent::OpenPaths(vec![path], false));
                }
            } else if let Some(rest) = body.strip_prefix("tab-action:") {
                let (header, pending_content) = rest
                    .split_once('\n')
                    .map(|(header, content)| (header, Some(content)))
                    .unwrap_or((rest, None));
                let mut parts = header.splitn(2, ':');
                let action = parts.next().unwrap_or("");
                let id = parts.next().and_then(|value| value.parse::<u64>().ok());
                let Some(id) = id else {
                    return;
                };
                if let Some(content) = pending_content {
                    let active = session_for_ipc.lock().unwrap().active().cloned();
                    let Some(active) = active else {
                        return;
                    };
                    if active.untitled {
                        // Untitled scratch tab: keep the content in memory
                        // instead of writing to disk. The Rust-side tab stays
                        // dirty (close/quit still prompt); only the webview's
                        // editor dirty flag is cleared so tab switches render.
                        let mut session = session_for_ipc.lock().unwrap();
                        if let Some(tab) = session.get_mut(active.id) {
                            tab.content = Some(content.to_string());
                            tab.dirty = true;
                        }
                        drop(session);
                        let _ = proxy_for_ipc.send_event(UserEvent::DirtyChanged(true));
                        let _ = proxy_for_ipc.send_event(UserEvent::EditorFlushed);
                    } else {
                        let path = active.path.clone();
                        *last_self_write_for_ipc.lock().unwrap() = Some(SelfWriteRecord {
                            path: path.clone(),
                            content: content.to_string(),
                        });
                        match fs::write(&path, content) {
                            Ok(()) => {
                                let _ = proxy_for_ipc.send_event(UserEvent::FileSaved(path));
                            }
                            Err(error) => {
                                let _ = proxy_for_ipc.send_event(UserEvent::SaveFailed {
                                    error: format!("{}: {error}", path.display()),
                                    content: content.to_string(),
                                    close_tab: (action == "close").then_some(id),
                                });
                                return;
                            }
                        }
                    }
                }
                match action {
                    "activate" => {
                        let _ = proxy_for_ipc.send_event(UserEvent::ActivateTab(id));
                    }
                    "close" => {
                        let _ = proxy_for_ipc.send_event(UserEvent::CloseTab(id));
                    }
                    _ => {}
                }
            } else if let Some(id) = body.strip_prefix("locate-tab:") {
                if let Ok(id) = id.parse::<u64>() {
                    let _ = proxy_for_ipc.send_event(UserEvent::LocateTab(id));
                }
            } else if let Some(payload) = body.strip_prefix("render-source:") {
                if let Some((token, content)) = payload.split_once('\n') {
                    if let Ok(token) = token.parse::<u64>() {
                        let _ = proxy_for_ipc
                            .send_event(UserEvent::SourcePreview(token, content.to_string()));
                    }
                }
            } else if let Some(token) = body.strip_prefix("prepare-rich-edit:") {
                if let Ok(token) = token.parse::<u64>() {
                    let active = session_for_ipc.lock().unwrap().active().cloned();
                    let result = match active {
                        Some(tab) if !tab.untitled => {
                            editor_assets::backup_document(&tab.path, &config_dir().join("backups"))
                                .map(Some)
                                .map_err(|error| error.to_string())
                        }
                        Some(_) => Ok(None),
                        None => Err("No active document".to_string()),
                    };
                    let _ = proxy_for_ipc.send_event(UserEvent::RichEditPrepared(token, result));
                }
            } else if body == "dirty:1" {
                let _ = proxy_for_ipc.send_event(UserEvent::DirtyChanged(true));
            } else if body == "dirty:0" {
                let _ = proxy_for_ipc.send_event(UserEvent::DirtyChanged(false));
            } else if body == "quit-ack" {
                let _ = proxy_for_ipc.send_event(UserEvent::QuitAck);
            } else if body == "external-change:dirty" {
                let _ = proxy_for_ipc.send_event(UserEvent::ExternalChangeResolved(true));
            } else if body == "external-change:clean" {
                let _ = proxy_for_ipc.send_event(UserEvent::ExternalChangeResolved(false));
            } else if body == "print" {
                let _ = proxy_for_ipc.send_event(UserEvent::Print);
            } else if body == "ready" {
                let _ = proxy_for_ipc.send_event(UserEvent::Ready);
            } else if body == "refresh" {
                let active = session_for_ipc.lock().unwrap().active().cloned();
                if let Some(active) = active {
                    if !active.untitled {
                        let _ = proxy_for_ipc.send_event(UserEvent::FileChanged(active.path));
                    }
                }
            } else if let Some(url) = body.strip_prefix("open-url:") {
                if is_allowed_update_url(url) {
                    let _ = open::that(url);
                }
            } else if body == "check-updates" || body.starts_with("check-updates:\n") {
                let payload = body.strip_prefix("check-updates:\n").unwrap_or("");
                let mut parts = payload.splitn(4, '\n');
                let download_url = parts.next().filter(|value| !value.is_empty());
                let digest = parts.next().filter(|value| !value.is_empty());
                let tag = parts.next().filter(|value| !value.is_empty());
                let relaunch_file = session_for_ipc
                    .lock()
                    .unwrap()
                    .active()
                    .map(|tab| tab.path.clone());
                if !check_native_updates(download_url, digest, relaunch_file) {
                    if let Some(url) = download_url.filter(|url| is_allowed_update_url(url)) {
                        if confirm_open_update(tag.unwrap_or("update")) {
                            let _ = open::that(url);
                        }
                    } else {
                        show_warning_dialog(
                            "Update Unavailable",
                            "Markdown Studio could not start the updater for this release.",
                        );
                    }
                }
            } else if let Some(result) = body.strip_prefix("update-check-result:") {
                let mut parts = result.splitn(4, '\n');
                match parts.next().unwrap_or("") {
                    "available" => {
                        let tag = parts.next().unwrap_or("update").to_string();
                        let url = parts.next().unwrap_or("").to_string();
                        let digest = parts
                            .next()
                            .filter(|value| !value.is_empty())
                            .map(str::to_string);
                        let _ = proxy_for_ipc.send_event(UserEvent::UpdateCheckResult(
                            UpdateCheckResult::Available { tag, url, digest },
                        ));
                    }
                    "none" => {
                        let _ = proxy_for_ipc
                            .send_event(UserEvent::UpdateCheckResult(UpdateCheckResult::UpToDate));
                    }
                    _ => {
                        let _ = proxy_for_ipc
                            .send_event(UserEvent::UpdateCheckResult(UpdateCheckResult::Failed));
                    }
                }
            } else if let Some(content) = body.strip_prefix("save:") {
                let active = session_for_ipc.lock().unwrap().active().cloned();
                if let Some(active) = active {
                    if active.untitled {
                        // No path yet: explicit save on an untitled tab is Save As.
                        let _ = proxy_for_ipc.send_event(UserEvent::SaveAs(content.to_string()));
                    } else {
                        let path = active.path.clone();
                        *last_self_write_for_ipc.lock().unwrap() = Some(SelfWriteRecord {
                            path: path.clone(),
                            content: content.to_string(),
                        });
                        match fs::write(&path, content) {
                            Ok(()) => {
                                let _ = proxy_for_ipc.send_event(UserEvent::FileSaved(path));
                            }
                            Err(error) => {
                                let _ = proxy_for_ipc.send_event(UserEvent::SaveFailed {
                                    error: format!("{}: {error}", path.display()),
                                    content: content.to_string(),
                                    close_tab: None,
                                });
                            }
                        }
                    }
                }
            } else if let Some(content) = body.strip_prefix("store-content\n") {
                // Untitled tab autosave: stash the content in memory.
                let mut session = session_for_ipc.lock().unwrap();
                if let Some(tab) = session.active_mut() {
                    if tab.untitled {
                        let tab_id = tab.id;
                        let content = content.to_string();
                        tab.content = Some(content.clone());
                        tab.dirty = true;
                        drop(session);
                        let _ = proxy_for_ipc.send_event(UserEvent::DirtyChanged(true));
                        let _ =
                            proxy_for_ipc.send_event(UserEvent::UntitledPreview(tab_id, content));
                    }
                }
            } else if let Some(content) = body.strip_prefix("save-as\n") {
                let _ = proxy_for_ipc.send_event(UserEvent::SaveAs(content.to_string()));
            } else if let Some(payload) = body.strip_prefix("export-docx\n") {
                let mut parts = payload.splitn(2, '\n');
                let flags = parts.next().unwrap_or("00");
                let content = parts.next().unwrap_or("");
                let _ = proxy_for_ipc.send_event(UserEvent::ExportDocx {
                    standardize: flags.as_bytes().first() == Some(&b'1'),
                    strip_numbers: flags.as_bytes().get(1) == Some(&b'1'),
                    content: content.to_string(),
                });
            } else if let Some(html) = body.strip_prefix("export-pdf\n") {
                let _ = proxy_for_ipc.send_event(UserEvent::ExportPdf(html.to_string()));
            } else if let Some(svg) = body.strip_prefix("download-svg\n") {
                let _ = proxy_for_ipc.send_event(UserEvent::DownloadSvg(svg.to_string()));
            } else if let Some(png) = body.strip_prefix("download-png\n") {
                let _ = proxy_for_ipc.send_event(UserEvent::DownloadPng(png.to_string()));
            } else if let Some(id) = body.strip_prefix("reveal-tab:") {
                if let Ok(id) = id.parse::<u64>() {
                    let _ = proxy_for_ipc.send_event(UserEvent::RevealTab(id));
                }
            }
        })
        .with_drag_drop_handler({
            let proxy = proxy.clone();
            move |event| {
                if let wry::DragDropEvent::Drop { paths, .. } = event {
                    let paths = paths
                        .into_iter()
                        .filter(|path| is_supported_document(path))
                        .collect::<Vec<_>>();
                    if !paths.is_empty() {
                        let _ = proxy.send_event(UserEvent::OpenPaths(paths, false));
                    }
                }
                true
            }
        });

    #[cfg(target_os = "linux")]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;

        let vbox = window
            .default_vbox()
            .expect("failed to get default GTK container");
        builder.build_gtk(vbox).expect("failed to build webview")
    };
    #[cfg(target_os = "windows")]
    let webview = match builder.build(&window) {
        Ok(webview) => webview,
        Err(error) => show_windows_webview_startup_failure(&error),
    };
    #[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
    let webview = builder.build(&window).expect("failed to build webview");
    bench_log("webview_built");
    let session_for_event = Arc::clone(&document_session);
    update_tabs(&webview, &session_for_event.lock().unwrap());
    if session_for_event
        .lock()
        .unwrap()
        .active()
        .map(|tab| tab.edit_on_open && !tab.missing)
        .unwrap_or(false)
    {
        let _ = webview
            .evaluate_script("if(window.__mdPreviewEnterEdit)window.__mdPreviewEnterEdit();");
        if let Some(tab) = session_for_event.lock().unwrap().active_mut() {
            tab.edit_on_open = false;
        }
    }

    // hljs + extra language packs aren't part of first-paint HTML anymore.
    // We push them in via evaluate_script the moment the webview tells us
    // it's painted (IPC 'ready'). Keeps ~125KB out of the HTML-parse critical
    // path so the app window shows content faster on cold start.
    let hljs_bootstrap = format!(
        "(function(){{{hljs_js};{hljs_extra};try{{window.hljs=hljs;}}catch(e){{}}if(typeof hljs!=='undefined'&&hljs.highlightAll){{hljs.highlightAll();}}}})();",
        hljs_js = HLJS_JS,
        hljs_extra = HLJS_EXTRA_LANGS,
    );

    // File watcher state
    let watcher_holder: Arc<Mutex<Option<notify::RecommendedWatcher>>> = Arc::new(Mutex::new(None));
    let watcher_for_event = Arc::clone(&watcher_holder);
    let initial_watch_path = session_for_event
        .lock()
        .unwrap()
        .active()
        .map(|tab| tab.path.clone());
    install_file_watcher(
        &watcher_holder,
        &proxy,
        &last_self_write,
        initial_watch_path,
    );

    let mut loaded_enhancers = EnhanceFlags::default();
    let mut pending_window_close = false;
    let mut pending_tab_close_after_save: Option<u64> = None;
    let mut warned_external_change: Option<PathBuf> = None;
    let mut pending_external_change: Option<PathBuf> = None;
    let mut new_file_bounced = false;
    let mut open_file_bounced = false;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            TaoEvent::UserEvent(UserEvent::NewFile) => {
                // Dirty editor content must be flushed (to disk for files, to
                // memory for untitled tabs) before the shared textarea is
                // repurposed for the new tab. Bounce through JS once; if the
                // tab is still dirty afterwards (save failed/cancelled), abort.
                let (dirty, untitled) = session_for_event
                    .lock()
                    .unwrap()
                    .active()
                    .map(|tab| (tab.dirty, tab.untitled))
                    .unwrap_or((false, false));
                if dirty {
                    if !new_file_bounced {
                        new_file_bounced = true;
                        let _ = webview.evaluate_script(
                            "if(window.__mdPreviewNewFile)window.__mdPreviewNewFile();",
                        );
                        return;
                    }
                    new_file_bounced = false;
                    if !untitled {
                        return; // save failed or cancelled: keep editing
                    }
                    // dirty untitled: content was stored in memory by the bounce
                }
                let scratch_dir = config_dir().join("scratch");
                let mut session = session_for_event.lock().unwrap();
                session.open_untitled(&scratch_dir, strings.untitled_name);
                persist_session(&session);
                render_active_document(
                    &webview,
                    &window,
                    &mut session,
                    &recent_files,
                    &local_images,
                    &enhance_flags,
                    &mut loaded_enhancers,
                    &strings,
                );
                drop(session);
                install_file_watcher(&watcher_for_event, &proxy, &last_self_write, None);
            }
            TaoEvent::UserEvent(UserEvent::OpenFile) => {
                let (dirty, untitled) = session_for_event
                    .lock()
                    .unwrap()
                    .active()
                    .map(|tab| (tab.dirty, tab.untitled))
                    .unwrap_or((false, false));
                if dirty {
                    if !open_file_bounced {
                        open_file_bounced = true;
                        let _ = webview.evaluate_script(
                            "if(window.__mdPreviewOpenFile)window.__mdPreviewOpenFile();",
                        );
                        return;
                    }
                    open_file_bounced = false;
                    if !untitled {
                        return; // save failed or cancelled: keep editing
                    }
                }
                if let Some(paths) = parented(rfd::FileDialog::new())
                    .add_filter("Markdown", &["md", "markdown", "mdown", "mkd", "txt"])
                    .pick_files()
                {
                    let _ = proxy.send_event(UserEvent::OpenPaths(paths, false));
                }
            }
            TaoEvent::UserEvent(UserEvent::OpenPaths(paths, edit_on_open)) => {
                // File-association launches arrive from a short-lived second
                // process. Reuse this window, restore it, and focus it even
                // when the request contains no file (for example, launching
                // the app again from Start).
                window.set_visible(true);
                window.set_minimized(false);
                window.set_focus();
                if paths.is_empty() {
                    return;
                }
                let mut session = session_for_event.lock().unwrap();
                let previous_active = session.active_id;
                let preserve_active = session.active().map(|tab| tab.dirty).unwrap_or(false);
                for path in paths.into_iter().filter(|path| is_supported_document(path)) {
                    session.open(path, edit_on_open);
                }
                if preserve_active {
                    if let Some(id) = previous_active {
                        session.activate(id);
                    }
                }
                persist_session(&session);
                if preserve_active {
                    update_tabs(&webview, &session);
                } else {
                    render_active_document(
                        &webview,
                        &window,
                        &mut session,
                        &recent_files,
                        &local_images,
                        &enhance_flags,
                        &mut loaded_enhancers,
                        &strings,
                    );
                }
                let path = watch_path_for(&session);
                drop(session);
                install_file_watcher(&watcher_for_event, &proxy, &last_self_write, path);
            }
            TaoEvent::UserEvent(UserEvent::ActivateTab(id)) => {
                let mut session = session_for_event.lock().unwrap();
                if session.activate(id) {
                    persist_session(&session);
                    render_active_document(
                        &webview,
                        &window,
                        &mut session,
                        &recent_files,
                        &local_images,
                        &enhance_flags,
                        &mut loaded_enhancers,
                        &strings,
                    );
                    let path = watch_path_for(&session);
                    drop(session);
                    install_file_watcher(&watcher_for_event, &proxy, &last_self_write, path);
                }
            }
            TaoEvent::UserEvent(UserEvent::CloseTab(id)) => {
                let mut session = session_for_event.lock().unwrap();
                // A dirty untitled tab holds its only copy in memory: ask
                // whether to save it to a file before closing.
                let unsaved_untitled = session
                    .tabs
                    .iter()
                    .find(|tab| tab.id == id)
                    .map(|tab| tab.untitled && tab.dirty)
                    .unwrap_or(false);
                if unsaved_untitled {
                    use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
                    let choice = parented(MessageDialog::new())
                        .set_level(MessageLevel::Warning)
                        .set_title(strings.unsaved_title)
                        .set_description(strings.unsaved_body)
                        .set_buttons(MessageButtons::YesNoCancel)
                        .show();
                    match choice {
                        MessageDialogResult::Yes => {
                            let content = session
                                .tabs
                                .iter()
                                .find(|tab| tab.id == id)
                                .and_then(|tab| tab.content.clone())
                                .unwrap_or_default();
                            let dialog = parented(rfd::FileDialog::new())
                                .add_filter("Markdown", &["md", "markdown", "mdown", "mkd"])
                                .set_file_name(strings.new_filename);
                            match dialog.save_file() {
                                Some(path) => {
                                    let path = normalize_new_markdown_path(path);
                                    if let Err(error) = fs::write(&path, &content) {
                                        drop(session);
                                        show_warning_dialog("Could Not Save", &error.to_string());
                                        return;
                                    }
                                }
                                None => return, // cancelled: keep the tab
                            }
                        }
                        MessageDialogResult::No => {
                            if let Some(tab) = session.get_mut(id) {
                                tab.content = None;
                                tab.dirty = false;
                            }
                        }
                        _ => return, // cancel: keep the tab
                    }
                }
                let was_active = session.active_id == Some(id);
                if session.close(id) {
                    persist_session(&session);
                    if was_active {
                        render_active_document(
                            &webview,
                            &window,
                            &mut session,
                            &recent_files,
                            &local_images,
                            &enhance_flags,
                            &mut loaded_enhancers,
                            &strings,
                        );
                        let path = watch_path_for(&session);
                        drop(session);
                        install_file_watcher(&watcher_for_event, &proxy, &last_self_write, path);
                    } else {
                        update_tabs(&webview, &session);
                    }
                }
            }
            TaoEvent::UserEvent(UserEvent::CloseActiveTab) => {
                if session_for_event.lock().unwrap().active_id.is_some() {
                    let _ = webview.evaluate_script(
                        "if(window.__mdPreviewCloseActiveTab)window.__mdPreviewCloseActiveTab();",
                    );
                } else {
                    clean_exit_now(&window, &session_for_event.lock().unwrap(), "last tab closed: clean exit");
                }
            }
            TaoEvent::UserEvent(UserEvent::LocateTab(id)) => {
                if let Some(path) = parented(rfd::FileDialog::new())
                    .add_filter("Markdown", &["md", "markdown", "mdown", "mkd", "txt"])
                    .pick_file()
                {
                    let mut session = session_for_event.lock().unwrap();
                    if session.relocate(id, path) && session.activate(id) {
                        persist_session(&session);
                        render_active_document(
                            &webview,
                            &window,
                            &mut session,
                            &recent_files,
                            &local_images,
                            &enhance_flags,
                            &mut loaded_enhancers,
                            &strings,
                        );
                        let path = watch_path_for(&session);
                        drop(session);
                        install_file_watcher(&watcher_for_event, &proxy, &last_self_write, path);
                    } else {
                        show_warning_dialog("Already Open", "That file is already open in another tab.");
                    }
                }
            }
            TaoEvent::UserEvent(UserEvent::FileChanged(path)) => {
                if self_write_still_matches_disk(last_self_write.lock().unwrap().as_ref(), &path) {
                    return;
                }
                if session_for_event
                    .lock()
                    .unwrap()
                    .active()
                    .map(|tab| tab.path.as_path())
                    == Some(path.as_path())
                {
                    pending_external_change = Some(path);
                    let _ = webview.evaluate_script(
                        "if(window.__mdPreviewResolveExternalChange)window.__mdPreviewResolveExternalChange();",
                    );
                }
            }
            TaoEvent::UserEvent(UserEvent::ExternalChangeResolved(webview_dirty)) => {
                let Some(path) = pending_external_change.take() else {
                    return;
                };
                // A queued notification may predate a completed autosave.
                if self_write_still_matches_disk(last_self_write.lock().unwrap().as_ref(), &path) {
                    return;
                }
                let mut session = session_for_event.lock().unwrap();
                if session.active().map(|tab| tab.path.as_path()) == Some(path.as_path()) {
                    let session_dirty = session.active().map(|tab| tab.dirty).unwrap_or(false);
                    if should_protect_external_change(webview_dirty, session_dirty) {
                        if let Some(tab) = session.active_mut() {
                            tab.dirty = true;
                        }
                        APP_DIRTY.store(true, Ordering::SeqCst);
                        let _ = webview.evaluate_script(
                            "if(window.__mdPreviewPauseAutosave)window.__mdPreviewPauseAutosave();",
                        );
                        if warned_external_change.as_ref() != Some(&path) {
                            show_warning_dialog(
                                if strings.preview_edit_zh { "文件已被其他程序修改" } else { "File Changed on Disk" },
                                if strings.preview_edit_zh { "检测到磁盘内容与本程序最后读取或保存的版本不同。自动保存已暂停，您的编辑仍保留。按 Ctrl+S 使用当前编辑覆盖磁盘版本；要保留外部版本，请先另存当前编辑，再重新打开原文件。" } else { "Automatic saving is paused and your edits are still in the editor. Press Cmd/Ctrl+S to replace the disk version, or save your edits elsewhere and reopen the file to keep the external version." },
                            );
                            warned_external_change = Some(path);
                        }
                        return;
                    }
                    if let Ok(content) = fs::read_to_string(&path) {
                        *last_self_write.lock().unwrap() = Some(SelfWriteRecord { path: path.clone(), content });
                    }
                    render_active_document(
                        &webview,
                        &window,
                        &mut session,
                        &recent_files,
                        &local_images,
                        &enhance_flags,
                        &mut loaded_enhancers,
                        &strings,
                    );
                    persist_session(&session);
                }
            }
            TaoEvent::UserEvent(UserEvent::SourcePreview(token, content)) => {
                let path = session_for_event.lock().unwrap().active().map(|tab| tab.path.clone());
                let html = md_to_html_with_registry(&content, path.as_deref().and_then(Path::parent), &local_images);
                let flags = enhance_flags_for(&content);
                *enhance_flags.lock().unwrap() = flags;
                let _ = webview.evaluate_script(&format!(
                    "if(window.__mdPreviewSourceRendered)window.__mdPreviewSourceRendered({token}, '{}', '{}', {}, {});",
                    escape_js(&html), escape_js(&content), flags.math, flags.mermaid
                ));
                for script in build_enhancer_bootstrap(flags, loaded_enhancers) {
                    let _ = webview.evaluate_script(&script);
                }
                loaded_enhancers.math |= flags.math;
                loaded_enhancers.mermaid |= flags.mermaid;
            }
            TaoEvent::UserEvent(UserEvent::RichEditPrepared(token, result)) => {
                let (ok, detail) = match result {
                    Ok(path) => (true, path.map(|path| path.display().to_string()).unwrap_or_default()),
                    Err(error) => (false, error),
                };
                let _ = webview.evaluate_script(&format!(
                    "if(window.__mdPreviewRichEditPrepared)window.__mdPreviewRichEditPrepared({token}, {ok}, '{}');",
                    escape_js(&detail)
                ));
            }
            TaoEvent::UserEvent(UserEvent::FileSaved(path)) => {
                if warned_external_change.as_ref() == Some(&path) {
                    warned_external_change = None;
                }
                let mut session = session_for_event.lock().unwrap();
                let active_matches = session.active().map(|tab| tab.path.as_path()) == Some(path.as_path());
                if let Some(tab) = session.tabs.iter_mut().find(|tab| tab.path == path) {
                    tab.dirty = false;
                    tab.missing = false;
                }
                APP_DIRTY.store(false, Ordering::SeqCst);
                if active_matches {
                    if let Ok(raw) = fs::read_to_string(&path) {
                        let html =
                            md_to_html_with_registry(&raw, path.parent(), &local_images);
                        let flags = enhance_flags_for(&raw);
                        *enhance_flags.lock().unwrap() = flags;
                        let render_script = format!(
                            "if(window.__setPreview)window.__setPreview('{}', {}, {}, '{}');if(window.__markSaved)window.__markSaved('{}');",
                            escape_js(&html),
                            flags.math,
                            flags.mermaid,
                            escape_js(&raw),
                            escape_js(&raw)
                        );
                        if let Err(error) = webview.evaluate_script(&render_script) {
                            log_event(&format!(
                                "set saved preview failed: script_bytes={} error={error}",
                                render_script.len()
                            ));
                        }
                        for script in build_enhancer_bootstrap(flags, loaded_enhancers) {
                            let _ = webview.evaluate_script(&script);
                        }
                        loaded_enhancers.math |= flags.math;
                        loaded_enhancers.mermaid |= flags.mermaid;
                    }
                }
                persist_session(&session);
                update_tabs(&webview, &session);
                update_window_title(&window, &session);
                if pending_window_close {
                    clean_exit_now(&window, &session, "save completed during pending close: exiting");
                }
                if let Some(id) = pending_tab_close_after_save.take() {
                    drop(session);
                    let _ = proxy.send_event(UserEvent::CloseTab(id));
                }
            }
            TaoEvent::UserEvent(UserEvent::SaveFailed {
                error,
                content,
                close_tab,
            }) => {
                log_event(&format!("save failed: {error}"));
                let _ = webview.evaluate_script(
                    "if(window.__mdPreviewPauseAutosave)window.__mdPreviewPauseAutosave();",
                );
                if pending_window_close {
                    match ask_after_save_failure(&strings, &error, true) {
                        SaveFailureChoice::SaveAs => {
                            let _ = proxy.send_event(UserEvent::SaveAs(content));
                        }
                        SaveFailureChoice::Discard => {
                            clean_exit_now(
                                &window,
                                &session_for_event.lock().unwrap(),
                                "save failed during pending close: user discarded edits",
                            );
                        }
                        SaveFailureChoice::Cancel => pending_window_close = false,
                    }
                } else if let Some(id) = close_tab {
                    match ask_after_save_failure(&strings, &error, false) {
                        SaveFailureChoice::SaveAs => {
                            pending_tab_close_after_save = Some(id);
                            let _ = proxy.send_event(UserEvent::SaveAs(content));
                        }
                        SaveFailureChoice::Discard => {
                            let mut session = session_for_event.lock().unwrap();
                            if let Some(tab) = session.get_mut(id) {
                                tab.dirty = false;
                            }
                            drop(session);
                            let _ = proxy.send_event(UserEvent::CloseTab(id));
                        }
                        SaveFailureChoice::Cancel => {}
                    }
                } else {
                    show_warning_dialog(strings.save_failed_title, &format!("{}\n\n{error}", strings.save_failed_body));
                }
            }
            TaoEvent::UserEvent(UserEvent::DirtyChanged(dirty)) => {
                APP_DIRTY.store(dirty, Ordering::SeqCst);
                let mut session = session_for_event.lock().unwrap();
                if let Some(tab) = session.active_mut() {
                    tab.dirty = dirty;
                }
                update_tabs(&webview, &session);
                update_window_title(&window, &session);
            }
            TaoEvent::UserEvent(UserEvent::ToggleEdit) => {
                let _ = webview.evaluate_script(
                    "if(window.__mdPreviewToggleEdit)window.__mdPreviewToggleEdit();",
                );
            }
            TaoEvent::UserEvent(UserEvent::ShowFind) => {
                let _ = webview
                    .evaluate_script("if(window.__mdPreviewShowFind)window.__mdPreviewShowFind();");
            }
            TaoEvent::UserEvent(UserEvent::RecentChanged) => {
                let html = empty_preview_html(&strings, &recent_files.lock().unwrap());
                let js = format!(
                    "if(window.__setEmptyPreview)window.__setEmptyPreview('{}');",
                    escape_js(&html)
                );
                let _ = webview.evaluate_script(&js);
            }
            TaoEvent::UserEvent(UserEvent::Print) => {
                let _ = webview.print();
            }
            TaoEvent::UserEvent(UserEvent::CheckUpdates) => {
                let proxy = proxy.clone();
                std::thread::spawn(move || {
                    let _ = proxy.send_event(UserEvent::UpdateCheckResult(check_github_updates()));
                });
            }
            TaoEvent::UserEvent(UserEvent::UpdateCheckResult(result)) => match result {
                UpdateCheckResult::Available { tag, url, digest } => {
                    if is_allowed_update_url(&url) && confirm_open_update(&tag) {
                        let relaunch_file = session_for_event
                            .lock()
                            .unwrap()
                            .active()
                            .map(|tab| tab.path.clone());
                        if !check_native_updates(
                            Some(url.as_str()),
                            digest.as_deref(),
                            relaunch_file,
                        ) {
                            let _ = open::that(url);
                        }
                    }
                }
                UpdateCheckResult::UpToDate => {
                    show_info_dialog(
                        "Markdown Studio Is Up to Date",
                        &format!(
                            "You are using the latest version: {}.",
                            env!("CARGO_PKG_VERSION")
                        ),
                    );
                }
                UpdateCheckResult::Failed => {
                    show_warning_dialog(
                        "Could Not Check for Updates",
                        "Markdown Studio could not reach the update service. Please try again later.",
                    );
                }
            },
            TaoEvent::UserEvent(UserEvent::SaveAs(content)) => {
                let Some(active) = session_for_event.lock().unwrap().active().cloned() else {
                    return;
                };
                let default_name = if active.untitled {
                    strings.new_filename.to_string()
                } else {
                    active
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| strings.new_filename.to_string())
                };
                let mut dialog = parented(rfd::FileDialog::new())
                    .add_filter("Markdown", &["md", "markdown", "mdown", "mkd"])
                    .set_file_name(&default_name);
                if !active.untitled {
                    if let Some(dir) = active.path.parent() {
                        dialog = dialog.set_directory(dir);
                    }
                }
                log_event(&format!("save-as dialog shown (pending close: {pending_window_close})"));
                let Some(path) = dialog.save_file() else {
                    // Cancelled: stay dirty, and do not let a pending quit proceed.
                    log_event("save-as dialog cancelled: pending close aborted");
                    pending_window_close = false;
                    pending_tab_close_after_save = None;
                    return;
                };
                let path = normalize_new_markdown_path(path);
                let conflict = session_for_event
                    .lock()
                    .unwrap()
                    .tabs
                    .iter()
                    .any(|tab| tab.id != active.id && tab.path == path);
                if conflict {
                    pending_window_close = false;
                    pending_tab_close_after_save = None;
                    show_warning_dialog(
                        "Already Open",
                        "That file is already open in another tab.",
                    );
                    return;
                }
                match fs::write(&path, &content) {
                    Ok(()) => {
                        *last_self_write.lock().unwrap() = Some(SelfWriteRecord {
                            path: path.clone(),
                            content: content.clone(),
                        });
                        let mut session = session_for_event.lock().unwrap();
                        if session.relocate(active.id, path.clone()) {
                            if let Some(tab) = session.get_mut(active.id) {
                                tab.content = None;
                            }
                            drop(session);
                            install_file_watcher(
                                &watcher_for_event,
                                &proxy,
                                &last_self_write,
                                Some(path.clone()),
                            );
                            let _ = proxy.send_event(UserEvent::FileSaved(path));
                        } else {
                            drop(session);
                            pending_window_close = false;
                            pending_tab_close_after_save = None;
                            show_warning_dialog(
                                "Already Open",
                                "That file is already open in another tab.",
                            );
                        }
                    }
                    Err(error) => {
                        pending_window_close = false;
                        pending_tab_close_after_save = None;
                        show_warning_dialog("Could Not Save", &error.to_string());
                    }
                }
            }
            TaoEvent::UserEvent(UserEvent::UntitledPreview(tab_id, content)) => {
                let is_active_untitled = session_for_event
                    .lock()
                    .unwrap()
                    .active()
                    .is_some_and(|tab| tab.id == tab_id && tab.untitled);
                if !is_active_untitled {
                    return;
                }
                let html = md_to_html_with_registry(&content, None, &local_images);
                let flags = enhance_flags_for(&content);
                *enhance_flags.lock().unwrap() = flags;
                let _ = webview.evaluate_script(&format!(
                    "if(window.__setPreview)window.__setPreview('{}', {}, {}, '{}');",
                    escape_js(&html),
                    flags.math,
                    flags.mermaid,
                    escape_js(&content)
                ));
                for script in build_enhancer_bootstrap(flags, loaded_enhancers) {
                    let _ = webview.evaluate_script(&script);
                }
                loaded_enhancers.math |= flags.math;
                loaded_enhancers.mermaid |= flags.mermaid;
            }
            TaoEvent::UserEvent(UserEvent::EditorFlushed) => {
                let _ = webview.evaluate_script(
                    "if(window.__mdPreviewSetDirtyQuiet)window.__mdPreviewSetDirtyQuiet(false);",
                );
            }
            TaoEvent::UserEvent(UserEvent::ExportDocx {
                standardize,
                strip_numbers,
                content,
            }) => {
                let active = session_for_event.lock().unwrap().active().cloned();
                let (stem, base_dir) = match &active {
                    Some(tab) if !tab.untitled => (
                        tab.path
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| strings.untitled_name.to_string()),
                        tab.path.parent().map(Path::to_path_buf),
                    ),
                    _ => (
                        strings.untitled_name.to_string(),
                        std::env::current_dir().ok(),
                    ),
                };
                let test_path = std::env::var_os("MD_PREVIEW_TEST_EXPORT").map(PathBuf::from);
                let Some(path) = (match &test_path {
                    Some(p) => Some(p.clone()),
                    None => parented(rfd::FileDialog::new())
                        .add_filter("Word", &["docx"])
                        .set_file_name(&format!("{stem}.docx"))
                        .save_file(),
                }) else {
                    return;
                };
                let mut path = path;
                if path
                    .extension()
                    .map(|ext| !ext.to_string_lossy().eq_ignore_ascii_case("docx"))
                    .unwrap_or(true)
                {
                    path.set_extension("docx");
                }
                let template = load_export_template();
                let options = docx::ExportOptions {
                    standardize,
                    strip_heading_numbers: strip_numbers,
                    template,
                };
                match docx::export_docx(&content, base_dir.as_deref(), &options) {
                    Ok(bytes) => match fs::write(&path, bytes) {
                        Ok(()) => {
                            log_event(&format!("docx exported to {}", path.display()));
                            if test_path.is_none() {
                                show_info_dialog(
                                    strings.export_done_title,
                                    &format!("{}{}", strings.export_done_body, path.display()),
                                );
                            }
                        }
                        Err(error) => {
                            show_warning_dialog("Could Not Save", &error.to_string())
                        }
                    },
                    Err(error) => show_warning_dialog("Export Failed", &error),
                }
            }
            TaoEvent::UserEvent(UserEvent::ExportPdf(html)) => {
                let active = session_for_event.lock().unwrap().active().cloned();
                let stem = match &active {
                    Some(tab) if !tab.untitled => tab
                        .path
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| strings.untitled_name.to_string()),
                    _ => strings.untitled_name.to_string(),
                };
                let test_path = std::env::var_os("MD_PREVIEW_TEST_EXPORT_PDF").map(PathBuf::from);
                let Some(path) = (match &test_path {
                    Some(p) => Some(p.clone()),
                    None => parented(rfd::FileDialog::new())
                        .add_filter("PDF", &["pdf"])
                        .set_file_name(&format!("{stem}.pdf"))
                        .save_file(),
                }) else {
                    return;
                };
                let mut path = path;
                if path
                    .extension()
                    .map(|ext| !ext.to_string_lossy().eq_ignore_ascii_case("pdf"))
                    .unwrap_or(true)
                {
                    path.set_extension("pdf");
                }
                #[cfg(target_os = "windows")]
                {
                    log_event("pdf export started");
                    match find_pdf_browser() {
                        Some(browser) => match export_pdf_via_browser(&browser, &html, &path) {
                            Ok(()) => {
                                log_event(&format!("pdf exported to {}", path.display()));
                                if test_path.is_none() {
                                    show_info_dialog(
                                        strings.export_done_title,
                                        &format!("{}{}", strings.pdf_done_body, path.display()),
                                    );
                                }
                            }
                            Err(error) => {
                                log_event(&format!("pdf export failed: {error}"));
                                show_warning_dialog("Export Failed", &error);
                            }
                        },
                        None => {
                            log_event("pdf export failed: no chromium browser found");
                            show_warning_dialog(strings.pdf_export, strings.pdf_no_browser);
                        }
                    }
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = (html, path, test_path);
                    show_warning_dialog(strings.pdf_export, strings.pdf_no_browser);
                }
            }
            TaoEvent::UserEvent(UserEvent::RevealTab(id)) => {
                let tab = session_for_event
                    .lock()
                    .unwrap()
                    .tabs
                    .iter()
                    .find(|tab| tab.id == id)
                    .cloned();
                if let Some(tab) = tab {
                    if !tab.untitled && tab.path.exists() {
                        reveal_in_folder(&tab.path);
                    }
                }
            }
            TaoEvent::UserEvent(UserEvent::DownloadSvg(svg)) => {
                let stem = diagram_stem(&session_for_event, &strings);
                let test_path = test_diagram_download_path("svg");
                let is_test = test_path.is_some();
                let Some(path) = test_path.or_else(|| parented(rfd::FileDialog::new())
                    .add_filter("SVG", &["svg"])
                    .set_file_name(&format!("{stem}.svg"))
                    .save_file())
                else {
                    return;
                };
                let mut path = path;
                if path.extension().map(|e| !e.eq_ignore_ascii_case("svg")).unwrap_or(true) {
                    path.set_extension("svg");
                }
                match fs::write(&path, svg) {
                    Ok(()) => {
                        log_event(&format!("diagram svg saved to {}", path.display()));
                        if is_test { return; }
                        show_info_dialog(
                            strings.export_done_title,
                            &format!("{}{}", strings.img_done_body, path.display()),
                        );
                    }
                    Err(error) => show_warning_dialog("Could Not Save", &error.to_string()),
                }
            }
            TaoEvent::UserEvent(UserEvent::DownloadPng(data_url)) => {
                let Some((_, b64)) = data_url.split_once(";base64,") else {
                    return;
                };
                let Some(bytes) = docx::docx_base64_decode(b64) else {
                    show_warning_dialog("Could Not Save", "Invalid image data");
                    return;
                };
                let stem = diagram_stem(&session_for_event, &strings);
                let test_path = test_diagram_download_path("png");
                let is_test = test_path.is_some();
                let Some(path) = test_path.or_else(|| parented(rfd::FileDialog::new())
                    .add_filter("PNG", &["png"])
                    .set_file_name(&format!("{stem}.png"))
                    .save_file())
                else {
                    return;
                };
                let mut path = path;
                if path.extension().map(|e| !e.eq_ignore_ascii_case("png")).unwrap_or(true) {
                    path.set_extension("png");
                }
                match fs::write(&path, bytes) {
                    Ok(()) => {
                        log_event(&format!("diagram png saved to {}", path.display()));
                        if is_test { return; }
                        show_info_dialog(
                            strings.export_done_title,
                            &format!("{}{}", strings.img_done_body, path.display()),
                        );
                    }
                    Err(error) => show_warning_dialog("Could Not Save", &error.to_string()),
                }
            }
            TaoEvent::UserEvent(UserEvent::Quit) => {
                if APP_DIRTY.load(Ordering::SeqCst) && !pending_window_close {
                    log_event("quit requested with unsaved changes: asking webview to save");
                    pending_window_close = true;
                    let _ = webview.evaluate_script(
                        "if(window.__mdPreviewSave)window.__mdPreviewSave();",
                    );
                } else if !pending_window_close {
                    clean_exit_now(&window, &session_for_event.lock().unwrap(), "quit: clean exit");
                } else {
                    log_event("quit requested again while save in flight: ignored");
                }
            }
            TaoEvent::UserEvent(UserEvent::QuitAck) => {
                if pending_window_close {
                    clean_exit_now(&window, &session_for_event.lock().unwrap(), "quit-ack: webview had nothing to save, exiting");
                }
            }
            TaoEvent::UserEvent(UserEvent::SetTheme(choice)) => {
                save_theme_choice(choice);
                window.set_theme(choice.tao_theme());
            }
            TaoEvent::UserEvent(UserEvent::OpenUrl(url)) => {
                let _ = open::that(url);
            }
            TaoEvent::UserEvent(UserEvent::Ready) => {
                // First paint is on the screen; now push hljs into the page
                // (kept out of first-paint HTML to keep it slim). Always do
                // this, even in bench mode, so subsequent panes would still
                // highlight — bench just exits right after measuring.
                let _ = webview.evaluate_script(&hljs_bootstrap);
                let flags = *enhance_flags.lock().unwrap();
                for js in build_enhancer_bootstrap(flags, loaded_enhancers) {
                    let _ = webview.evaluate_script(&js);
                }
                loaded_enhancers.math |= flags.math;
                loaded_enhancers.mermaid |= flags.mermaid;
                update_tabs(&webview, &session_for_event.lock().unwrap());
                if bench {
                    eprintln!("[bench] +{}ms ready", t0.elapsed().as_millis());
                    *control_flow = ControlFlow::Exit;
                }
            }
            // macOS: Finder file opens and embedded Finder Sync actions arrive here.
            TaoEvent::Opened { urls } => {
                let mut paths = Vec::new();
                for url in urls {
                    if let Ok(path) = url.to_file_path() {
                        if is_supported_document(&path) {
                            paths.push(path);
                        }
                        continue;
                    }
                    if let Some(action) = parse_finder_action(url.as_str()) {
                        match action {
                            FinderAction::Create { folder, kind } => match create_finder_file(&folder, &kind) {
                                Ok(path) if kind == "md" => {
                                    let _ = proxy.send_event(UserEvent::OpenPaths(vec![path], true));
                                }
                                Ok(_) => {}
                                Err(error) => show_warning_dialog("Could Not Create File", &error.to_string()),
                            },
                            FinderAction::Terminal { folder } => {
                                if !open_terminal(&folder) {
                                    show_warning_dialog("Could Not Open Terminal", &folder.to_string_lossy());
                                }
                            }
                        }
                    }
                }
                if !paths.is_empty() {
                    let _ = proxy.send_event(UserEvent::OpenPaths(paths, false));
                }
            }
            TaoEvent::WindowEvent {
                event: WindowEvent::Focused(true),
                ..
            } => {
                #[cfg(target_os = "windows")]
                {
                    // Restore both the native WebView2 controller focus and
                    // the textarea focus after Alt+Tab or another foreground
                    // app. This keeps the Windows TSF/IME context attached.
                    let _ = webview.focus();
                    let _ = webview.evaluate_script(
                        "if(window.__mdPreviewRestoreEditorFocus)window.__mdPreviewRestoreEditorFocus();",
                    );
                }
            }
            TaoEvent::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                if APP_DIRTY.load(Ordering::SeqCst) && !pending_window_close {
                    log_event("close requested with unsaved changes: asking webview to save");
                    pending_window_close = true;
                    let _ = webview.evaluate_script(
                        "if(window.__mdPreviewSave)window.__mdPreviewSave();",
                    );
                } else if !pending_window_close {
                    clean_exit_now(&window, &session_for_event.lock().unwrap(), "close requested: clean exit");
                } else {
                    log_event("close requested again while save in flight: ignored");
                }
            }
            _ => {}
        }
    });
}
