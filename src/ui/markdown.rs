//! 轻量零依赖 Markdown 富文本渲染器（专用于更新弹窗发布说明）：
//! - 支持标题（`#`–`####`）、段落、无序/嵌套列表、有序列表、引用块、围栏代码块与分隔线
//! - 行内支持 **粗体**、*斜体*、`行内代码`、**`粗体代码`**、~~删除线~~、[链接](url) 与转义字符
//! - 完整支持标题与正文中的 Emoji 展示，并保留 `snake_case` 标识符中的下划线不误伤为斜体
//! - 通过 `MarkdownCache` 按原文缓存解析结果，60fps 滚动/悬停重绘零重复解析

use eframe::egui::{
    self, Align, Color32, CornerRadius, CursorIcon, FontFamily, FontId, Pos2, Rect, Sense, Stroke,
    StrokeKind, TextFormat, Vec2,
};

use crate::ui::theme::{bold_family, lerp_color, with_alpha, Palette, ThemeMode};

const BODY_FONT_SIZE: f32 = 12.0;
const BODY_LINE_HEIGHT: f32 = 19.5;
const CODE_FONT_SIZE: f32 = 11.2;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpanStyle {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strikethrough: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdSpan {
    pub text: String,
    pub style: SpanStyle,
    pub link_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MdBlock {
    Heading {
        level: u8,
        spans: Vec<MdSpan>,
    },
    Paragraph(Vec<MdSpan>),
    BulletItem {
        indent: u8,
        spans: Vec<MdSpan>,
    },
    OrderedItem {
        number: String,
        indent: u8,
        spans: Vec<MdSpan>,
    },
    Quote(Vec<MdSpan>),
    CodeBlock(String),
    Divider,
}

#[derive(Default)]
pub struct MarkdownCache {
    source: String,
    blocks: Vec<MdBlock>,
    #[cfg(test)]
    parse_count: usize,
}

impl MarkdownCache {
    pub fn blocks_for(&mut self, markdown: &str) -> &[MdBlock] {
        if self.source != markdown || (self.blocks.is_empty() && !markdown.trim().is_empty()) {
            self.source.clear();
            self.source.push_str(markdown);
            self.blocks = parse_markdown(markdown);
            #[cfg(test)]
            {
                self.parse_count += 1;
            }
        }
        &self.blocks
    }

    pub fn clear(&mut self) {
        if !self.source.is_empty() || !self.blocks.is_empty() {
            self.source = String::new();
            self.blocks = Vec::new();
        }
    }
}

/// 将 Markdown 文本解析为块级富文本结构。
pub fn parse_markdown(input: &str) -> Vec<MdBlock> {
    // 清理不可见的 Emoji 变体选择符 U+FE0F，避免在轮廓字库中渲染成额外空白或占位符
    let normalized = input
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\u{fe0f}', "");
    let lines: Vec<&str> = normalized.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let raw = lines[i];
        let trimmed = raw.trim();

        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        // 1. 围栏代码块（``` 或 ~~~）
        if let Some(fence) = fence_marker(trimmed) {
            i += 1;
            let mut code_lines = Vec::new();
            while i < lines.len() {
                if lines[i].trim_start().starts_with(fence) {
                    i += 1;
                    break;
                }
                code_lines.push(lines[i]);
                i += 1;
            }
            let code = code_lines.join("\n");
            if !code.trim().is_empty() {
                blocks.push(MdBlock::CodeBlock(code));
            }
            continue;
        }

        // 2. 分隔线（--- / *** / ___）
        if is_horizontal_rule(trimmed) {
            blocks.push(MdBlock::Divider);
            i += 1;
            continue;
        }

        // 3. ATX 标题（# .. ####）
        if let Some((level, content)) = parse_heading_line(trimmed) {
            let spans = parse_inline(content);
            if !spans.is_empty() {
                blocks.push(MdBlock::Heading { level, spans });
            }
            i += 1;
            continue;
        }

        // 4. 引用块（> ...）
        if let Some(first_quote) = strip_quote_prefix(trimmed) {
            let mut buf = first_quote.to_string();
            i += 1;
            while i < lines.len() {
                let next_trimmed = lines[i].trim();
                if next_trimmed.is_empty() {
                    break;
                }
                if let Some(rest) = strip_quote_prefix(next_trimmed) {
                    append_joined_line(&mut buf, rest);
                    i += 1;
                } else {
                    break;
                }
            }
            let spans = parse_inline(&buf);
            if !spans.is_empty() {
                blocks.push(MdBlock::Quote(spans));
            }
            continue;
        }

        // 5. 无序列表项（- / * / +，支持嵌套缩进与续行）
        if let Some((indent, first_text)) = parse_bullet_line(raw) {
            let mut buf = first_text.to_string();
            i += 1;
            while i < lines.len() {
                let next = lines[i];
                let next_t = next.trim();
                if next_t.is_empty()
                    || is_block_start(next)
                    || leading_indent_spaces(next) < (indent as usize * 2) + 2
                {
                    break;
                }
                append_joined_line(&mut buf, next_t);
                i += 1;
            }
            let spans = parse_inline(&buf);
            if !spans.is_empty() {
                blocks.push(MdBlock::BulletItem { indent, spans });
            }
            continue;
        }

        // 6. 有序列表项（1. / 2)）
        if let Some((indent, number, first_text)) = parse_ordered_line(raw) {
            let mut buf = first_text.to_string();
            i += 1;
            while i < lines.len() {
                let next = lines[i];
                let next_t = next.trim();
                if next_t.is_empty()
                    || is_block_start(next)
                    || leading_indent_spaces(next) < (indent as usize * 2) + 2
                {
                    break;
                }
                append_joined_line(&mut buf, next_t);
                i += 1;
            }
            let spans = parse_inline(&buf);
            if !spans.is_empty() {
                blocks.push(MdBlock::OrderedItem {
                    number,
                    indent,
                    spans,
                });
            }
            continue;
        }

        // 7. 普通段落（合并连续软换行）
        let mut buf = trimmed.to_string();
        i += 1;
        while i < lines.len() {
            let next = lines[i];
            let next_t = next.trim();
            if next_t.is_empty() || is_block_start(next) {
                break;
            }
            append_joined_line(&mut buf, next_t);
            i += 1;
        }
        let spans = parse_inline(&buf);
        if !spans.is_empty() {
            blocks.push(MdBlock::Paragraph(spans));
        }
    }

    blocks
}

fn is_block_start(line: &str) -> bool {
    let trimmed = line.trim();
    fence_marker(trimmed).is_some()
        || is_horizontal_rule(trimmed)
        || parse_heading_line(trimmed).is_some()
        || strip_quote_prefix(trimmed).is_some()
        || parse_bullet_line(line).is_some()
        || parse_ordered_line(line).is_some()
}

fn fence_marker(trimmed: &str) -> Option<&'static str> {
    if trimmed.starts_with("```") {
        Some("```")
    } else if trimmed.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

fn is_horizontal_rule(trimmed: &str) -> bool {
    let mut marker = None;
    let mut count = 0usize;
    for ch in trimmed.chars() {
        if ch == ' ' || ch == '\t' {
            continue;
        }
        if matches!(ch, '-' | '*' | '_') {
            match marker {
                None => {
                    marker = Some(ch);
                    count = 1;
                }
                Some(m) if m == ch => count += 1,
                Some(_) => return false,
            }
        } else {
            return false;
        }
    }
    count >= 3
}

fn parse_heading_line(trimmed: &str) -> Option<(u8, &str)> {
    let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let content = rest.trim().trim_end_matches('#').trim();
    if content.is_empty() {
        return None;
    }
    Some(((hashes as u8).min(4), content))
}

fn strip_quote_prefix(trimmed: &str) -> Option<&str> {
    let rest = trimmed.strip_prefix('>')?;
    Some(rest.strip_prefix(' ').unwrap_or(rest).trim())
}

fn leading_indent_spaces(line: &str) -> usize {
    let mut spaces = 0usize;
    for ch in line.chars() {
        match ch {
            ' ' => spaces += 1,
            '\t' => spaces += 4,
            _ => break,
        }
    }
    spaces
}

fn parse_bullet_line(line: &str) -> Option<(u8, &str)> {
    let spaces = leading_indent_spaces(line);
    let trimmed = line.trim_start_matches([' ', '\t']);
    let mut chars = trimmed.chars();
    let bullet = chars.next()?;
    if !matches!(bullet, '-' | '*' | '+') {
        return None;
    }
    let after = chars.as_str();
    if !after.starts_with([' ', '\t']) {
        return None;
    }
    let text = after.trim();
    if text.is_empty() {
        return None;
    }
    let indent = ((spaces / 2).min(2)) as u8;
    Some((indent, text))
}

fn parse_ordered_line(line: &str) -> Option<(u8, String, &str)> {
    let spaces = leading_indent_spaces(line);
    let trimmed = line.trim_start_matches([' ', '\t']);
    let digits_len = trimmed.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits_len == 0 || digits_len > 4 {
        return None;
    }
    let after_digits = &trimmed[digits_len..];
    let delim = after_digits.as_bytes().first().copied()?;
    if delim != b'.' && delim != b')' {
        return None;
    }
    let rest = &after_digits[1..];
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let text = rest.trim();
    if text.is_empty() {
        return None;
    }
    let indent = ((spaces / 2).min(2)) as u8;
    Some((indent, format!("{}.", &trimmed[..digits_len]), text))
}

fn append_joined_line(buf: &mut String, next: &str) {
    let next = next.trim();
    if next.is_empty() {
        return;
    }
    if !buf.is_empty() {
        let prev_ascii = buf
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, ',' | '.' | ';' | ':'));
        let next_ascii = next
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '(' | '[' | '`'));
        if prev_ascii && next_ascii {
            buf.push(' ');
        }
    }
    buf.push_str(next);
}

