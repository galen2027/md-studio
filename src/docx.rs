//! Markdown -> .docx (WordprocessingML) export.
//!
//! Self-contained: hand-rolled stored (uncompressed) ZIP writer plus a
//! pulldown-cmark event walker that emits WordprocessingML directly.
//! Keeping this dependency-light preserves the app's small binary size.

use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct ExportOptions {
    /// 格式标准化: 宋体五号正文、去粗体/底色、去空格、表格表头无底色、图片居中。
    pub standardize: bool,
    /// 去掉标题中的编号（1. / 1.1 / 一、 / （一） / 第一章 / ① …），保留标题层级。
    pub strip_heading_numbers: bool,
    /// 从用户的 template.docx 学到的样式基底（字体/字号/行距/页面），None 用内置默认。
    pub template: Option<TemplateStyle>,
}

/// Style base learned from a user-provided Word template (.docx).
/// Only a targeted set of attributes is extracted; anything missing falls
/// back to the built-in defaults.
#[derive(Clone, Debug, Default)]
pub struct TemplateStyle {
    /// Normal 字体 (ascii, hAnsi, eastAsia)。
    pub normal_fonts: Option<(String, String, String)>,
    /// Normal 字号（半磅，如 24 = 12pt）。
    pub normal_sz: Option<u32>,
    /// 行距 w:line 值（240 = 单倍，360 = 1.5 倍）。
    pub line: Option<u32>,
    /// 正文首行缩进（firstLineChars，200 = 缩进 2 字；0 = 不缩进）。
    pub first_line_chars: Option<u32>,
    /// 标题字号（半磅），按 H1..H6。
    pub heading_sz: [Option<u32>; 6],
    /// 页面 (宽, 高, 上边距, 右边距, 下边距, 左边距)，单位 twips。
    pub page: Option<(u32, u32, u32, u32, u32, u32)>,
}

impl TemplateStyle {
    /// Parse styles.xml + document.xml sectPr out of a .docx package.
    pub fn from_docx(bytes: &[u8]) -> Option<TemplateStyle> {
        let cursor = std::io::Cursor::new(bytes);
        let mut zip = zip::ZipArchive::new(cursor).ok()?;
        let mut styles = String::new();
        let mut document = String::new();
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).ok()?;
            let name = file.name().to_string();
            let target = if name == "word/styles.xml" {
                Some(&mut styles)
            } else if name == "word/document.xml" {
                Some(&mut document)
            } else {
                None
            };
            if let Some(buf) = target {
                std::io::Read::read_to_string(&mut file, buf).ok()?;
            }
        }
        if styles.is_empty() {
            return None;
        }
        Some(Self::from_xml(&styles, &document))
    }

    fn from_xml(styles: &str, document: &str) -> TemplateStyle {
        let mut tpl = TemplateStyle::default();
        if let Some(normal) = find_style_by_name(styles, "Normal") {
            tpl.normal_fonts = attr3(normal, "w:rFonts", &["w:ascii", "w:hAnsi", "w:eastAsia"]);
            tpl.normal_sz = attr_u32(normal, "w:sz", "w:val");
            if let Some(spacing) = find_tag(normal, "w:spacing") {
                tpl.line = attr_val_u32(spacing, "w:line");
            }
            if let Some(ind) = find_tag(normal, "w:ind") {
                tpl.first_line_chars = attr_val_u32(ind, "w:firstLineChars");
            }
        }
        for level in 1..=6u8 {
            let lower = format!("heading {level}");
            let upper = format!("Heading {level}");
            if let Some(h) =
                find_style_by_name(styles, &lower).or_else(|| find_style_by_name(styles, &upper))
            {
                tpl.heading_sz[(level - 1) as usize] = attr_u32(h, "w:sz", "w:val");
            }
        }
        if let Some(sect) = find_tag(document, "w:sectPr") {
            let pgsz = find_tag(sect, "w:pgSz");
            let pgmar = find_tag(sect, "w:pgMar");
            if let (Some(pgsz), Some(pgmar)) = (pgsz, pgmar) {
                let w = attr_val_u32(pgsz, "w:w");
                let h = attr_val_u32(pgsz, "w:h");
                let top = attr_val_u32(pgmar, "w:top");
                let right = attr_val_u32(pgmar, "w:right");
                let bottom = attr_val_u32(pgmar, "w:bottom");
                let left = attr_val_u32(pgmar, "w:left");
                if let (Some(w), Some(h), Some(top), Some(right), Some(bottom), Some(left)) =
                    (w, h, top, right, bottom, left)
                {
                    tpl.page = Some((w, h, top, right, bottom, left));
                }
            }
        }
        tpl
    }

    /// Usable content width in twips, derived from page size and margins.
    pub fn content_width_twips(&self) -> u32 {
        match self.page {
            Some((width, _, _, right, _, left)) => width.saturating_sub(right).saturating_sub(left),
            None => DEFAULT_CONTENT_WIDTH_TWIPS,
        }
    }

    /// Usable content width in EMU, derived from page size and margins.
    pub fn content_width_emu(&self) -> i64 {
        self.content_width_twips() as i64 * 635
    }
}

/// Extract the body of `<w:style ...><w:name w:val="{name}"/>...</w:style>`.
fn find_style_by_name<'a>(styles: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("<w:name w:val=\"{name}\"/>");
    let name_pos = styles.find(&needle)?;
    let style_start = styles[..name_pos].rfind("<w:style ")?;
    let style_end = styles[name_pos..].find("</w:style>")? + name_pos;
    Some(&styles[style_start..style_end])
}

/// Return the first `<tag .../>` element inside `xml`, including its children
/// (through the matching `</tag>`) when it is not self-closing.
fn find_tag<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open_attr = format!("<{tag} ");
    let open_plain = format!("<{tag}>");
    let start = xml.find(&open_attr).or_else(|| xml.find(&open_plain))?;
    let rest = &xml[start..];
    let self_close = rest.find("/>");
    let plain_close = rest.find('>');
    match (self_close, plain_close) {
        (Some(s), Some(p)) if s < p => Some(&rest[..s + 2]),
        (Some(s), None) => Some(&rest[..s + 2]),
        (_, Some(p)) => {
            let close = format!("</{tag}>");
            let end = rest[p..].find(&close).map(|c| p + c + close.len());
            Some(&rest[..end.unwrap_or(p + 1)])
        }
        _ => None,
    }
}

fn attr_val_u32(tag: &str, attr: &str) -> Option<u32> {
    let needle = format!("{attr}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    tag[start..end].parse().ok()
}

fn attr_u32<'a>(xml: &'a str, tag: &str, attr: &str) -> Option<u32> {
    attr_val_u32(find_tag(xml, tag)?, attr)
}

fn attr3(xml: &str, tag: &str, attrs: &[&str; 3]) -> Option<(String, String, String)> {
    let tag = find_tag(xml, tag)?;
    let mut values = Vec::new();
    for attr in attrs {
        let needle = format!("{attr}=\"");
        let start = tag.find(&needle)? + needle.len();
        let end = tag[start..].find('"')? + start;
        values.push(tag[start..end].to_string());
    }
    Some((values[0].clone(), values[1].clone(), values[2].clone()))
}

// ---------------------------------------------------------------------------
// ZIP (stored) writer
// ---------------------------------------------------------------------------

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *slot = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

fn push_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn zip_stored(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();

    for (name, data) in entries {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let size = data.len() as u32;
        let name_bytes = name.as_bytes();

        // Local file header
        out.extend_from_slice(b"PK\x03\x04");
        push_u16(&mut out, 20); // version needed
        push_u16(&mut out, 0x0800); // UTF-8 names
        push_u16(&mut out, 0); // stored
        push_u16(&mut out, 0); // time
        push_u16(&mut out, 0x21); // date: 1980-01-01
        push_u32(&mut out, crc);
        push_u32(&mut out, size);
        push_u32(&mut out, size);
        push_u16(&mut out, name_bytes.len() as u16);
        push_u16(&mut out, 0);
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(data);

        // Central directory record
        central.extend_from_slice(b"PK\x01\x02");
        push_u16(&mut central, 20); // version made by
        push_u16(&mut central, 20);
        push_u16(&mut central, 0x0800);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0x21);
        push_u32(&mut central, crc);
        push_u32(&mut central, size);
        push_u32(&mut central, size);
        push_u16(&mut central, name_bytes.len() as u16);
        push_u16(&mut central, 0); // extra
        push_u16(&mut central, 0); // comment
        push_u16(&mut central, 0); // disk
        push_u16(&mut central, 0); // internal attrs
        push_u32(&mut central, 0); // external attrs
        push_u32(&mut central, offset);
        central.extend_from_slice(name_bytes);
    }

    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06");
    push_u16(&mut out, 0);
    push_u16(&mut out, 0);
    push_u16(&mut out, entries.len() as u16);
    push_u16(&mut out, entries.len() as u16);
    push_u32(&mut out, central.len() as u32);
    push_u32(&mut out, cd_offset);
    push_u16(&mut out, 0);
    out
}

// ---------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------

fn xml_escape(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len());
    for character in s.chars() {
        if !matches!(
            character,
            '\u{9}' | '\u{A}' | '\u{D}'
                | '\u{20}'..='\u{D7FF}'
                | '\u{E000}'..='\u{FFFD}'
                | '\u{10000}'..='\u{10FFFF}'
        ) {
            continue;
        }
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

/// 去掉标题文本开头的编号：1. / 1.1 / 1.1.1 / 1、 / 一、 / （一） / (1) / 第一章 / ① …
pub fn strip_heading_number(text: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"^\s*(?:\d+(?:(?:\.\d+)+[\.、\)]?|[\.、\)])|第[0-9一二三四五六七八九十百千万零两]+[章节回篇部卷][、\.．:]?|[一二三四五六七八九十]+[、\.．]|[（(][0-9一二三四五六七八九十]+[）)]|[①②③④⑤⑥⑦⑧⑨⑩⑪⑫⑬⑭⑮⑯⑰⑱⑲⑳])[\s　]*",
        )
        .expect("heading-number regex compiles")
    });
    re.replace(text, "").trim_start().to_string()
}

fn strip_spaces(text: &str) -> String {
    text.chars()
        .filter(|c| *c != ' ' && *c != '\u{3000}')
        .collect()
}

fn split_front_matter(md: &str) -> &str {
    let mut lines = md.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return md;
    };
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return md;
    }
    let mut offset = first.len();
    for line in lines {
        let value = line.trim_end_matches(['\r', '\n']);
        if value == "---" || value == "..." {
            return &md[offset + line.len()..];
        }
        offset += line.len();
    }
    md
}

// ---------------------------------------------------------------------------
// Image resolution (mirrors the preview's safety rules)
// ---------------------------------------------------------------------------

fn resolve_local_image(base_dir: &Path, url: &str) -> Option<PathBuf> {
    let path_part = url.split(['#', '?']).next()?.trim();
    if path_part.is_empty()
        || path_part.starts_with('#')
        || path_part.starts_with('/')
        || path_part.starts_with('\\')
        || path_part.starts_with("//")
        || path_part.contains(':')
    {
        return None;
    }
    let mut candidate = base_dir.to_path_buf();
    for segment in path_part.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        let segment_path = Path::new(segment);
        if segment_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return None;
        }
        candidate.push(segment_path);
    }
    if candidate == base_dir {
        return None;
    }
    Some(candidate)
}

struct MediaImage {
    rid: String,
    file_name: String,
    bytes: Vec<u8>,
    cx_emu: i64,
    cy_emu: i64,
    doc_pr_id: u32,
}

fn image_drawing_xml(image: &MediaImage) -> String {
    let MediaImage {
        rid,
        file_name,
        cx_emu: cx,
        cy_emu: cy,
        doc_pr_id,
        ..
    } = image;
    format!(
        "<w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\"><wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:docPr id=\"{doc_pr_id}\" name=\"{file_name}\"/><a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:pic><pic:nvPicPr><pic:cNvPr id=\"{doc_pr_id}\" name=\"{file_name}\"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed=\"{rid}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"
    )
}