/// 解析行内 Markdown 语法（支持嵌套，如 `` **`粗体代码`** `` 与 `[**链接**](url)`）。
pub fn parse_inline(text: &str) -> Vec<MdSpan> {
    let mut out = Vec::new();
    parse_inline_into(text, SpanStyle::default(), None, &mut out);
    out
}

fn push_span(out: &mut Vec<MdSpan>, text: &str, style: SpanStyle, link_url: Option<&str>) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = out.last_mut() {
        if last.style == style && last.link_url.as_deref() == link_url {
            last.text.push_str(text);
            return;
        }
    }
    out.push(MdSpan {
        text: text.to_string(),
        style,
        link_url: link_url.map(str::to_string),
    });
}

fn parse_inline_into(s: &str, style: SpanStyle, link_url: Option<&str>, out: &mut Vec<MdSpan>) {
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut plain_start = 0usize;

    let flush_plain = |out: &mut Vec<MdSpan>, start: usize, end: usize| {
        if start < end {
            push_span(out, &s[start..end], style, link_url);
        }
    };

    while i < len {
        match bytes[i] {
            // 反斜杠转义
            b'\\' if i + 1 < len && is_escapable(bytes[i + 1]) => {
                flush_plain(out, plain_start, i);
                push_span(out, &s[i + 1..i + 2], style, link_url);
                i += 2;
                plain_start = i;
            }
            // 行内代码（支持单反引号与双反引号）
            b'`' => {
                let tick_len = bytes[i..].iter().take_while(|&&b| b == b'`').count();
                let content_start = i + tick_len;
                if let Some(close_rel) = find_closing_backticks(&s[content_start..], tick_len) {
                    let raw_code = &s[content_start..content_start + close_rel];
                    let code_text = if raw_code.len() >= 2
                        && raw_code.starts_with(' ')
                        && raw_code.ends_with(' ')
                        && raw_code.bytes().any(|b| b != b' ')
                    {
                        &raw_code[1..raw_code.len() - 1]
                    } else {
                        raw_code
                    };
                    flush_plain(out, plain_start, i);
                    let mut code_style = style;
                    code_style.code = true;
                    push_span(out, code_text, code_style, link_url);
                    i = content_start + close_rel + tick_len;
                    plain_start = i;
                    continue;
                }
                i += tick_len;
            }
            // 图片 ![alt](url) -> 仅温和保留 alt 文本（不暴露裸 URL）
            b'!' if i + 1 < len && bytes[i + 1] == b'[' => {
                if let Some((alt, _url, next_i)) = parse_link_at(s, i + 1) {
                    flush_plain(out, plain_start, i);
                    if !alt.trim().is_empty() {
                        push_span(out, alt.trim(), style, link_url);
                    }
                    i = next_i;
                    plain_start = i;
                    continue;
                }
                i += 1;
            }
            // 链接 [label](url)
            b'[' if link_url.is_none() => {
                if let Some((label, url, next_i)) = parse_link_at(s, i) {
                    flush_plain(out, plain_start, i);
                    parse_inline_into(label, style, Some(url), out);
                    i = next_i;
                    plain_start = i;
                    continue;
                }
                i += 1;
            }
            // 粗体 **...** 或 __...__
            b'*' | b'_' if i + 1 < len && bytes[i + 1] == bytes[i] && !style.bold => {
                let delim = bytes[i];
                let ok_boundary = delim == b'*' || is_underscore_open_boundary(s, i);
                if ok_boundary {
                    if let Some(close_idx) = find_double_delim(s, i + 2, delim) {
                        let inner = &s[i + 2..close_idx];
                        if !inner.is_empty() && !inner.starts_with(' ') && !inner.ends_with(' ') {
                            flush_plain(out, plain_start, i);
                            let mut next_style = style;
                            next_style.bold = true;
                            parse_inline_into(inner, next_style, link_url, out);
                            i = close_idx + 2;
                            plain_start = i;
                            continue;
                        }
                    }
                }
                i += 2;
            }
            // 删除线 ~~...~~
            b'~' if i + 1 < len && bytes[i + 1] == b'~' && !style.strikethrough => {
                if let Some(close_idx) = find_double_delim(s, i + 2, b'~') {
                    let inner = &s[i + 2..close_idx];
                    if !inner.is_empty() && !inner.starts_with(' ') && !inner.ends_with(' ') {
                        flush_plain(out, plain_start, i);
                        let mut next_style = style;
                        next_style.strikethrough = true;
                        parse_inline_into(inner, next_style, link_url, out);
                        i = close_idx + 2;
                        plain_start = i;
                        continue;
                    }
                }
                i += 2;
            }
            // 斜体 *...* 或 _..._（下划线严格校验词边界，保护 snake_case 标识符）
            b'*' | b'_' if !style.italic => {
                let delim = bytes[i];
                let ok_open = if delim == b'_' {
                    is_underscore_open_boundary(s, i)
                } else {
                    true
                };
                if ok_open {
                    if let Some(close_idx) = find_single_delim(s, i + 1, delim) {
                        let inner = &s[i + 1..close_idx];
                        if !inner.is_empty()
                            && !inner.starts_with([' ', '\t'])
                            && !inner.ends_with([' ', '\t'])
                        {
                            flush_plain(out, plain_start, i);
                            let mut next_style = style;
                            next_style.italic = true;
                            parse_inline_into(inner, next_style, link_url, out);
                            i = close_idx + 1;
                            plain_start = i;
                            continue;
                        }
                    }
                }
                i += 1;
            }
            _ => {
                // 前进到下一个 UTF-8 字符边界
                let ch_len = s[i..].chars().next().map_or(1, char::len_utf8);
                i += ch_len;
            }
        }
    }

    flush_plain(out, plain_start, len);
}

fn is_escapable(b: u8) -> bool {
    matches!(
        b,
        b'\\' | b'`' | b'*' | b'_' | b'~' | b'[' | b']' | b'(' | b')' | b'#' | b'-' | b'>' | b'!'
    )
}

fn find_closing_backticks(rest: &str, count: usize) -> Option<usize> {
    let bytes = rest.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let run = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            if run == count {
                return Some(i);
            }
            i += run;
        } else {
            i += 1;
        }
    }
    None
}

fn parse_link_at(s: &str, open_bracket: usize) -> Option<(&str, &str, usize)> {
    let bytes = s.as_bytes();
    if bytes.get(open_bracket) != Some(&b'[') {
        return None;
    }
    let mut depth = 0usize;
    let mut close_bracket = None;
    let mut i = open_bracket;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'[' => {
                depth += 1;
                i += 1;
            }
            b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    close_bracket = Some(i);
                    break;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    let close_b = close_bracket?;
    if bytes.get(close_b + 1) != Some(&b'(') {
        return None;
    }
    let url_start = close_b + 2;
    let mut paren_depth = 0usize;
    let mut close_paren = None;
    let mut j = url_start;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' if j + 1 < bytes.len() => j += 2,
            b'(' => {
                paren_depth += 1;
                j += 1;
            }
            b')' => {
                if paren_depth == 0 {
                    close_paren = Some(j);
                    break;
                }
                paren_depth -= 1;
                j += 1;
            }
            _ => j += 1,
        }
    }
    let close_p = close_paren?;
    let label = &s[open_bracket + 1..close_b];
    let url = s[url_start..close_p].trim();
    if label.is_empty() || url.is_empty() {
        return None;
    }
    Some((label, url, close_p + 1))
}