fn decode_image(bytes: &[u8], fallback_ext: &str) -> Option<(String, u32, u32)> {
    let format = image::guess_format(bytes).ok()?;
    let (ext, ok) = match format {
        image::ImageFormat::Png => ("png", true),
        image::ImageFormat::Jpeg => ("jpg", true),
        image::ImageFormat::Gif => ("gif", true),
        image::ImageFormat::Bmp => ("bmp", true),
        _ => (fallback_ext, false),
    };
    if !ok {
        return None;
    }
    let dims = image::load_from_memory(bytes).ok().map(|img| {
        use image::GenericImageView;
        img.dimensions()
    })?;
    Some((ext.to_string(), dims.0, dims.1))
}

fn parse_logical_image_size(title: &str) -> Option<(u32, u32)> {
    let value = title.strip_prefix("mdp-logical-size=")?;
    let (width, height) = value.split_once('x')?;
    let width = width.parse::<f64>().ok()?;
    let height = height.parse::<f64>().ok()?;
    if !width.is_finite()
        || !height.is_finite()
        || width < 1.0
        || height < 1.0
        || width > 16_000.0
        || height > 16_000.0
    {
        return None;
    }
    Some((width.round() as u32, height.round() as u32))
}

// ---------------------------------------------------------------------------
// Exporter
// ---------------------------------------------------------------------------

const DEFAULT_CONTENT_WIDTH_TWIPS: u32 = 9_026;
const MAX_IMG_WIDTH_EMU: i64 = DEFAULT_CONTENT_WIDTH_TWIPS as i64 * 635;
const TABLE_MIN_COLUMN_WIDTH_TWIPS: u32 = 480;
const TABLE_HORIZONTAL_CELL_MARGIN_TWIPS: u32 = 120;
const TABLE_NATURAL_UNIT_TWIPS: u32 = 120;