fn is_underscore_open_boundary(s: &str, idx: usize) -> bool {
    if idx == 0 {
        return true;
    }
    s[..idx]
        .chars()
        .next_back()
        .is_none_or(|c| !c.is_ascii_alphanumeric())
}

fn is_underscore_close_boundary(s: &str, after_idx: usize) -> bool {
    if after_idx >= s.len() {
        return true;
    }
    s[after_idx..]
        .chars()
        .next()
        .is_none_or(|c| !c.is_ascii_alphanumeric())
}

fn find_double_delim(s: &str, start: usize, delim: u8) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = start;
    while i + 1 < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == b'`' {
            let run = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            if let Some(close_rel) = find_closing_backticks(&s[i + run..], run) {
                i = i + run + close_rel + run;
                continue;
            }
            i += run;
            continue;
        }
        if bytes[i] == delim
            && bytes[i + 1] == delim
            && (delim != b'_' || is_underscore_close_boundary(s, i + 2))
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_single_delim(s: &str, start: usize, delim: u8) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == b'`' {
            let run = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            if let Some(close_rel) = find_closing_backticks(&s[i + run..], run) {
                i = i + run + close_rel + run;
                continue;
            }
            i += run;
            continue;
        }
        if bytes[i] == delim {
            // 跳过双标记 `**` / `__`，避免破坏内层粗体
            if i + 1 < bytes.len() && bytes[i + 1] == delim {
                i += 2;
                continue;
            }
            if delim != b'_' || is_underscore_close_boundary(s, i + 1) {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// 在 `egui::Ui` 中将解析好的 Markdown 块渲染为清晰易读的 Fluent 富文本。
/// 返回用户点击的链接 URL（若有）。
pub fn draw_markdown(ui: &mut egui::Ui, p: &Palette, blocks: &[MdBlock]) -> Option<String> {
    let mut clicked_url: Option<String> = None;
    let avail_w = (ui.available_width() - 4.0).max(120.0);
    let body_color = match p.mode {
        ThemeMode::Light => lerp_color(p.text, p.text_secondary, 0.55),
        ThemeMode::Dark => lerp_color(p.text, p.text_secondary, 0.42),
    };
    // 行内标签采用克制的中性浅灰底纹与清晰文字色，柔和融入正文不喧宾夺主
    let code_bg = match p.mode {
        ThemeMode::Light => p.card,
        ThemeMode::Dark => p.card_hover,
    };
    let code_border = match p.mode {
        ThemeMode::Light => p.border,
        ThemeMode::Dark => p.border_strong,
    };
    let code_fg = p.text;

    ui.spacing_mut().item_spacing.y = 0.0;

    for (idx, block) in blocks.iter().enumerate() {
        if idx > 0 {
            let gap = match (&blocks[idx - 1], block) {
                (_, MdBlock::Heading { .. }) => 10.0,
                (MdBlock::Heading { .. }, _) => 5.0,
                (MdBlock::BulletItem { indent: a, .. }, MdBlock::BulletItem { indent: b, .. })
                | (
                    MdBlock::OrderedItem { indent: a, .. },
                    MdBlock::OrderedItem { indent: b, .. },
                ) => {
                    if *a > 0 || *b > 0 {
                        4.0
                    } else {
                        6.0
                    }
                }
                (_, MdBlock::Divider) | (MdBlock::Divider, _) => 6.0,
                _ => 7.0,
            };
            ui.add_space(gap);
        }

        match block {
            MdBlock::Heading { level, spans } => {
                let font_size = match level {
                    1 => 14.0,
                    2 => 13.5,
                    _ => 12.5,
                };
                let line_h = font_size + 6.0;
                let bar_w = 8.0_f32;
                let text_w = (avail_w - bar_w).max(80.0);
                let built = build_layout_job(
                    spans, text_w, font_size, line_h, p.text, p.text, code_fg, p.accent, true,
                );
                let galley = ui.painter().layout_job(built.job);
                let h = galley.size().y.max(line_h);
                let (rect, _) = ui.allocate_exact_size(Vec2::new(avail_w, h), Sense::hover());
                let first_row_cy = rect.top()
                    + galley
                        .rows
                        .first()
                        .map_or(line_h * 0.5, |r| r.rect.center().y);
                let bar_h = (font_size - 1.0).max(11.0);
                let bar_rect = Rect::from_center_size(
                    Pos2::new(rect.left() + 1.5, first_row_cy),
                    Vec2::new(3.0, bar_h),
                );
                ui.painter()
                    .rect_filled(bar_rect, CornerRadius::same(2), p.accent);
                let text_pos = Pos2::new(rect.left() + bar_w, rect.top());
                paint_inline_code_pills(
                    ui.painter(),
                    text_pos,
                    &galley,
                    &built.code_sections,
                    font_size + 3.5,
                    code_bg,
                    code_border,
                );
                ui.painter().galley(text_pos, galley.clone(), p.text);
                if let Some(url) = handle_link_hit(ui, text_pos, &galley, &built.links) {
                    clicked_url = Some(url);
                }
            }

            MdBlock::Paragraph(spans) => {
                let built = build_layout_job(
                    spans,
                    avail_w,
                    BODY_FONT_SIZE,
                    BODY_LINE_HEIGHT,
                    body_color,
                    p.text,
                    code_fg,
                    p.accent,
                    false,
                );
                if let Some(url) =
                    paint_rich_block(ui, avail_w, built, body_color, code_bg, code_border)
                {
                    clicked_url = Some(url);
                }
            }

            MdBlock::BulletItem { indent, spans } => {
                let left_pad = f32::from(*indent) * 14.0;
                let marker_col = 13.0_f32;
                let text_offset = left_pad + marker_col;
                let text_w = (avail_w - text_offset).max(80.0);
                let built = build_layout_job(
                    spans,
                    text_w,
                    BODY_FONT_SIZE,
                    BODY_LINE_HEIGHT,
                    body_color,
                    p.text,
                    code_fg,
                    p.accent,
                    false,
                );
                let galley = ui.painter().layout_job(built.job);
                let h = galley.size().y.max(BODY_LINE_HEIGHT);
                let (rect, _) = ui.allocate_exact_size(Vec2::new(avail_w, h), Sense::hover());
                let first_row_cy = rect.top()
                    + galley
                        .rows
                        .first()
                        .map_or(BODY_LINE_HEIGHT * 0.5, |r| r.rect.center().y);
                let dot_center = Pos2::new(rect.left() + left_pad + 4.5, first_row_cy);
                if *indent == 0 {
                    ui.painter().circle_filled(dot_center, 2.3, p.accent);
                } else {
                    ui.painter()
                        .circle_stroke(dot_center, 2.0, Stroke::new(1.1_f32, p.text_muted));
                }
                let text_pos = Pos2::new(rect.left() + text_offset, rect.top());
                paint_inline_code_pills(
                    ui.painter(),
                    text_pos,
                    &galley,
                    &built.code_sections,
                    BODY_FONT_SIZE + 3.5,
                    code_bg,
                    code_border,
                );
                ui.painter().galley(text_pos, galley.clone(), body_color);
                if let Some(url) = handle_link_hit(ui, text_pos, &galley, &built.links) {
                    clicked_url = Some(url);
                }
            }

            MdBlock::OrderedItem {
                number,
                indent,
                spans,
            } => {
                let left_pad = f32::from(*indent) * 14.0;
                let num_galley = ui.painter().layout_no_wrap(
                    number.clone(),
                    FontId::new(11.5, bold_family()),
                    p.accent,
                );
                let marker_col = (num_galley.size().x + 6.0).max(18.0);
                let text_offset = left_pad + marker_col;
                let text_w = (avail_w - text_offset).max(80.0);
                let built = build_layout_job(
                    spans,
                    text_w,
                    BODY_FONT_SIZE,
                    BODY_LINE_HEIGHT,
                    body_color,
                    p.text,
                    code_fg,
                    p.accent,
                    false,
                );
                let galley = ui.painter().layout_job(built.job);
                let h = galley.size().y.max(BODY_LINE_HEIGHT);
                let (rect, _) = ui.allocate_exact_size(Vec2::new(avail_w, h), Sense::hover());
                let first_row_cy = rect.top()
                    + galley
                        .rows
                        .first()
                        .map_or(BODY_LINE_HEIGHT * 0.5, |r| r.rect.center().y);
                ui.painter().galley(
                    Pos2::new(
                        rect.left() + left_pad,
                        first_row_cy - num_galley.size().y * 0.5,
                    ),
                    num_galley,
                    p.accent,
                );
                let text_pos = Pos2::new(rect.left() + text_offset, rect.top());
                paint_inline_code_pills(
                    ui.painter(),
                    text_pos,
                    &galley,
                    &built.code_sections,
                    BODY_FONT_SIZE + 3.5,
                    code_bg,
                    code_border,
                );
                ui.painter().galley(text_pos, galley.clone(), body_color);
                if let Some(url) = handle_link_hit(ui, text_pos, &galley, &built.links) {
                    clicked_url = Some(url);
                }
            }

            MdBlock::Quote(spans) => {
                let inner_w = (avail_w - 18.0).max(80.0);
                let built = build_layout_job(
                    spans,
                    inner_w,
                    11.5,
                    18.5,
                    p.text_secondary,
                    p.text,
                    code_fg,
                    p.accent,
                    false,
                );
                let galley = ui.painter().layout_job(built.job);
                let box_h = galley.size().y + 10.0;
                let (rect, _) = ui.allocate_exact_size(Vec2::new(avail_w, box_h), Sense::hover());
                ui.painter().rect_filled(
                    rect,
                    CornerRadius::same(6),
                    with_alpha(p.accent, if p.mode == ThemeMode::Dark { 18 } else { 12 }),
                );
                let bar = Rect::from_min_max(
                    Pos2::new(rect.left() + 1.0, rect.top() + 3.0),
                    Pos2::new(rect.left() + 3.5, rect.bottom() - 3.0),
                );
                ui.painter()
                    .rect_filled(bar, CornerRadius::same(2), p.accent_border);
                let text_pos = Pos2::new(rect.left() + 11.0, rect.top() + 5.0);
                paint_inline_code_pills(
                    ui.painter(),
                    text_pos,
                    &galley,
                    &built.code_sections,
                    15.0,
                    code_bg,
                    code_border,
                );
                ui.painter()
                    .galley(text_pos, galley.clone(), p.text_secondary);
                if let Some(url) = handle_link_hit(ui, text_pos, &galley, &built.links) {
                    clicked_url = Some(url);
                }
            }

            MdBlock::CodeBlock(code) => {
                egui::Frame::new()
                    .fill(p.card)
                    .stroke(Stroke::new(1.0_f32, p.border))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(10, 7))
                    .show(ui, |ui| {
                        ui.set_width((avail_w - 22.0).max(80.0));
                        let mut job = egui::text::LayoutJob::default();
                        job.wrap.max_width = (avail_w - 22.0).max(80.0);
                        job.append(
                            code,
                            0.0,
                            TextFormat {
                                font_id: FontId::monospace(11.0),
                                line_height: Some(16.5),
                                color: p.text,
                                ..Default::default()
                            },
                        );
                        ui.add(egui::Label::new(job));
                    });
            }

            MdBlock::Divider => {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(avail_w, 3.0), Sense::hover());
                ui.painter().line_segment(
                    [rect.left_center(), rect.right_center()],
                    Stroke::new(1.0_f32, p.divider),
                );
            }
        }
    }

    clicked_url
}

struct LinkByteRange {
    char_start: usize,
    char_end: usize,
    url: String,
}

struct BuiltRichJob {
    job: egui::text::LayoutJob,
    links: Vec<LinkByteRange>,
    code_sections: Vec<u32>,
}

#[allow(clippy::too_many_arguments)]
fn build_layout_job(
    spans: &[MdSpan],
    max_width: f32,
    base_size: f32,
    line_height: f32,
    base_color: Color32,
    strong_color: Color32,
    code_fg: Color32,
    link_color: Color32,
    force_bold: bool,
) -> BuiltRichJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = max_width;
    let mut links = Vec::new();
    let mut code_sections = Vec::new();
    let mut char_cursor = 0usize;
    let mut prev_was_code = false;

    for (idx, span) in spans.iter().enumerate() {
        let is_bold = force_bold || span.style.bold;
        let is_link = span.link_url.is_some();

        let (font_id, color) = if span.style.code {
            let family = if is_bold {
                bold_family()
            } else {
                FontFamily::Monospace
            };
            let sz = if force_bold {
                (base_size - 0.5).max(11.0)
            } else {
                CODE_FONT_SIZE
            };
            (
                FontId::new(sz, family),
                if is_link { link_color } else { code_fg },
            )
        } else {
            let family = if is_bold {
                bold_family()
            } else {
                FontFamily::Proportional
            };
            let color = if is_link {
                link_color
            } else if is_bold {
                strong_color
            } else {
                base_color
            };
            (FontId::new(base_size, family), color)
        };

        let format = TextFormat {
            font_id,
            // 行内代码留空 line_height 配合 Align::Center，让 Consolas 在 19.5px 行高内自然下沉并与微软雅黑基线齐平
            line_height: if span.style.code {
                None
            } else {
                Some(line_height)
            },
            color,
            background: Color32::TRANSPARENT,
            italics: span.style.italic,
            underline: if is_link {
                Stroke::new(1.0_f32, with_alpha(link_color, 180))
            } else {
                Stroke::NONE
            },
            strikethrough: if span.style.strikethrough {
                Stroke::new(1.0_f32, base_color)
            } else {
                Stroke::NONE
            },
            valign: Align::Center,
            ..Default::default()
        };

        let section_idx = job.sections.len() as u32;
        if span.style.code {
            code_sections.push(section_idx);
        }
        // 在行内代码胶囊前后留出轻量呼吸间距（不插可断行空格，杜绝行尾出现孤立色块）
        let leading_space = if idx == 0 {
            0.0
        } else if span.style.code || prev_was_code {
            3.0
        } else {
            0.0
        };

        let start_char = char_cursor;
        char_cursor += span.text.chars().count();
        job.append(&span.text, leading_space, format);
        if let Some(url) = &span.link_url {
            links.push(LinkByteRange {
                char_start: start_char,
                char_end: char_cursor,
                url: url.clone(),
            });
        }
        prev_was_code = span.style.code;
    }

    BuiltRichJob {
        job,
        links,
        code_sections,
    }
}