struct ListCtx {
    num_id: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TableRunStyle {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    hyperlink_rid: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TableInline {
    Text {
        text: String,
        style: TableRunStyle,
    },
    Image {
        dest_url: String,
        alt: String,
        logical_size: Option<(u32, u32)>,
        style: TableRunStyle,
    },
    Break,
}

struct TablePendingImage {
    dest_url: String,
    alt: String,
    logical_size: Option<(u32, u32)>,
    style: TableRunStyle,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TableCell {
    inlines: Vec<TableInline>,
}

impl TableCell {
    fn push_text(&mut self, text: &str, style: TableRunStyle) {
        for (index, part) in text.replace('\r', "").split('\n').enumerate() {
            if index > 0 {
                self.inlines.push(TableInline::Break);
            }
            if part.is_empty() {
                continue;
            }
            match self.inlines.last_mut() {
                Some(TableInline::Text {
                    text: previous,
                    style: previous_style,
                }) if *previous_style == style => previous.push_str(part),
                _ => self.inlines.push(TableInline::Text {
                    text: part.to_string(),
                    style: style.clone(),
                }),
            }
        }
    }

    fn push_break(&mut self) {
        self.inlines.push(TableInline::Break);
    }

    fn push_image(&mut self, image: TablePendingImage) {
        self.inlines.push(TableInline::Image {
            dest_url: image.dest_url,
            alt: image.alt,
            logical_size: image.logical_size,
            style: image.style,
        });
    }

    fn display_text(&self, standardize: bool) -> String {
        let mut text = String::new();
        for inline in &self.inlines {
            match inline {
                TableInline::Text { text: value, style } => {
                    if standardize && !style.code {
                        text.push_str(&strip_spaces(value));
                    } else {
                        text.push_str(value);
                    }
                }
                TableInline::Image { alt, .. } => {
                    if alt.is_empty() {
                        text.push_str("图片");
                    } else if standardize {
                        text.push_str(&strip_spaces(alt));
                    } else {
                        text.push_str(alt);
                    }
                }
                TableInline::Break => text.push('\n'),
            }
        }
        text
    }

    fn is_multiline(&self) -> bool {
        self.inlines.iter().any(|inline| match inline {
            TableInline::Break => true,
            TableInline::Image { alt, .. } => alt.contains('\n'),
            TableInline::Text { .. } => false,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TableRow {
    cells: Vec<TableCell>,
    is_header: bool,
}

#[derive(Clone, Debug)]
struct TableModel {
    alignments: Vec<Alignment>,
    rows: Vec<TableRow>,
}

impl TableModel {
    fn column_count(&self) -> usize {
        self.rows
            .iter()
            .map(|row| row.cells.len())
            .max()
            .unwrap_or(0)
            .max(self.alignments.len())
    }

    fn layout(&self, available_width: u32, standardize: bool) -> TableLayout {
        let column_count = self.column_count();
        if column_count == 0 {
            return TableLayout::default();
        }

        let mut column_weights = vec![4u32; column_count];
        let mut has_long_content = false;
        let mut has_multiline_content = false;
        for row in &self.rows {
            for (column, cell) in row.cells.iter().enumerate() {
                let text = cell.display_text(standardize);
                let raw_width = text.split('\n').map(visual_text_width).max().unwrap_or(0);
                has_long_content |= raw_width > 32;
                has_multiline_content |= cell.is_multiline();
                column_weights[column] = column_weights[column].max(raw_width.clamp(4, 32));
            }
        }

        let available_width = available_width.max(1);
        let natural_widths: Vec<u32> = column_weights
            .iter()
            .map(|weight| {
                weight
                    .saturating_mul(TABLE_NATURAL_UNIT_TWIPS)
                    .saturating_add(TABLE_HORIZONTAL_CELL_MARGIN_TWIPS * 2)
                    .max(TABLE_MIN_COLUMN_WIDTH_TWIPS)
            })
            .collect();
        let natural_table_width = natural_widths
            .iter()
            .copied()
            .fold(0u32, u32::saturating_add);
        let minimum_percent = match column_count {
            1 | 2 => 50,
            3 | 4 => 70,
            _ => 100,
        };
        let minimum_table_width = ((available_width as u64 * minimum_percent + 99) / 100) as u32;
        let use_full_width = column_count >= 5 || has_long_content || has_multiline_content;
        let table_width = if use_full_width {
            available_width
        } else {
            natural_table_width
                .max(minimum_table_width)
                .min(available_width)
        };

        let mut alignments = Vec::with_capacity(column_count);
        for column in 0..column_count {
            let explicit = self
                .alignments
                .get(column)
                .copied()
                .unwrap_or(Alignment::None);
            let alignment = match explicit {
                Alignment::Left => TableColumnAlignment::Left,
                Alignment::Center => TableColumnAlignment::Center,
                Alignment::Right => TableColumnAlignment::Right,
                Alignment::None => {
                    let mut saw_value = false;
                    let all_numeric = self.rows.iter().filter(|row| !row.is_header).all(|row| {
                        let value = row
                            .cells
                            .get(column)
                            .map(|cell| cell.display_text(standardize))
                            .unwrap_or_default();
                        let value = value.trim();
                        if value.is_empty() {
                            true
                        } else {
                            saw_value = true;
                            is_numeric_table_value(value)
                        }
                    });
                    if saw_value && all_numeric {
                        TableColumnAlignment::Right
                    } else {
                        TableColumnAlignment::Left
                    }
                }
            };
            alignments.push(alignment);
        }

        TableLayout {
            table_width,
            column_widths: distribute_column_widths(table_width, &natural_widths),
            alignments,
            compact: column_count >= 7,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TableColumnAlignment {
    Left,
    Center,
    Right,
}

impl TableColumnAlignment {
    fn word_value(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TableLayout {
    table_width: u32,
    column_widths: Vec<u32>,
    alignments: Vec<TableColumnAlignment>,
    compact: bool,
}

fn distribute_column_widths(table_width: u32, natural_widths: &[u32]) -> Vec<u32> {
    if natural_widths.is_empty() {
        return Vec::new();
    }
    let column_count = natural_widths.len() as u32;
    let minimum_total = TABLE_MIN_COLUMN_WIDTH_TWIPS.saturating_mul(column_count);
    if table_width < minimum_total {
        // A malformed or extremely narrow template can make both constraints
        // impossible. Keep the table inside the real page width in that case.
        let base = table_width / column_count;
        let remainder = table_width % column_count;
        return (0..column_count)
            .map(|index| base + u32::from(index < remainder))
            .collect();
    }

    let remaining = table_width - minimum_total;
    let weights: Vec<u32> = natural_widths
        .iter()
        .map(|width| width.saturating_sub(TABLE_MIN_COLUMN_WIDTH_TWIPS).max(1))
        .collect();
    let weight_sum: u64 = weights.iter().map(|weight| *weight as u64).sum();
    let mut widths = vec![TABLE_MIN_COLUMN_WIDTH_TWIPS; natural_widths.len()];
    let mut remainders = Vec::with_capacity(natural_widths.len());
    let mut allocated = 0u32;
    for (index, weight) in weights.iter().enumerate() {
        let numerator = remaining as u64 * *weight as u64;
        let addition = (numerator / weight_sum) as u32;
        widths[index] += addition;
        allocated += addition;
        remainders.push((numerator % weight_sum, index));
    }
    remainders.sort_by(|left, right| right.cmp(left));
    for (_, index) in remainders
        .into_iter()
        .take((remaining - allocated) as usize)
    {
        widths[index] += 1;
    }
    widths
}

fn visual_text_width(text: &str) -> u32 {
    text.chars().map(visual_char_width).sum()
}

fn visual_char_width(character: char) -> u32 {
    let code = character as u32;
    if character.is_ascii() {
        1
    } else if matches!(
        code,
        0x2E80..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE19
            | 0xFE30..=0xFE6F
            | 0xFF01..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x20000..=0x3FFFD
    ) {
        2
    } else {
        1
    }
}

fn is_numeric_table_value(value: &str) -> bool {
    use regex::Regex;
    use std::sync::OnceLock;

    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^(?:[+-]?\s*[¥￥$€£]?\s*(?:[0-9]{1,3}(?:,[0-9]{3})+|[0-9]+)(?:\.[0-9]+)?\s*(?:[%％]|元|圆)?|\([+-]?\s*[¥￥$€£]?\s*(?:[0-9]{1,3}(?:,[0-9]{3})+|[0-9]+)(?:\.[0-9]+)?\s*(?:[%％]|元|圆)?\))$",
        )
        .expect("numeric-table-value regex compiles")
    })
    .is_match(value.trim())
}

fn html_break_count(html: &str) -> usize {
    let mut rest = html.trim();
    let mut count = 0;
    while !rest.is_empty() {
        let length = ["<br />", "<br/>", "<br>"].iter().find_map(|tag| {
            rest.get(..tag.len())
                .filter(|prefix| prefix.eq_ignore_ascii_case(tag))
                .map(|_| tag.len())
        });
        let Some(length) = length else {
            return 0;
        };
        count += 1;
        rest = rest[length..].trim_start();
    }
    count
}

struct TableBuilder {
    model: TableModel,
    current_row: Option<TableRow>,
    current_cell: Option<TableCell>,
    in_header: bool,
    bold_depth: usize,
    italic_depth: usize,
    strike: bool,
    hyperlink_rid: Option<String>,
    in_image: Option<TablePendingImage>,
}

impl TableBuilder {
    fn new(alignments: Vec<Alignment>) -> Self {
        Self {
            model: TableModel {
                alignments,
                rows: Vec::new(),
            },
            current_row: None,
            current_cell: None,
            in_header: false,
            bold_depth: 0,
            italic_depth: 0,
            strike: false,
            hyperlink_rid: None,
            in_image: None,
        }
    }

    fn start_row(&mut self, is_header: bool) {
        self.finish_row();
        self.current_row = Some(TableRow {
            cells: Vec::new(),
            is_header,
        });
    }

    fn finish_cell(&mut self) {
        let Some(cell) = self.current_cell.take() else {
            return;
        };
        if self.current_row.is_none() {
            self.start_row(self.in_header);
        }
        if let Some(row) = self.current_row.as_mut() {
            row.cells.push(cell);
        }
    }

    fn finish_row(&mut self) {
        self.finish_cell();
        if let Some(row) = self.current_row.take() {
            if !row.cells.is_empty() {
                self.model.rows.push(row);
            }
        }
    }

    fn style(&self, code: bool) -> TableRunStyle {
        TableRunStyle {
            bold: self.bold_depth > 0,
            italic: self.italic_depth > 0,
            strike: self.strike,
            code,
            hyperlink_rid: self.hyperlink_rid.clone(),
        }
    }

    fn push_text(&mut self, text: &str, code: bool) {
        if self.current_cell.is_none() {
            self.current_cell = Some(TableCell::default());
        }
        let style = self.style(code);
        if let Some(cell) = self.current_cell.as_mut() {
            cell.push_text(text, style);
        }
    }

    fn push_break(&mut self) {
        if self.current_cell.is_none() {
            self.current_cell = Some(TableCell::default());
        }
        if let Some(cell) = self.current_cell.as_mut() {
            cell.push_break();
        }
    }

    fn finish_image(&mut self) {
        let Some(image) = self.in_image.take() else {
            return;
        };
        if self.current_cell.is_none() {
            self.current_cell = Some(TableCell::default());
        }
        if let Some(cell) = self.current_cell.as_mut() {
            cell.push_image(image);
        }
    }

    fn handle_event(&mut self, event: Event<'_>, exporter: &mut Exporter<'_>) -> bool {
        match event {
            Event::Start(tag) => match tag {
                Tag::TableHead => {
                    self.in_header = true;
                    self.start_row(true);
                }
                Tag::TableRow => self.start_row(self.in_header),
                Tag::TableCell => self.current_cell = Some(TableCell::default()),
                Tag::Strong => self.bold_depth = self.bold_depth.saturating_add(1),
                Tag::Emphasis => self.italic_depth = self.italic_depth.saturating_add(1),
                Tag::Strikethrough => self.strike = true,
                Tag::Link { dest_url, .. } => {
                    self.hyperlink_rid = exporter.register_hyperlink(&dest_url)
                }
                Tag::Image {
                    dest_url, title, ..
                } => {
                    self.in_image = Some(TablePendingImage {
                        dest_url: dest_url.to_string(),
                        alt: String::new(),
                        logical_size: parse_logical_image_size(&title),
                        style: self.style(false),
                    });
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Table => {
                    self.finish_row();
                    return true;
                }
                TagEnd::TableHead => {
                    self.finish_row();
                    self.in_header = false;
                }
                TagEnd::TableRow => self.finish_row(),
                TagEnd::TableCell => self.finish_cell(),
                TagEnd::Strong => self.bold_depth = self.bold_depth.saturating_sub(1),
                TagEnd::Emphasis => self.italic_depth = self.italic_depth.saturating_sub(1),
                TagEnd::Strikethrough => self.strike = false,
                TagEnd::Link => self.hyperlink_rid = None,
                TagEnd::Image => self.finish_image(),
                _ => {}
            },
            Event::Text(text) => {
                if let Some(image) = self.in_image.as_mut() {
                    image.alt.push_str(&text);
                } else {
                    self.push_text(&text, false);
                }
            }
            Event::Code(code) => {
                if let Some(image) = self.in_image.as_mut() {
                    image.alt.push_str(&code);
                } else {
                    self.push_text(&code, true);
                }
            }
            Event::SoftBreak => {
                if let Some(image) = self.in_image.as_mut() {
                    image.alt.push(' ');
                } else {
                    self.push_text(" ", false);
                }
            }
            Event::HardBreak => {
                if let Some(image) = self.in_image.as_mut() {
                    image.alt.push('\n');
                } else {
                    self.push_break();
                }
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                for _ in 0..html_break_count(&html) {
                    self.push_break();
                }
            }
            Event::InlineMath(math) => self.push_text(&format!("${math}$"), false),
            Event::DisplayMath(math) => self.push_text(&format!("$${math}$$"), false),
            Event::TaskListMarker(checked) => {
                self.push_text(if checked { "☑ " } else { "☐ " }, false)
            }
            Event::FootnoteReference(name) => self.push_text(&format!("[^{name}]"), false),
            _ => {}
        }
        false
    }

    fn finish(mut self) -> TableModel {
        self.finish_row();
        self.model
    }
}

struct Exporter<'a> {
    body: String,
    opts: &'a ExportOptions,
    base_dir: Option<&'a Path>,

    // inline state
    bold_depth: usize,
    italic_depth: usize,
    strike: bool,
    inline_code: bool,

    // block state
    para_open: bool,
    para_ppr_insert_pos: usize,
    para_has_ppr: bool,
    para_has_non_image_content: bool,
    para_has_image: bool,
    heading_level: Option<u8>,
    heading_text_done: bool,
    in_code_block: bool,
    quote_depth: u32,
    list_stack: Vec<ListCtx>,
    in_image: Option<(String, String, Option<(u32, u32)>)>, // (dest_url, alt, logical px)

    // registries
    numbering: String,
    next_num_id: u32,
    hyperlink_rels: String,
    next_relationship_id: u64,
    link_color_open: bool,
    media: Vec<MediaImage>,
    max_img_width_emu: i64,
    table_width_twips: u32,
}

impl<'a> Exporter<'a> {
    fn new(opts: &'a ExportOptions, base_dir: Option<&'a Path>) -> Self {
        let table_width_twips = opts
            .template
            .as_ref()
            .map(|template| template.content_width_twips())
            .unwrap_or(DEFAULT_CONTENT_WIDTH_TWIPS);
        let max_img_width_emu = opts
            .template
            .as_ref()
            .map(|t| t.content_width_emu())
            .unwrap_or(MAX_IMG_WIDTH_EMU);
        Exporter {
            body: String::new(),
            opts,
            base_dir,
            bold_depth: 0,
            italic_depth: 0,
            strike: false,
            inline_code: false,
            para_open: false,
            para_ppr_insert_pos: 0,
            para_has_ppr: false,
            para_has_non_image_content: false,
            para_has_image: false,
            heading_level: None,
            heading_text_done: false,
            in_code_block: false,
            quote_depth: 0,
            list_stack: Vec::new(),
            in_image: None,
            numbering: String::new(),
            next_num_id: 1, // numId 1 = shared bullet list
            hyperlink_rels: String::new(),
            // rId1 and rId2 are reserved for styles.xml and numbering.xml.
            next_relationship_id: 3,
            link_color_open: false,
            media: Vec::new(),
            max_img_width_emu,
            table_width_twips,
        }
    }

    fn open_para(&mut self, ppr: &str) {
        if self.para_open {
            return;
        }
        self.body.push_str("<w:p>");
        if !ppr.is_empty() {
            self.body.push_str("<w:pPr>");
            self.body.push_str(ppr);
            self.para_ppr_insert_pos = self.body.len();
            self.body.push_str("</w:pPr>");
            self.para_has_ppr = true;
        } else {
            self.para_ppr_insert_pos = self.body.len();
            self.para_has_ppr = false;
        }
        self.para_has_non_image_content = false;
        self.para_has_image = false;
        self.para_open = true;
    }

    fn close_para(&mut self) {
        if self.para_open {
            if self.para_has_image && !self.para_has_non_image_content {
                if self.para_has_ppr {
                    self.body
                        .insert_str(self.para_ppr_insert_pos, "<w:jc w:val=\"center\"/>");
                } else {
                    self.body.insert_str(
                        self.para_ppr_insert_pos,
                        "<w:pPr><w:ind w:firstLine=\"0\" w:firstLineChars=\"0\"/><w:jc w:val=\"center\"/></w:pPr>",
                    );
                }
            }
            self.body.push_str("</w:p>");
            self.para_open = false;
            self.para_has_ppr = false;
            self.para_has_non_image_content = false;
            self.para_has_image = false;
        }
    }

    fn block_ppr(&self) -> String {
        // Paragraph directly inside a blockquote.
        if self.quote_depth > 0 {
            return "<w:pBdr><w:left w:val=\"single\" w:sz=\"12\" w:space=\"8\" w:color=\"C0C4C8\"/></w:pBdr><w:ind w:left=\"340\" w:firstLine=\"0\" w:firstLineChars=\"0\"/>".to_string();
        }
        String::new() // Normal style supplies 1.5 spacing + first-line indent.
    }

    fn list_item_ppr(&self) -> String {
        let depth = self.list_stack.len().min(3);
        let ilvl = depth.saturating_sub(1);
        let num_id = self.list_stack.last().map(|ctx| ctx.num_id).unwrap_or(1);
        let left = 480 * depth as u32;
        format!(
            "<w:numPr><w:ilvl w:val=\"{ilvl}\"/><w:numId w:val=\"{num_id}\"/></w:numPr><w:ind w:left=\"{left}\" w:hanging=\"240\" w:firstLine=\"0\" w:firstLineChars=\"0\"/>"
        )
    }

    fn heading_ppr(level: u8) -> String {
        format!("<w:pStyle w:val=\"Heading{level}\"/>")
    }

    fn code_block_ppr() -> &'static str {
        // 代码块浅灰底在默认与格式标准化两种模式下都保留。
        "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"F5F5F5\"/><w:ind w:left=\"240\" w:firstLine=\"0\" w:firstLineChars=\"0\"/><w:spacing w:before=\"60\" w:after=\"60\"/>"
    }

    fn run_rpr(&self) -> String {
        let mut rpr = String::new();
        if self.inline_code || self.in_code_block {
            rpr.push_str(
                "<w:rFonts w:ascii=\"Microsoft YaHei\" w:hAnsi=\"Microsoft YaHei\" w:eastAsia=\"微软雅黑\"/><w:sz w:val=\"20\"/>",
            );
            if self.inline_code && !self.in_code_block && !self.opts.standardize {
                rpr.push_str("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"F2F2F2\"/>");
            }
            return rpr;
        }
        let bold = self.bold_depth > 0 && !(self.opts.standardize && self.heading_level.is_none());
        if bold {
            rpr.push_str("<w:b/><w:bCs/>");
        }
        if self.italic_depth > 0 {
            rpr.push_str("<w:i/>");
        }
        if self.strike {
            rpr.push_str("<w:strike/>");
        }
        if self.link_color_open {
            rpr.push_str("<w:color w:val=\"1565C0\"/><w:u w:val=\"single\"/>");
        }
        rpr
    }

    fn emit_text(&mut self, text: &str) {
        if self.in_image.is_some() {
            return;
        }
        let mut text = text.to_string();
        if self.heading_level.is_some() {
            if self.opts.strip_heading_numbers && !self.heading_text_done {
                text = strip_heading_number(&text);
                self.heading_text_done = true;
            }
        } else if self.opts.standardize && !self.in_code_block && !self.inline_code {
            text = strip_spaces(&text);
        }
        if text.is_empty() {
            return;
        }
        self.para_has_non_image_content = true;
        if self.in_code_block {
            let rpr = self.run_rpr();
            let escaped = xml_escape(&text);
            let with_breaks = escaped.replace('\n', "</w:t><w:br/><w:t xml:space=\"preserve\">");
            self.body.push_str("<w:r>");
            if !rpr.is_empty() {
                self.body.push_str("<w:rPr>");
                self.body.push_str(&rpr);
                self.body.push_str("</w:rPr>");
            }
            self.body.push_str("<w:t xml:space=\"preserve\">");
            self.body.push_str(&with_breaks);
            self.body.push_str("</w:t></w:r>");
            return;
        }
        let rpr = self.run_rpr();
        self.body.push_str("<w:r>");
        if !rpr.is_empty() {
            self.body.push_str("<w:rPr>");
            self.body.push_str(&rpr);
            self.body.push_str("</w:rPr>");
        }
        self.body.push_str("<w:t xml:space=\"preserve\">");
        self.body.push_str(&xml_escape(&text));
        self.body.push_str("</w:t></w:r>");
    }

    fn start_list(&mut self, start: Option<u64>) {
        if start.is_some() {
            self.next_num_id += 1;
            let num_id = self.next_num_id;
            let start_value = start.unwrap_or(1);
            self.numbering.push_str(&format!(
                "<w:num w:numId=\"{num_id}\"><w:abstractNumId w:val=\"1\"/><w:lvlOverride w:ilvl=\"0\"><w:startOverride w:val=\"{start_value}\"/></w:lvlOverride></w:num>"
            ));
            self.list_stack.push(ListCtx { num_id });
        } else {
            // Bullets share one abstract definition but need their own num so
            // nested/sibling lists keep independent indentation state.
            self.next_num_id += 1;
            let num_id = self.next_num_id;
            self.numbering.push_str(&format!(
                "<w:num w:numId=\"{num_id}\"><w:abstractNumId w:val=\"0\"/></w:num>"
            ));
            self.list_stack.push(ListCtx { num_id });
        }
        self.close_para();
    }

    fn register_hyperlink(&mut self, url: &str) -> Option<String> {
        let external =
            url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:");
        if external {
            let rid = self.allocate_relationship_id();
            self.hyperlink_rels.push_str(&format!(
                "<Relationship Id=\"{rid}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink\" Target=\"{}\" TargetMode=\"External\"/>",
                xml_escape(url)
            ));
            Some(rid)
        } else {
            None
        }
    }

    fn allocate_relationship_id(&mut self) -> String {
        let rid = format!("rId{}", self.next_relationship_id);
        self.next_relationship_id += 1;
        rid
    }

    fn start_hyperlink(&mut self, url: &str) {
        let rid = self.register_hyperlink(url);
        if let Some(rid) = rid.as_ref() {
            self.body
                .push_str(&format!("<w:hyperlink r:id=\"{rid}\" w:history=\"1\">"));
        }
        // Anchor / relative links keep their text but lose navigation.
        self.link_color_open = rid.is_some();
    }

    fn render_table_run_properties(
        &self,
        style: &TableRunStyle,
        is_header: bool,
        compact: bool,
    ) -> String {
        let mut properties = String::new();
        if style.code {
            properties.push_str(
                "<w:rFonts w:ascii=\"Microsoft YaHei\" w:hAnsi=\"Microsoft YaHei\" w:eastAsia=\"微软雅黑\"/>",
            );
        }
        if !self.opts.standardize && (is_header || style.bold) {
            properties.push_str("<w:b/><w:bCs/>");
        }
        if style.italic {
            properties.push_str("<w:i/>");
        }
        if style.strike {
            properties.push_str("<w:strike/>");
        }
        if style.hyperlink_rid.is_some() {
            properties.push_str("<w:color w:val=\"1565C0\"/>");
        }
        if compact {
            properties.push_str("<w:sz w:val=\"18\"/><w:szCs w:val=\"18\"/>");
        } else if style.code {
            properties.push_str("<w:sz w:val=\"20\"/><w:szCs w:val=\"20\"/>");
        }
        if style.hyperlink_rid.is_some() {
            properties.push_str("<w:u w:val=\"single\"/>");
        }
        if style.code && !self.opts.standardize {
            properties.push_str("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"F2F2F2\"/>");
        }
        properties
    }

    fn render_table_text_run(
        &mut self,
        text: &str,
        style: &TableRunStyle,
        is_header: bool,
        compact: bool,
    ) {
        let text = if self.opts.standardize && !style.code {
            strip_spaces(text)
        } else {
            text.to_string()
        };
        if text.is_empty() {
            return;
        }
        if let Some(rid) = style.hyperlink_rid.as_ref() {
            self.body
                .push_str(&format!("<w:hyperlink r:id=\"{rid}\" w:history=\"1\">"));
        }
        let properties = self.render_table_run_properties(style, is_header, compact);
        self.body.push_str("<w:r>");
        if !properties.is_empty() {
            self.body.push_str("<w:rPr>");
            self.body.push_str(&properties);
            self.body.push_str("</w:rPr>");
        }
        self.body.push_str("<w:t xml:space=\"preserve\">");
        self.body.push_str(&xml_escape(&text));
        self.body.push_str("</w:t></w:r>");
        if style.hyperlink_rid.is_some() {
            self.body.push_str("</w:hyperlink>");
        }
    }

    fn render_table(&mut self, table: TableModel) {
        let layout = table.layout(self.table_width_twips, self.opts.standardize);
        if layout.column_widths.is_empty() {
            return;
        }
        let keep_header_with_body = table.rows.iter().any(|row| !row.is_header);

        self.close_para();
        self.body.push_str(&format!(
            "<w:tbl><w:tblPr><w:tblW w:w=\"{}\" w:type=\"dxa\"/><w:tblBorders><w:top w:val=\"single\" w:sz=\"6\" w:space=\"0\" w:color=\"808080\"/><w:left w:val=\"single\" w:sz=\"6\" w:space=\"0\" w:color=\"808080\"/><w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"0\" w:color=\"808080\"/><w:right w:val=\"single\" w:sz=\"6\" w:space=\"0\" w:color=\"808080\"/><w:insideH w:val=\"single\" w:sz=\"6\" w:space=\"0\" w:color=\"808080\"/><w:insideV w:val=\"single\" w:sz=\"6\" w:space=\"0\" w:color=\"808080\"/></w:tblBorders><w:tblLayout w:type=\"fixed\"/><w:tblCellMar><w:top w:w=\"80\" w:type=\"dxa\"/><w:left w:w=\"120\" w:type=\"dxa\"/><w:bottom w:w=\"80\" w:type=\"dxa\"/><w:right w:w=\"120\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr><w:tblGrid>",
            layout.table_width
        ));
        for width in &layout.column_widths {
            self.body.push_str(&format!("<w:gridCol w:w=\"{width}\"/>"));
        }
        self.body.push_str("</w:tblGrid>");

        for row in &table.rows {
            let multiline = row.cells.iter().any(TableCell::is_multiline);
            self.body.push_str("<w:tr>");
            if row.is_header {
                self.body
                    .push_str("<w:trPr><w:cantSplit/><w:tblHeader/></w:trPr>");
            }
            let keep_next = if row.is_header && keep_header_with_body {
                "<w:keepNext/>"
            } else {
                ""
            };
            for column in 0..layout.column_widths.len() {
                let width = layout.column_widths[column];
                let vertical_alignment = if multiline { "top" } else { "center" };
                let shading = if row.is_header && !self.opts.standardize {
                    "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"EAF2F8\"/>"
                } else {
                    ""
                };
                let standardized_header_border = if row.is_header && self.opts.standardize {
                    "<w:tcBorders><w:bottom w:val=\"single\" w:sz=\"8\" w:space=\"0\" w:color=\"808080\"/></w:tcBorders>"
                } else {
                    ""
                };
                self.body.push_str(&format!(
                    "<w:tc><w:tcPr><w:tcW w:w=\"{width}\" w:type=\"dxa\"/>{standardized_header_border}{shading}<w:vAlign w:val=\"{vertical_alignment}\"/></w:tcPr><w:p><w:pPr>{keep_next}<w:spacing w:before=\"0\" w:after=\"0\" w:line=\"276\" w:lineRule=\"auto\"/><w:ind w:firstLine=\"0\" w:firstLineChars=\"0\"/><w:jc w:val=\"{}\"/></w:pPr>",
                    layout.alignments[column].word_value()
                ));
                if let Some(cell) = row.cells.get(column) {
                    for inline in &cell.inlines {
                        match inline {
                            TableInline::Break => self.body.push_str("<w:r><w:br/></w:r>"),
                            TableInline::Text { text, style } => {
                                self.render_table_text_run(
                                    text,
                                    style,
                                    row.is_header,
                                    layout.compact,
                                );
                            }
                            TableInline::Image {
                                dest_url,
                                alt,
                                logical_size,
                                style,
                            } => {
                                let max_width_emu = i64::from(
                                    width
                                        .saturating_sub(TABLE_HORIZONTAL_CELL_MARGIN_TWIPS * 2)
                                        .max(1),
                                ) * 635;
                                if let Some(image_index) =
                                    self.register_image(dest_url, *logical_size, max_width_emu)
                                {
                                    let drawing = image_drawing_xml(&self.media[image_index]);
                                    if let Some(rid) = style.hyperlink_rid.as_ref() {
                                        self.body.push_str(&format!(
                                            "<w:hyperlink r:id=\"{rid}\" w:history=\"1\">"
                                        ));
                                    }
                                    self.body.push_str(&drawing);
                                    if style.hyperlink_rid.is_some() {
                                        self.body.push_str("</w:hyperlink>");
                                    }
                                } else if !alt.is_empty() {
                                    self.render_table_text_run(
                                        &format!("[图片] {alt}"),
                                        style,
                                        row.is_header,
                                        layout.compact,
                                    );
                                }
                            }
                        }
                    }
                }
                self.body.push_str("</w:p></w:tc>");
            }
            self.body.push_str("</w:tr>");
        }
        self.body.push_str("</w:tbl>");
    }

    fn register_image(
        &mut self,
        dest: &str,
        logical_size: Option<(u32, u32)>,
        max_width_emu: i64,
    ) -> Option<usize> {
        let resolved: Option<(Vec<u8>, String)> = if let Some(data) = dest.strip_prefix("data:") {
            data.split_once(";base64,").and_then(|(mime, b64)| {
                let ext = mime.strip_prefix("image/").unwrap_or("png").to_string();
                docx_base64_decode(b64).map(|bytes| (bytes, ext))
            })
        } else {
            self.base_dir
                .and_then(|base| resolve_local_image(base, &dest))
                .and_then(|path| {
                    let ext = path
                        .extension()
                        .map(|e| e.to_string_lossy().to_ascii_lowercase())
                        .unwrap_or_else(|| "png".to_string());
                    std::fs::read(path).ok().map(|bytes| (bytes, ext))
                })
        };
        let (bytes, ext) = resolved?;
        let (ext, w_px, h_px) = decode_image(&bytes, &ext)?;

        let rid = self.allocate_relationship_id();
        let file_name = format!("image{}.{}", self.media.len() + 1, ext);
        // Mermaid export carries its logical SVG size separately from the
        // 300-PPI fallback PNG. This keeps Word's placed size stable while
        // increasing only the raster sample count.
        let (layout_w_px, layout_h_px) = logical_size.unwrap_or((w_px, h_px));
        let mut cx = layout_w_px as i64 * 9525;
        let mut cy = layout_h_px as i64 * 9525;
        let max_width_emu = max_width_emu.max(1);
        if cx > max_width_emu {
            cy = cy * max_width_emu / cx;
            cx = max_width_emu;
        }
        let doc_pr_id = 1000 + self.media.len() as u32;
        self.media.push(MediaImage {
            rid,
            file_name,
            bytes,
            cx_emu: cx,
            cy_emu: cy,
            doc_pr_id,
        });
        Some(self.media.len() - 1)
    }

    fn emit_image(&mut self) {
        let Some((dest, alt, logical_size)) = self.in_image.take() else {
            return;
        };
        let Some(image_index) = self.register_image(&dest, logical_size, self.max_img_width_emu)
        else {
            // Unresolvable image: keep the alt text in the surrounding paragraph.
            if !alt.is_empty() {
                if !self.para_open {
                    self.open_para(&self.block_ppr());
                }
                self.emit_text(&format!("[图片] {alt}"));
            }
            return;
        };
        let drawing = image_drawing_xml(&self.media[image_index]);

        if !self.para_open {
            self.open_para(&self.block_ppr());
        }
        self.body.push_str(&drawing);
        self.para_has_image = true;
    }
}

pub fn docx_base64_decode(input: &str) -> Option<Vec<u8>> {
    let table = |b: u8| -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let bytes: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        if chunk.len() < 2 {
            return None;
        }
        let pad = chunk.iter().filter(|&&b| b == b'=').count();
        if chunk.len() < 4 && pad == 0 {
            return None;
        }
        let v0 = table(chunk[0])?;
        let v1 = table(chunk[1])?;
        let v2 = if chunk.len() > 2 && chunk[2] != b'=' {
            table(chunk[2])?
        } else {
            0
        };
        let v3 = if chunk.len() > 3 && chunk[3] != b'=' {
            table(chunk[3])?
        } else {
            0
        };
        let n = ((v0 as u32) << 18) | ((v1 as u32) << 12) | ((v2 as u32) << 6) | v3 as u32;
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 && chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Static XML parts
// ---------------------------------------------------------------------------

fn styles_xml(standardize: bool, tpl: Option<&TemplateStyle>) -> String {
    let tpl_fonts = tpl.and_then(|t| t.normal_fonts.clone());
    let (ascii, hansi, east, sz) = if standardize {
        (
            "Times New Roman".to_string(),
            "Times New Roman".to_string(),
            "宋体".to_string(),
            21,
        ) // 五号
    } else if let Some((a, h, e)) = tpl_fonts {
        let sz = tpl.and_then(|t| t.normal_sz).unwrap_or(22);
        (a, h, e, sz)
    } else {
        (
            "Microsoft YaHei".to_string(),
            "Microsoft YaHei".to_string(),
            "微软雅黑".to_string(),
            22,
        ) // 11pt
    };
    let line = tpl.and_then(|t| t.line).unwrap_or(360);
    let first_line = tpl.and_then(|t| t.first_line_chars).unwrap_or(200);
    // 标题字体: 无模板时微软雅黑；有模板时跟随模板正文（模板标题继承 Normal）。
    let (h_ascii, h_hansi, h_east) = if tpl.is_some() {
        (ascii.as_str(), hansi.as_str(), east.as_str())
    } else {
        ("Microsoft YaHei", "Microsoft YaHei", "微软雅黑")
    };
    let heading_sizes = [36, 32, 28, 26, 24, 22]; // 18/16/14/13/12/11 pt
    let mut headings = String::new();
    for (i, default_sz) in heading_sizes.iter().enumerate() {
        let level = i + 1;
        let sz = tpl.and_then(|t| t.heading_sz[i]).unwrap_or(*default_sz);
        headings.push_str(&format!(
            "<w:style w:type=\"paragraph\" w:styleId=\"Heading{level}\"><w:name w:val=\"heading {level}\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:qFormat/><w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before=\"240\" w:after=\"120\" w:line=\"360\" w:lineRule=\"auto\"/><w:ind w:firstLine=\"0\" w:firstLineChars=\"0\"/><w:outlineLvl w:val=\"{i}\"/></w:pPr><w:rPr><w:rFonts w:ascii=\"{h_ascii}\" w:hAnsi=\"{h_hansi}\" w:eastAsia=\"{h_east}\"/><w:b/><w:bCs/><w:color w:val=\"000000\"/><w:sz w:val=\"{sz}\"/><w:szCs w:val=\"{sz}\"/></w:rPr></w:style>"
        ));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii=\"{ascii}\" w:hAnsi=\"{hansi}\" w:eastAsia=\"{east}\"/><w:sz w:val=\"{sz}\"/><w:szCs w:val=\"{sz}\"/><w:lang w:val=\"en-US\" w:eastAsia=\"zh-CN\"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:line=\"{line}\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults>\
<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:qFormat/><w:pPr><w:spacing w:line=\"{line}\" w:lineRule=\"auto\"/><w:ind w:firstLineChars=\"{first_line}\"/></w:pPr><w:rPr><w:rFonts w:ascii=\"{ascii}\" w:hAnsi=\"{hansi}\" w:eastAsia=\"{east}\"/><w:sz w:val=\"{sz}\"/><w:szCs w:val=\"{sz}\"/></w:rPr></w:style>\
{headings}\
</w:styles>"
    )
}

fn numbering_xml(standardize: bool, extra_nums: &str) -> String {
    let bullet_chars = ["•", "◦", "▪"];
    let mut bullet_levels = String::new();
    let mut decimal_levels = String::new();
    let ordered_fmt = if standardize { "%1)" } else { "%1." };
    for i in 0..3 {
        let left = 480 * (i + 1);
        bullet_levels.push_str(&format!(
            "<w:lvl w:ilvl=\"{i}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/><w:lvlText w:val=\"{}\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"{left}\" w:hanging=\"240\"/></w:pPr></w:lvl>",
            bullet_chars[i as usize]
        ));
        decimal_levels.push_str(&format!(
            "<w:lvl w:ilvl=\"{i}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"decimal\"/><w:lvlText w:val=\"{ordered_fmt}\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"{left}\" w:hanging=\"240\"/></w:pPr></w:lvl>"
        ));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:numbering xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:abstractNum w:abstractNumId=\"0\">{bullet_levels}</w:abstractNum>\
<w:abstractNum w:abstractNumId=\"1\">{decimal_levels}</w:abstractNum>\
<w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num>\
{extra_nums}\
</w:numbering>"
    )
}

fn content_types_xml(image_exts: &[String]) -> String {
    let mut defaults = String::from(
        "<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/>",
    );
    for ext in ["png", "jpg", "jpeg", "gif", "bmp"] {
        if image_exts.iter().any(|e| e == ext) {
            let mime = match ext {
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "gif" => "image/gif",
                _ => "image/bmp",
            };
            defaults.push_str(&format!(
                "<Default Extension=\"{ext}\" ContentType=\"{mime}\"/>"
            ));
        }
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">{defaults}\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
<Override PartName=\"/word/numbering.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml\"/>\
</Types>"
    )
}

const ROOT_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
</Relationships>";

// ---------------------------------------------------------------------------
// Main entry
// ---------------------------------------------------------------------------

pub fn export_docx(
    markdown: &str,
    base_dir: Option<&Path>,
    opts: &ExportOptions,
) -> Result<Vec<u8>, String> {
    let body_md = split_front_matter(markdown);
    let parser = Parser::new_ext(body_md, Options::all());
    let mut ex = Exporter::new(opts, base_dir);
    let mut table_builder: Option<TableBuilder> = None;
    let mut previous_block_was_table = false;

    for event in parser {
        if table_builder.is_none() {
            if let Event::Start(Tag::Table(alignments)) = &event {
                ex.close_para();
                // Word joins adjacent OOXML tables unless a paragraph separates
                // them, even when the Markdown source contains a blank line.
                if previous_block_was_table {
                    ex.body.push_str("<w:p/>");
                }
                table_builder = Some(TableBuilder::new(alignments.clone()));
                previous_block_was_table = false;
                continue;
            }
        }
        if let Some(builder) = table_builder.as_mut() {
            let finished = builder.handle_event(event, &mut ex);
            if finished {
                if let Some(builder) = table_builder.take() {
                    ex.render_table(builder.finish());
                    previous_block_was_table = true;
                }
            }
            continue;
        }

        previous_block_was_table = false;
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    if !ex.para_open {
                        let ppr = if !ex.list_stack.is_empty() {
                            ex.list_item_ppr()
                        } else {
                            ex.block_ppr()
                        };
                        ex.open_para(&ppr);
                    }
                }
                Tag::Heading { level, .. } => {
                    let lvl = level as u8;
                    ex.heading_level = Some(lvl);
                    ex.heading_text_done = false;
                    ex.open_para(&Exporter::heading_ppr(lvl));
                }
                Tag::BlockQuote(_) => {
                    ex.quote_depth += 1;
                }
                Tag::CodeBlock(_) => {
                    ex.in_code_block = true;
                    ex.open_para(Exporter::code_block_ppr());
                }
                Tag::List(start) => ex.start_list(start),
                Tag::Item => {
                    ex.close_para();
                    ex.open_para(&ex.list_item_ppr());
                }
                Tag::Emphasis => ex.italic_depth = ex.italic_depth.saturating_add(1),
                Tag::Strong => ex.bold_depth = ex.bold_depth.saturating_add(1),
                Tag::Strikethrough => ex.strike = true,
                Tag::Link { dest_url, .. } => ex.start_hyperlink(&dest_url),
                Tag::Image {
                    dest_url, title, ..
                } => {
                    ex.in_image = Some((
                        dest_url.to_string(),
                        String::new(),
                        parse_logical_image_size(&title),
                    ));
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => ex.close_para(),
                TagEnd::Heading(_) => {
                    ex.close_para();
                    ex.heading_level = None;
                }
                TagEnd::BlockQuote(_) => {
                    ex.close_para();
                    ex.quote_depth = ex.quote_depth.saturating_sub(1);
                }
                TagEnd::CodeBlock => {
                    ex.close_para();
                    ex.in_code_block = false;
                }
                TagEnd::List(_) => {
                    ex.close_para();
                    ex.list_stack.pop();
                }
                TagEnd::Item => ex.close_para(),
                TagEnd::Emphasis => ex.italic_depth = ex.italic_depth.saturating_sub(1),
                TagEnd::Strong => ex.bold_depth = ex.bold_depth.saturating_sub(1),
                TagEnd::Strikethrough => ex.strike = false,
                TagEnd::Link => {
                    if ex.link_color_open {
                        ex.body.push_str("</w:hyperlink>");
                        ex.link_color_open = false;
                    }
                }
                TagEnd::Image => ex.emit_image(),
                _ => {}
            },
            Event::Text(text) => {
                if let Some((_, alt, _)) = ex.in_image.as_mut() {
                    alt.push_str(&text);
                } else {
                    if !ex.para_open && !ex.list_stack.is_empty() {
                        // Tight list item: no Paragraph event wraps the text.
                        ex.open_para(&ex.list_item_ppr());
                    }
                    ex.emit_text(&text);
                }
            }
            Event::Code(code) => {
                if !ex.para_open {
                    ex.open_para(&ex.block_ppr());
                }
                ex.inline_code = true;
                ex.emit_text(&code);
                ex.inline_code = false;
            }
            Event::InlineMath(math) => {
                ex.emit_text(&format!("${math}$"));
            }
            Event::DisplayMath(math) => {
                ex.open_para(
                    "<w:jc w:val=\"center\"/><w:ind w:firstLine=\"0\" w:firstLineChars=\"0\"/>",
                );
                ex.emit_text(&format!("$${math}$$"));
                ex.close_para();
            }
            Event::SoftBreak => {
                if !ex.in_code_block {
                    ex.emit_text(" ");
                }
            }
            Event::HardBreak => {
                if ex.para_open {
                    ex.body.push_str("<w:r><w:br/></w:r>");
                    ex.para_has_non_image_content = true;
                }
            }
            Event::Rule => {
                ex.close_para();
                ex.body.push_str("<w:p><w:pPr><w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"C0C4C8\"/></w:pBdr><w:ind w:firstLine=\"0\" w:firstLineChars=\"0\"/></w:pPr></w:p>");
            }
            Event::TaskListMarker(checked) => {
                if !ex.para_open {
                    ex.open_para(&ex.list_item_ppr());
                }
                ex.emit_text(if checked { "☑ " } else { "☐ " });
            }
            Event::Html(_) | Event::InlineHtml(_) => {}
            Event::FootnoteReference(name) => {
                ex.emit_text(&format!("[^{name}]"));
            }
        }
    }
    if let Some(builder) = table_builder.take() {
        ex.render_table(builder.finish());
    }
    ex.close_para();

    // Assemble the package.
    let (pg_w, pg_h, pg_top, pg_right, pg_bottom, pg_left) = opts
        .template
        .as_ref()
        .and_then(|t| t.page)
        .unwrap_or((11906, 16838, 1440, 1440, 1440, 1440));
    let document_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
<w:body>{}<w:sectPr><w:pgSz w:w=\"{pg_w}\" w:h=\"{pg_h}\"/><w:pgMar w:top=\"{pg_top}\" w:right=\"{pg_right}\" w:bottom=\"{pg_bottom}\" w:left=\"{pg_left}\" w:header=\"851\" w:footer=\"992\" w:gutter=\"0\"/></w:sectPr></w:body></w:document>",
        ex.body
    );

    let mut doc_rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering\" Target=\"numbering.xml\"/>",
    );
    for image in &ex.media {
        doc_rels.push_str(&format!(
            "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/{}\"/>",
            image.rid, image.file_name
        ));
    }
    doc_rels.push_str(&ex.hyperlink_rels);
    doc_rels.push_str("</Relationships>");

    let image_exts: Vec<String> = ex
        .media
        .iter()
        .map(|m| m.file_name.rsplit('.').next().unwrap_or("png").to_string())
        .collect();

    let mut entries: Vec<(String, Vec<u8>)> = vec![
        (
            "[Content_Types].xml".to_string(),
            content_types_xml(&image_exts).into_bytes(),
        ),
        ("_rels/.rels".to_string(), ROOT_RELS.as_bytes().to_vec()),
        ("word/document.xml".to_string(), document_xml.into_bytes()),
        (
            "word/_rels/document.xml.rels".to_string(),
            doc_rels.into_bytes(),
        ),
        (
            "word/styles.xml".to_string(),
            styles_xml(opts.standardize, opts.template.as_ref()).into_bytes(),
        ),
        (
            "word/numbering.xml".to_string(),
            numbering_xml(opts.standardize, &ex.numbering).into_bytes(),
        ),
    ];
    for image in ex.media {
        entries.push((format!("word/media/{}", image.file_name), image.bytes));
    }

    Ok(zip_stored(&entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_names(docx: &[u8]) -> Vec<String> {
        let mut names = Vec::new();
        let mut pos = 0;
        while pos + 30 < docx.len() {
            if &docx[pos..pos + 4] != b"PK\x03\x04" {
                break;
            }
            let name_len = u16::from_le_bytes([docx[pos + 26], docx[pos + 27]]) as usize;
            let extra_len = u16::from_le_bytes([docx[pos + 28], docx[pos + 29]]) as usize;
            let size = u32::from_le_bytes([
                docx[pos + 18],
                docx[pos + 19],
                docx[pos + 20],
                docx[pos + 21],
            ]) as usize;
            names.push(String::from_utf8_lossy(&docx[pos + 30..pos + 30 + name_len]).to_string());
            pos += 30 + name_len + extra_len + size;
        }
        names
    }

    fn document_xml(docx: &[u8]) -> String {
        zip_part(docx, "word/document.xml")
    }

    fn first_table_xml(document: &str) -> &str {
        let start = document.find("<w:tbl>").expect("table start");
        let end = document[start..].find("</w:tbl>").expect("table end") + start + "</w:tbl>".len();
        &document[start..end]
    }

    fn first_table_width(table: &str) -> u32 {
        let marker = "<w:tblW w:w=\"";
        let start = table.find(marker).expect("table width") + marker.len();
        let end = table[start..].find('"').expect("table width end") + start;
        table[start..end].parse().expect("numeric table width")
    }

    fn table_grid_widths(table: &str) -> Vec<u32> {
        let start = table.find("<w:tblGrid>").expect("table grid") + "<w:tblGrid>".len();
        let end = table[start..].find("</w:tblGrid>").expect("table grid end") + start;
        let grid = &table[start..end];
        let marker = "<w:gridCol w:w=\"";
        grid.match_indices(marker)
            .map(|(position, _)| {
                let value_start = position + marker.len();
                let value_end =
                    grid[value_start..].find('"').expect("grid width end") + value_start;
                grid[value_start..value_end]
                    .parse()
                    .expect("numeric grid width")
            })
            .collect()
    }

    fn cell_containing<'a>(table: &'a str, text: &str) -> &'a str {
        let marker = format!(">{text}<");
        let text_position = table.find(&marker).expect("cell text");
        let start = table[..text_position].rfind("<w:tc>").expect("cell start");
        let end = table[text_position..].find("</w:tc>").expect("cell end")
            + text_position
            + "</w:tc>".len();
        &table[start..end]
    }

    fn assert_visible_table_borders(table: &str) {
        for edge in ["top", "left", "bottom", "right", "insideH", "insideV"] {
            let expected =
                format!("<w:{edge} w:val=\"single\" w:sz=\"6\" w:space=\"0\" w:color=\"808080\"/>");
            assert!(
                table.contains(&expected),
                "missing visible {edge} table border: {expected}"
            );
        }
        assert!(!table.contains("w:val=\"nil\""));
        assert!(!table.contains("w:val=\"none\""));
    }

    fn run_containing<'a>(document: &'a str, text: &str) -> &'a str {
        let text_position = document.find(text).expect("run text");
        let start = document[..text_position].rfind("<w:r>").expect("run start");
        let end = document[text_position..].find("</w:r>").expect("run end")
            + text_position
            + "</w:r>".len();
        &document[start..end]
    }

    fn simple_table_markdown(column_count: usize) -> String {
        let headers = (1..=column_count)
            .map(|column| format!("H{column}"))
            .collect::<Vec<_>>()
            .join(" | ");
        let separators = std::iter::repeat("---")
            .take(column_count)
            .collect::<Vec<_>>()
            .join(" | ");
        let values = (1..=column_count)
            .map(|column| format!("V{column}"))
            .collect::<Vec<_>>()
            .join(" | ");
        format!("| {headers} |\n| {separators} |\n| {values} |\n")
    }

    #[test]
    fn data_url_image_is_embedded_as_media() {
        // 2x1 red PNG, base64-encoded.
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAIAAAB7QOjdAAAADUlEQVR4nGP4z8AARAAI/gH/xp559wAAAABJRU5ErkJggg==";
        let md = format!("前\n\n![流程图](data:image/png;base64,{png})\n\n后\n");
        let bytes = export_docx(&md, None, &ExportOptions::default()).unwrap();
        let names = entry_names(&bytes);
        assert!(
            names.iter().any(|n| n == "word/media/image1.png"),
            "embedded image media entry: {names:?}"
        );
        let doc = document_xml(&bytes);
        assert!(doc.contains("<w:drawing>"));
        assert!(doc.contains("<w:jc w:val=\"center\"/>"));
        let rels = zip_part(&bytes, "word/_rels/document.xml.rels");
        assert!(rels.contains("relationships/image"));
    }

    #[test]
    fn inline_image_stays_inside_the_surrounding_paragraph() {
        // 2x1 red PNG, base64-encoded.
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAIAAAB7QOjdAAAADUlEQVR4nGP4z8AARAAI/gH/xp559wAAAABJRU5ErkJggg==";
        let markdown = format!("前![流程图](data:image/png;base64,{png})后\n");
        let bytes = export_docx(&markdown, None, &ExportOptions::default()).unwrap();
        let document = document_xml(&bytes);

        assert_eq!(document.matches("<w:p>").count(), 1);
        let paragraph_start = document.find("<w:p>").expect("paragraph start");
        let paragraph_end = document[paragraph_start..]
            .find("</w:p>")
            .map(|offset| paragraph_start + offset + "</w:p>".len())
            .expect("paragraph end");
        let paragraph = &document[paragraph_start..paragraph_end];
        let before = paragraph.find(">前<").expect("text before image");
        let drawing = paragraph.find("<w:drawing>").expect("inline drawing");
        let after = paragraph.find(">后<").expect("text after image");

        assert!(before < drawing && drawing < after);
        assert!(!paragraph.contains("<w:jc w:val=\"center\"/>"));
        assert!(!document.contains("</w:p><w:r>"));
    }

    #[test]
    fn image_and_hyperlink_relationship_ids_do_not_collide() {
        // The previous split allocation used rId101+ for images and rId200+
        // for hyperlinks, so image 100 collided with the first hyperlink.
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAIAAAB7QOjdAAAADUlEQVR4nGP4z8AARAAI/gH/xp559wAAAABJRU5ErkJggg==";
        let mut markdown = String::new();
        for index in 0..100 {
            markdown.push_str(&format!("![图{index}](data:image/png;base64,{png})\n\n"));
        }
        markdown.push_str("[官网](https://example.com)\n");

        let bytes = export_docx(&markdown, None, &ExportOptions::default()).unwrap();
        let relationships = zip_part(&bytes, "word/_rels/document.xml.rels");
        let marker = "<Relationship Id=\"";
        let mut ids = Vec::new();
        let mut rest = relationships.as_str();
        while let Some(start) = rest.find(marker) {
            rest = &rest[start + marker.len()..];
            let end = rest.find('"').expect("relationship id end");
            ids.push(rest[..end].to_string());
            rest = &rest[end + 1..];
        }
        let unique_ids: std::collections::HashSet<_> = ids.iter().collect();

        assert_eq!(relationships.matches("relationships/image").count(), 100);
        assert_eq!(relationships.matches("relationships/hyperlink").count(), 1);
        assert_eq!(
            ids.len(),
            unique_ids.len(),
            "duplicate IDs in {relationships}"
        );
    }

    #[test]
    fn table_images_embed_data_and_local_pngs_without_distorting_column_widths() {
        // 2x1 red PNG, base64-encoded.
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAIAAAB7QOjdAAAADUlEQVR4nGP4z8AARAAI/gH/xp559wAAAABJRU5ErkJggg==";
        let temp_dir = std::env::temp_dir().join(format!(
            "mdpreview-docx-table-images-{}",
            std::process::id()
        ));
        let image_path = temp_dir.join("tiny.png");
        std::fs::create_dir_all(&temp_dir).unwrap();
        std::fs::write(&image_path, docx_base64_decode(png).unwrap()).unwrap();

        let markdown = format!(
            "| 预览 | 说明 |\n|---|---|\n| data<br>![数据图](data:image/png;base64,{png}) | 这是用于验证列宽估算的一段较长说明文字 |\n| local<br>![本地图](tiny.png) | 另一段用于验证列宽估算的较长说明文字 |\n"
        );
        let result = export_docx(&markdown, Some(&temp_dir), &ExportOptions::default());
        let _ = std::fs::remove_file(&image_path);
        let _ = std::fs::remove_dir(&temp_dir);
        let bytes = result.unwrap();

        let names = entry_names(&bytes);
        assert!(names.iter().any(|name| name == "word/media/image1.png"));
        assert!(names.iter().any(|name| name == "word/media/image2.png"));
        let document = document_xml(&bytes);
        let table = first_table_xml(&document);
        assert!(cell_containing(table, "data").contains("<w:drawing>"));
        assert!(cell_containing(table, "local").contains("<w:drawing>"));
        assert_eq!(table.matches("<w:drawing>").count(), 2);
        let relationships = zip_part(&bytes, "word/_rels/document.xml.rels");
        assert_eq!(relationships.matches("relationships/image").count(), 2);
        assert!(relationships.contains("Target=\"media/image1.png\""));
        assert!(relationships.contains("Target=\"media/image2.png\""));
        let widths = table_grid_widths(table);
        assert!(
            widths[0] < widths[1],
            "image URLs must not inflate text-based column weights: {widths:?}"
        );
    }

    #[test]
    fn mermaid_fallback_uses_logical_size_not_png_sample_size() {
        // The PNG is 2x1 pixels, but the Mermaid SVG logical size is 100x50.
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAIAAAB7QOjdAAAADUlEQVR4nGP4z8AARAAI/gH/xp559wAAAABJRU5ErkJggg==";
        let md = format!("![流程图](data:image/png;base64,{png} \"mdp-logical-size=100x50\")\n");
        let bytes = export_docx(&md, None, &ExportOptions::default()).unwrap();
        let doc = document_xml(&bytes);

        assert!(doc.contains("<wp:extent cx=\"952500\" cy=\"476250\"/>"));
        assert!(!doc.contains("<wp:extent cx=\"19050\" cy=\"9525\"/>"));
    }

    #[test]
    fn logical_image_size_metadata_is_strictly_bounded() {
        assert_eq!(
            parse_logical_image_size("mdp-logical-size=1000x500"),
            Some((1000, 500))
        );
        assert_eq!(parse_logical_image_size("caption"), None);
        assert_eq!(parse_logical_image_size("mdp-logical-size=0x500"), None);
        assert_eq!(parse_logical_image_size("mdp-logical-size=99999x500"), None);
    }

    #[test]
    fn heading_number_prefixes_are_stripped() {
        assert_eq!(strip_heading_number("1. 概述"), "概述");
        assert_eq!(strip_heading_number("1.1 安装"), "安装");
        assert_eq!(strip_heading_number("1.1.1 细节"), "细节");
        assert_eq!(strip_heading_number("2、 计划"), "计划");
        assert_eq!(strip_heading_number("一、背景"), "背景");
        assert_eq!(strip_heading_number("（一） 目标"), "目标");
        assert_eq!(strip_heading_number("(1) 步骤"), "步骤");
        assert_eq!(strip_heading_number("第一章 总则"), "总则");
        assert_eq!(strip_heading_number("① 要点"), "要点");
        // Not a number prefix: kept as-is.
        assert_eq!(strip_heading_number("版本 1.1 说明"), "版本 1.1 说明");
    }

    #[test]
    fn xml_escape_filters_invalid_xml_1_0_controls_and_preserves_whitespace() {
        assert_eq!(
            xml_escape("前\u{1}\t中\n后\r<&>\""),
            "前\t中\n后\r&lt;&amp;&gt;&quot;"
        );
        assert_eq!(
            xml_escape("\u{0}\u{B}\u{1F}\u{FFFE}\u{FFFF}\u{FFFD}\u{10000}"),
            "\u{FFFD}\u{10000}"
        );

        let markdown = "正文\u{1}保留\n\n| 值 |\n|---|\n| 表\u{1}格 |\n";
        let bytes = export_docx(markdown, None, &ExportOptions::default()).unwrap();
        let document = document_xml(&bytes);
        assert!(!document.contains('\u{1}'));
        assert!(document.contains("正文保留"));
        assert!(document.contains("表格"));
    }

    #[test]
    fn export_produces_valid_docx_structure() {
        let md = "# 标题\n\n正文段落 **加粗** `code`。\n\n- 甲\n- 乙\n\n| A | B |\n|---|---|\n| 1 | 2 |\n";
        let bytes = export_docx(md, None, &ExportOptions::default()).unwrap();
        let names = entry_names(&bytes);
        assert!(names.contains(&"[Content_Types].xml".to_string()));
        assert!(names.contains(&"word/document.xml".to_string()));
        assert!(names.contains(&"word/styles.xml".to_string()));
        assert!(names.contains(&"word/numbering.xml".to_string()));

        let doc = document_xml(&bytes);
        assert!(doc.contains(r#"<w:pStyle w:val="Heading1"/>"#));
        assert!(doc.contains("<w:b/>"));
        assert!(doc.contains("<w:tbl>"));
        assert!(doc.contains("<w:numPr>"));
        assert!(doc.contains(r#"<w:shd w:val="clear" w:color="auto" w:fill="F2F2F2"/>"#));
        assert!(doc.contains(r#"w:fill="EAF2F8""#));
    }

    #[test]
    fn table_layout_is_fixed_and_bounded_for_multiple_column_counts() {
        for column_count in [1usize, 2, 3, 4, 6, 7, 10] {
            let markdown = format!("正文样式不变。\n\n{}", simple_table_markdown(column_count));
            let bytes = export_docx(&markdown, None, &ExportOptions::default()).unwrap();
            let document = document_xml(&bytes);
            let table = first_table_xml(&document);
            let expected_width = match column_count {
                1 | 2 => 4_513,
                3 | 4 => 6_319,
                _ => DEFAULT_CONTENT_WIDTH_TWIPS,
            };
            let grid_widths = table_grid_widths(table);

            assert!(table.contains("<w:tblLayout w:type=\"fixed\"/>"));
            assert!(!table.contains("w:type=\"autofit\""));
            assert!(!table.contains("w:type=\"auto\""));
            assert_eq!(first_table_width(table), expected_width);
            assert_eq!(grid_widths.len(), column_count);
            assert_eq!(grid_widths.iter().sum::<u32>(), expected_width);
            assert!(
                grid_widths
                    .iter()
                    .all(|width| *width >= TABLE_MIN_COLUMN_WIDTH_TWIPS),
                "{column_count} columns should keep the minimum width: {grid_widths:?}"
            );
            assert_eq!(table.matches("<w:tcW ").count(), column_count * 2);
            assert!(table.contains("w:type=\"dxa\""));
            if column_count >= 7 {
                assert!(
                    table.contains("<w:sz w:val=\"18\"/><w:szCs w:val=\"18\"/>"),
                    "wide tables use a local compact font"
                );
                let table_start = document.find("<w:tbl>").expect("table start");
                assert!(
                    !document[..table_start].contains("<w:sz w:val=\"18\"/>"),
                    "compact font must not leak into body paragraphs"
                );
            }
        }
    }

    #[test]
    fn table_content_weights_make_short_columns_narrower() {
        assert_eq!(visual_text_width("ASCII"), 5);
        assert_eq!(visual_text_width("中文Ａ"), 6);

        let markdown = "| 标签 | 详细说明 |\n|---|---|\n| 短 | https://example.com/a/very/long/path/that/forces/full/width |\n";
        let bytes = export_docx(markdown, None, &ExportOptions::default()).unwrap();
        let document = document_xml(&bytes);
        let table = first_table_xml(&document);
        let widths = table_grid_widths(table);

        assert_eq!(first_table_width(table), DEFAULT_CONTENT_WIDTH_TWIPS);
        assert_eq!(widths.iter().sum::<u32>(), DEFAULT_CONTENT_WIDTH_TWIPS);
        assert!(
            widths[0] < widths[1],
            "short labels should be narrower than long descriptions: {widths:?}"
        );
    }

    #[test]
    fn table_alignment_keeps_markdown_and_infers_numeric_columns() {
        let markdown = "| 显式左 | 显式中 | 显式右 | 自动数字 | 混合 |\n|:---|:---:|---:|---|---|\n| 123 | center-a | right-a | 1,200.50 | A1 |\n| 456 | center-b | right-b | 50% | 2 |\n|  | center-c | right-c |  | 3 |\n| 789 | center-d | right-d | ￥30.00 | 4 |\n";
        let bytes = export_docx(markdown, None, &ExportOptions::default()).unwrap();
        let document = document_xml(&bytes);
        let table = first_table_xml(&document);

        assert!(cell_containing(table, "123").contains("<w:jc w:val=\"left\"/>"));
        assert!(cell_containing(table, "center-a").contains("<w:jc w:val=\"center\"/>"));
        assert!(cell_containing(table, "right-a").contains("<w:jc w:val=\"right\"/>"));
        assert!(cell_containing(table, "1,200.50").contains("<w:jc w:val=\"right\"/>"));
        assert!(cell_containing(table, "50%").contains("<w:jc w:val=\"right\"/>"));
        assert!(cell_containing(table, "￥30.00").contains("<w:jc w:val=\"right\"/>"));
        assert!(cell_containing(table, "A1").contains("<w:jc w:val=\"left\"/>"));
        assert_eq!(
            table.matches("<w:tc>").count(),
            25,
            "empty cells are retained"
        );
    }

    #[test]
    fn table_preserves_links_inline_code_and_html_break_variants() {
        let markdown =
            "| 内容 |\n|---|\n| [官网](https://example.com)<br>`code`<br/>第二行<br />第三行 |\n";
        let bytes = export_docx(markdown, None, &ExportOptions::default()).unwrap();
        let document = document_xml(&bytes);
        let table = first_table_xml(&document);
        let relationships = zip_part(&bytes, "word/_rels/document.xml.rels");

        assert_eq!(table.matches("<w:br/>").count(), 3);
        assert!(table.contains("<w:hyperlink r:id=\"rId3\" w:history=\"1\">"));
        assert!(relationships.contains("Target=\"https://example.com\""));
        assert!(
            table.contains("Microsoft YaHei"),
            "inline code font is retained"
        );
        assert!(table.contains("w:fill=\"F2F2F2\""));
        assert_eq!(first_table_width(table), DEFAULT_CONTENT_WIDTH_TWIPS);
        assert!(table.contains("<w:vAlign w:val=\"top\"/>"));
        assert!(table.contains("<w:vAlign w:val=\"center\"/>"));
    }

    #[test]
    fn nested_strong_and_emphasis_keep_outer_style_after_inner_end() {
        let markdown = "**body-outer __body-inner__ body-bold-tail** body-plain-tail\n\n*italic-outer _italic-inner_ italic-tail* italic-plain-tail\n\n| 内容 |\n|---|\n| **table-outer __table-inner__ table-bold-tail** table-plain-tail / *table-italic-outer _table-italic-inner_ table-italic-tail* table-italic-plain-tail |\n";
        let bytes = export_docx(markdown, None, &ExportOptions::default()).unwrap();
        let document = document_xml(&bytes);

        assert!(run_containing(&document, " body-bold-tail").contains("<w:b/>"));
        assert!(run_containing(&document, " italic-tail").contains("<w:i/>"));
        assert!(run_containing(&document, " table-bold-tail").contains("<w:b/>"));
        assert!(run_containing(&document, " table-italic-tail").contains("<w:i/>"));
        assert!(!run_containing(&document, " body-plain-tail").contains("<w:b/>"));
        assert!(!run_containing(&document, " italic-plain-tail").contains("<w:i/>"));
        assert!(!run_containing(&document, " table-plain-tail").contains("<w:b/>"));
        assert!(!run_containing(&document, " table-italic-plain-tail").contains("<w:i/>"));
    }

    #[test]
    fn table_hard_break_event_is_buffered_as_a_word_break() {
        let opts = ExportOptions::default();
        let mut exporter = Exporter::new(&opts, None);
        let mut builder = TableBuilder::new(vec![Alignment::None]);
        builder.start_row(true);
        builder.current_cell = Some(TableCell::default());
        builder.push_text("表头", false);
        builder.finish_row();
        builder.start_row(false);
        builder.current_cell = Some(TableCell::default());
        builder.push_text("第一行", false);
        assert!(!builder.handle_event(Event::HardBreak, &mut exporter));
        builder.push_text("第二行", false);
        builder.finish_row();
        exporter.render_table(builder.finish());

        assert_eq!(exporter.body.matches("<w:br/>").count(), 1);
        assert!(exporter.body.contains("第一行"));
        assert!(exporter.body.contains("第二行"));
        assert_eq!(
            first_table_width(&exporter.body),
            DEFAULT_CONTENT_WIDTH_TWIPS
        );
    }

    #[test]
    fn table_style_has_margins_header_rules_and_standardize_compatibility() {
        let markdown = "| 表头 | 数量 |\n|---|---|\n| 内容 | 1 |\n";
        let default_bytes = export_docx(markdown, None, &ExportOptions::default()).unwrap();
        let default_document = document_xml(&default_bytes);
        let default_table = first_table_xml(&default_document);

        assert_visible_table_borders(default_table);
        assert!(default_table.contains("<w:top w:w=\"80\" w:type=\"dxa\"/>"));
        assert!(default_table.contains("<w:left w:w=\"120\" w:type=\"dxa\"/>"));
        assert!(default_table.contains("<w:bottom w:w=\"80\" w:type=\"dxa\"/>"));
        assert!(default_table.contains("<w:right w:w=\"120\" w:type=\"dxa\"/>"));
        assert!(default_table.contains(
            "<w:spacing w:before=\"0\" w:after=\"0\" w:line=\"276\" w:lineRule=\"auto\"/>"
        ));
        assert!(default_table.contains("w:fill=\"EAF2F8\""));
        assert!(cell_containing(default_table, "表头").contains("<w:b/><w:bCs/>"));
        assert!(default_table.contains("<w:cantSplit/><w:tblHeader/>"));
        assert_eq!(default_table.matches("<w:keepNext/>").count(), 2);
        assert!(cell_containing(default_table, "表头").contains("<w:keepNext/>"));
        assert!(!cell_containing(default_table, "内容").contains("<w:keepNext/>"));

        let standardized_bytes = export_docx(
            markdown,
            None,
            &ExportOptions {
                standardize: true,
                strip_heading_numbers: false,
                template: None,
            },
        )
        .unwrap();
        let standardized_document = document_xml(&standardized_bytes);
        let standardized_table = first_table_xml(&standardized_document);
        assert!(!standardized_table.contains("EAF2F8"));
        assert!(!standardized_table.contains("<w:b/>"));
        assert_visible_table_borders(standardized_table);
        assert_eq!(standardized_table.matches("<w:keepNext/>").count(), 2);
        assert!(standardized_table
            .contains("<w:bottom w:val=\"single\" w:sz=\"8\" w:space=\"0\" w:color=\"808080\"/>"));
    }

    #[test]
    fn table_header_repeats_and_only_header_is_marked_non_splitting() {
        let mut markdown = String::from("| 序号 | 内容 |\n|---|---|\n");
        for row in 1..=100 {
            markdown.push_str(&format!("| {row} | 第{row}行内容 |\n"));
        }
        let bytes = export_docx(&markdown, None, &ExportOptions::default()).unwrap();
        let document = document_xml(&bytes);
        let table = first_table_xml(&document);

        assert_eq!(table.matches("<w:tr>").count(), 101);
        assert_eq!(table.matches("<w:tblHeader/>").count(), 1);
        assert_eq!(table.matches("<w:cantSplit/>").count(), 1);
        assert!(table.contains("第100行内容"));
    }

    #[test]
    fn adjacent_markdown_tables_stay_separate_in_word() {
        let markdown = "| A |\n|---|\n| 1 |\n\n| B | C |\n|---|---|\n| 2 | 3 |\n";
        let bytes = export_docx(markdown, None, &ExportOptions::default()).unwrap();
        let document = document_xml(&bytes);

        assert_eq!(document.matches("<w:tbl>").count(), 2);
        assert!(document.contains("</w:tbl><w:p/><w:tbl>"));
    }

    #[test]
    fn table_width_uses_custom_template_page_margins() {
        let template = TemplateStyle {
            page: Some((10_000, 16_000, 1_000, 2_000, 1_000, 2_000)),
            ..TemplateStyle::default()
        };
        let markdown = simple_table_markdown(6);
        let bytes = export_docx(
            &markdown,
            None,
            &ExportOptions {
                standardize: false,
                strip_heading_numbers: false,
                template: Some(template),
            },
        )
        .unwrap();
        let document = document_xml(&bytes);
        let table = first_table_xml(&document);
        let widths = table_grid_widths(table);

        assert_eq!(first_table_width(table), 6_000);
        assert_eq!(widths.iter().sum::<u32>(), 6_000);
        assert!(widths.iter().all(|width| *width >= 480));
        assert!(document.contains("w:right=\"2000\""));
        assert!(document.contains("w:left=\"2000\""));
    }

    #[test]
    fn standardize_strips_bold_and_spaces() {
        let md = "正文 段落　带 空格 和 **加粗**。\n\n含 `code` 与表:\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n```\nblock\n```\n";
        let bytes = export_docx(
            md,
            None,
            &ExportOptions {
                standardize: true,
                strip_heading_numbers: false,
                template: None,
            },
        )
        .unwrap();
        let doc = document_xml(&bytes);
        assert!(!doc.contains("<w:b/>"), "standardize strips body bold");
        assert!(!doc.contains("空 格"), "half-width spaces removed");
        assert!(doc.contains("空格"), "text joined without spaces");
        assert!(
            !doc.contains("F2F2F2"),
            "standardize strips inline-code and table-header shading"
        );
        assert!(
            doc.contains("F5F5F5"),
            "code block keeps its light gray background in both modes"
        );
    }

    #[test]
    fn strip_numbers_keeps_heading_levels() {
        let md = "# 1. 概述\n\n## 1.1 安装\n";
        let bytes = export_docx(
            md,
            None,
            &ExportOptions {
                standardize: false,
                strip_heading_numbers: true,
                template: None,
            },
        )
        .unwrap();
        let doc = document_xml(&bytes);
        assert!(doc.contains(r#"<w:pStyle w:val="Heading1"/>"#));
        assert!(doc.contains(r#"<w:pStyle w:val="Heading2"/>"#));
        assert!(doc.contains(">概述<"));
        assert!(doc.contains(">安装<"));
        assert!(!doc.contains("1.1"));
    }

    #[test]
    fn crc32_matches_known_vector() {
        // CRC-32 of "123456789" is 0xCBF43926.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    // Temporary verification helper: dump sample docx files to a directory
    // named by the DUMP_DOCX_DIR environment variable.
    #[test]
    fn dump_sample_docx_for_manual_validation() {
        let Ok(dir) = std::env::var("DUMP_DOCX_DIR") else {
            return;
        };
        let md = "# 1. 概述\n\n这是**正文**段落，含 `inline code` 与[链接](https://example.com)。\n\n## 1.1 安装\n\n### （一） 步骤\n\n- 项目甲\n- 项目乙\n\n1. 第一步\n2. 第二步\n\n> 引用块内容。\n\n```rust\nfn main() {}\n```\n\n| 名称 | 数量 |\n|---|---|\n| 甲 | 1 |\n| 乙 | 2 |\n\n含 全角　和 半角 空格 的句子。\n";
        let template = std::env::var("DUMP_DOCX_TEMPLATE")
            .ok()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| TemplateStyle::from_docx(&bytes));
        if template.is_some() {
            eprintln!("dump: template loaded");
        }
        for (name, opts) in [
            (
                "default.docx",
                ExportOptions {
                    standardize: false,
                    strip_heading_numbers: false,
                    template: None,
                },
            ),
            (
                "standardized.docx",
                ExportOptions {
                    standardize: true,
                    strip_heading_numbers: false,
                    template: None,
                },
            ),
            (
                "stripped.docx",
                ExportOptions {
                    standardize: false,
                    strip_heading_numbers: true,
                    template: None,
                },
            ),
            (
                "both.docx",
                ExportOptions {
                    standardize: true,
                    strip_heading_numbers: true,
                    template: None,
                },
            ),
            (
                "templated.docx",
                ExportOptions {
                    standardize: false,
                    strip_heading_numbers: false,
                    template: template.clone(),
                },
            ),
        ] {
            let bytes = export_docx(md, None, &opts).unwrap();
            std::fs::write(std::path::Path::new(&dir).join(name), bytes).unwrap();
        }

        let mut stress = String::from(
            "# DOCX 表格分页验收\n\n| 序号 | 标签 | 详细说明 | 金额 | 完成率 |\n|---:|:---|---|---:|---:|\n",
        );
        for row in 1..=100 {
            let detail = if row % 10 == 0 {
                format!(
                    "第{row}行包含较长中文说明、inline code `value_{row}` 以及换行<br>第二行内容"
                )
            } else {
                format!("第{row}行中文说明，用于检查固定列宽与跨页内容完整性")
            };
            stress.push_str(&format!(
                "| {row} | T{row} | {detail} | ￥{row}.00 | {row}% |\n"
            ));
        }
        stress.push_str(
            "\n| C1 | C2 | C3 | C4 | C5 | C6 | C7 | C8 | C9 | C10 |\n|---|---|---|---|---|---|---|---|---|---|\n| 一 | 二 | 三 | 四 | 五 | 六 | 七 | 八 | 九 | 十 |\n",
        );
        let stress_bytes = export_docx(&stress, None, &ExportOptions::default()).unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join("table-stress.docx"),
            stress_bytes,
        )
        .unwrap();
    }

    #[test]
    fn template_style_is_learned_from_docx_package() {
        let styles = "<?xml version=\"1.0\"?><w:styles>\
<w:style w:type=\"paragraph\" w:styleId=\"1\"><w:name w:val=\"Normal\"/><w:pPr><w:spacing w:line=\"360\" w:lineRule=\"auto\"/><w:ind w:firstLine=\"480\" w:firstLineChars=\"200\"/></w:pPr><w:rPr><w:rFonts w:ascii=\"Times New Roman\" w:hAnsi=\"Times New Roman\" w:eastAsia=\"宋体\"/><w:sz w:val=\"24\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"5\"><w:name w:val=\"heading 1\"/><w:rPr><w:b/><w:sz w:val=\"32\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"6\"><w:name w:val=\"heading 2\"/><w:rPr><w:sz w:val=\"30\"/></w:rPr></w:style>\
</w:styles>";
        let document = "<?xml version=\"1.0\"?><w:document><w:body><w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:top=\"1440\" w:right=\"1800\" w:bottom=\"1440\" w:left=\"1800\"/></w:sectPr></w:body></w:document>";
        let package = zip_stored(&[
            ("word/styles.xml".to_string(), styles.as_bytes().to_vec()),
            (
                "word/document.xml".to_string(),
                document.as_bytes().to_vec(),
            ),
        ]);

        let tpl = TemplateStyle::from_docx(&package).expect("template parses");
        assert_eq!(
            tpl.normal_fonts,
            Some((
                "Times New Roman".to_string(),
                "Times New Roman".to_string(),
                "宋体".to_string()
            ))
        );
        assert_eq!(tpl.normal_sz, Some(24));
        assert_eq!(tpl.line, Some(360));
        assert_eq!(tpl.first_line_chars, Some(200));
        assert_eq!(tpl.heading_sz[0], Some(32));
        assert_eq!(tpl.heading_sz[1], Some(30));
        assert_eq!(tpl.heading_sz[2], None);
        assert_eq!(tpl.page, Some((11906, 16838, 1440, 1800, 1440, 1800)));
        assert_eq!(tpl.content_width_emu(), (11906 - 1800 - 1800) as i64 * 635);

        // Export with the template: Normal uses 宋体 12pt, H1 16pt, margins applied.
        let bytes = export_docx(
            "# 标题\n\n正文。\n",
            None,
            &ExportOptions {
                standardize: false,
                strip_heading_numbers: false,
                template: Some(tpl),
            },
        )
        .unwrap();
        let styles_out = zip_part(&bytes, "word/styles.xml");
        assert!(styles_out.contains("w:eastAsia=\"宋体\""));
        assert!(styles_out.contains("w:sz w:val=\"24\""));
        assert!(styles_out.contains("w:sz w:val=\"32\"")); // H1 from template
        assert!(styles_out.contains("w:sz w:val=\"28\"")); // H3 falls back to default
        let doc = document_xml(&bytes);
        assert!(doc.contains("w:right=\"1800\""));
    }

    /// Extract one entry's text out of a stored-ZIP docx built in memory.
    fn zip_part(docx: &[u8], wanted: &str) -> String {
        let mut pos = 0;
        while pos + 30 < docx.len() {
            if &docx[pos..pos + 4] != b"PK\x03\x04" {
                break;
            }
            let name_len = u16::from_le_bytes([docx[pos + 26], docx[pos + 27]]) as usize;
            let extra_len = u16::from_le_bytes([docx[pos + 28], docx[pos + 29]]) as usize;
            let size = u32::from_le_bytes([
                docx[pos + 18],
                docx[pos + 19],
                docx[pos + 20],
                docx[pos + 21],
            ]) as usize;
            let name = String::from_utf8_lossy(&docx[pos + 30..pos + 30 + name_len]).to_string();
            let data_start = pos + 30 + name_len + extra_len;
            if name == wanted {
                return String::from_utf8_lossy(&docx[data_start..data_start + size]).to_string();
            }
            pos = data_start + size;
        }
        panic!("entry {wanted} not found");
    }
}