/// 在 Galley 绘制前为每个行内代码区段绘制圆角胶囊底纹（严丝合缝贴合字形、不粘连上下行、绝不断裂）。
fn paint_inline_code_pills(
    painter: &egui::Painter,
    origin: Pos2,
    galley: &egui::Galley,
    code_sections: &[u32],
    pill_height: f32,
    fill: Color32,
    border: Color32,
) {
    if code_sections.is_empty() {
        return;
    }
    let stroke = Stroke::new(1.0_f32, border);
    for row in &galley.rows {
        let half_h = (pill_height * 0.5).min(row.rect.height() * 0.46);
        let glyphs = &row.glyphs;
        let mut i = 0usize;
        while i < glyphs.len() {
            if !code_sections.contains(&glyphs[i].section_index) {
                i += 1;
                continue;
            }
            let sec = glyphs[i].section_index;
            let start = i;
            while i < glyphs.len() && glyphs[i].section_index == sec {
                i += 1;
            }
            let mut run_start = start;
            let mut run_end = i;
            while run_start < run_end && glyphs[run_start].chr.is_whitespace() {
                run_start += 1;
            }
            while run_end > run_start && glyphs[run_end - 1].chr.is_whitespace() {
                run_end -= 1;
            }
            if run_start >= run_end {
                continue;
            }
            let min_x = origin.x + glyphs[run_start].pos.x - 3.0;
            let max_x = origin.x + glyphs[run_end - 1].max_x() + 3.0;
            // `glyph.pos.y` 是精确文字基线；按基线上方 ~3.2px 居中包裹，保证上下留白完全对称
            let baseline_y = origin.y + glyphs[run_start].pos.y;
            let pill_cy = baseline_y - 3.2;
            let rect = Rect::from_min_max(
                Pos2::new(min_x, pill_cy - half_h),
                Pos2::new(max_x, pill_cy + half_h),
            );
            painter.rect(
                rect,
                CornerRadius::same(4),
                fill,
                stroke,
                StrokeKind::Inside,
            );
        }
    }
}

fn paint_rich_block(
    ui: &mut egui::Ui,
    avail_w: f32,
    built: BuiltRichJob,
    fallback_color: Color32,
    code_bg: Color32,
    code_border: Color32,
) -> Option<String> {
    let galley = ui.painter().layout_job(built.job);
    let h = galley.size().y.max(BODY_LINE_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(avail_w, h), Sense::hover());
    paint_inline_code_pills(
        ui.painter(),
        rect.min,
        &galley,
        &built.code_sections,
        BODY_FONT_SIZE + 3.5,
        code_bg,
        code_border,
    );
    ui.painter()
        .galley(rect.min, galley.clone(), fallback_color);
    handle_link_hit(ui, rect.min, &galley, &built.links)
}

fn handle_link_hit(
    ui: &mut egui::Ui,
    origin: Pos2,
    galley: &egui::Galley,
    links: &[LinkByteRange],
) -> Option<String> {
    if links.is_empty() {
        return None;
    }
    let rect = Rect::from_min_size(origin, galley.size());
    let resp = ui.interact(
        rect,
        ui.id().with(("md_links", origin.x as i32, origin.y as i32)),
        Sense::click(),
    );
    let pos = resp.hover_pos()?;
    let rel = pos - origin;
    let cursor = galley.cursor_from_pos(rel);
    let idx = cursor.ccursor.index;
    let hit = links
        .iter()
        .find(|r| idx >= r.char_start && idx < r.char_end)?;
    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    let url = hit.url.clone();
    let clicked = resp.clicked();
    resp.on_hover_text(url.clone());
    clicked.then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_changelog_v024_and_strips_raw_markdown_syntax() {
        let src = r#"深度优化全链路内存占用与长期驻留开销：主界面打开时私有内存下降 **63%**（约 **112 MB → 41 MB**），后台守护与最小化挂机全程零堆积。

- **系统字库只读映射与零拷贝复用**：主界面通过 Win32 只读文件映射（`MapViewOfFile`）直接共享系统微软雅黑与等宽字库，单次打开主窗口直降约 **71 MB** 私有内存。
- **日志尾部定长寻址**：启动回填日志改为按 `32 KiB` 反向读取，`ui_prefs.json` 保持兼容。"#;

        let blocks = parse_markdown(src);
        assert_eq!(blocks.len(), 3);

        let MdBlock::Paragraph(p) = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(p.iter().any(|s| s.text == "63%" && s.style.bold));
        assert!(p.iter().any(|s| s.text == "112 MB → 41 MB" && s.style.bold));
        assert!(!p.iter().any(|s| s.text.contains("**")));

        let MdBlock::BulletItem { indent, spans } = &blocks[1] else {
            panic!("expected bullet");
        };
        assert_eq!(*indent, 0);
        assert!(spans[0].style.bold && spans[0].text == "系统字库只读映射与零拷贝复用");
        assert!(spans
            .iter()
            .any(|s| s.style.code && s.text == "MapViewOfFile"));

        let MdBlock::BulletItem { spans: b2, .. } = &blocks[2] else {
            panic!("expected second bullet");
        };
        assert!(b2.iter().any(|s| s.style.code && s.text == "ui_prefs.json"));
    }

    #[test]
    fn supports_headings_nested_lists_bold_code_links_and_preserves_snake_case() {
        let src = r#"### ✨ 校园网自助服务（`172.16.4.2/Self/`）

- **50:50 对称双卡**：
  - 支持 **`运营商 | 校园网`** 一键切换，配置文件 `auto_check_updates` 与普通词 my_var_name 不应被拆成斜体。
1. 第一步：查看 [发布页面](https://github.com/junkaih2019/gxnu-campus-net-helper/releases)
2. 第二步：支持 *斜体* 与 ~~删除线~~

> 温馨提示：保留原有账号配置。

---

```text
gxnu-guard --once
```"#;

        let blocks = parse_markdown(src);
        assert!(matches!(
            &blocks[0],
            MdBlock::Heading { level: 3, spans }
                if spans[0].text.starts_with("✨ 校园网自助服务")
                    && spans.iter().any(|s| s.style.code && s.text == "172.16.4.2/Self/")
        ));
        assert!(matches!(&blocks[1], MdBlock::BulletItem { indent: 0, .. }));
        let MdBlock::BulletItem { indent: 1, spans } = &blocks[2] else {
            panic!("expected nested bullet");
        };
        assert!(spans
            .iter()
            .any(|s| s.style.bold && s.style.code && s.text == "运营商 | 校园网"));
        assert!(spans
            .iter()
            .any(|s| !s.style.italic && s.text.contains("my_var_name")));

        let MdBlock::OrderedItem {
            number, spans: o1, ..
        } = &blocks[3]
        else {
            panic!("expected ordered item 1");
        };
        assert_eq!(number, "1.");
        assert!(o1.iter().any(|s| {
            s.text == "发布页面"
                && s.link_url.as_deref()
                    == Some("https://github.com/junkaih2019/gxnu-campus-net-helper/releases")
        }));

        let MdBlock::OrderedItem { spans: o2, .. } = &blocks[4] else {
            panic!("expected ordered item 2");
        };
        assert!(o2.iter().any(|s| s.style.italic && s.text == "斜体"));
        assert!(o2
            .iter()
            .any(|s| s.style.strikethrough && s.text == "删除线"));

        assert!(matches!(&blocks[5], MdBlock::Quote(_)));
        assert!(matches!(&blocks[6], MdBlock::Divider));
        assert!(matches!(
            &blocks[7],
            MdBlock::CodeBlock(code) if code == "gxnu-guard --once"
        ));
    }

    #[test]
    fn cache_reuses_parsed_blocks_until_source_changes() {
        let mut cache = MarkdownCache::default();
        assert_eq!(cache.blocks_for("- **A**").len(), 1);
        assert_eq!(cache.blocks_for("- **A**").len(), 1);
        assert_eq!(cache.parse_count, 1);
        assert_eq!(cache.blocks_for("- **B**").len(), 1);
        assert_eq!(cache.parse_count, 2);
        cache.clear();
        assert!(cache.blocks.is_empty());
    }
}
