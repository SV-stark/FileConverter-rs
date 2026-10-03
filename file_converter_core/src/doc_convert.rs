use crate::error::{FileConverterError, Result};
use crate::types::OutputType;
use ebook_rs::Book;
use std::fs;
use std::path::Path;
use std::process::Command;

use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str};

fn write_output_file<P: AsRef<Path>, C: AsRef<[u8]>>(path: P, content: C) -> Result<()> {
    if let Some(parent) = path.as_ref().parent()
        && !parent.as_os_str().is_empty()
    {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(path, content).map_err(FileConverterError::Io)
}

fn sanitize_text_for_pdf(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len());
    for c in text.chars() {
        if c == '\n' || c == '\r' || c == '\t' {
            bytes.push(b' ');
        } else if (c as u32) < 128 && !c.is_control() {
            bytes.push(c as u8);
        } else if (160..=255).contains(&(c as u32)) {
            // ISO-8859-1 / Latin-1 accented characters (é, ü, ñ, à, ç, etc.)
            bytes.push(c as u8);
        } else {
            match c {
                '“' | '”' => bytes.push(b'"'),
                '‘' | '’' => bytes.push(b'\''),
                '—' | '–' => bytes.push(b'-'),
                '…' => bytes.extend_from_slice(b"..."),
                '\u{00A0}' => bytes.push(b' '),
                _ => bytes.push(b' '),
            }
        }
    }
    bytes
}

/// Creates a multi-page vector PDF document from plain text or extracted markup.
pub fn create_pdf_from_text(title: &str, text: &str, output_path: &str) -> Result<()> {
    let mut pdf = Pdf::new();

    let catalog_id = Ref::new(1);
    let pages_id = Ref::new(2);
    let font_id = Ref::new(3);
    let bold_font_id = Ref::new(4);

    let page_width = 595.28f32; // A4 standard width
    let page_height = 841.89f32; // A4 standard height
    let margin = 50.0f32;
    let line_height = 14.0f32;
    let max_lines_per_page = ((page_height - margin * 2.0) / line_height).floor() as usize;

    let mut lines = Vec::new();
    for raw_line in text.lines() {
        let trimmed = raw_line.trim_end();
        if trimmed.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut cur_line = String::new();
        for word in trimmed.split_whitespace() {
            if cur_line.is_empty() {
                cur_line.push_str(word);
            } else if cur_line.len() + 1 + word.len() <= 80 {
                cur_line.push(' ');
                cur_line.push_str(word);
            } else {
                lines.push(cur_line);
                cur_line = word.to_string();
            }
        }
        if !cur_line.is_empty() {
            lines.push(cur_line);
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    let mut page_ids = Vec::new();
    let mut current_ref_num = 5;

    for (page_idx, chunk) in lines.chunks(max_lines_per_page.max(1)).enumerate() {
        let page_id = Ref::new(current_ref_num);
        current_ref_num += 1;
        let content_id = Ref::new(current_ref_num);
        current_ref_num += 1;

        let mut content = Content::new();
        content.begin_text();
        content.set_font(Name(b"F1"), 10.0);
        content.set_leading(line_height);

        let start_y = page_height - margin;
        content.next_line(margin, start_y);

        if page_idx == 0 && !title.is_empty() {
            content.set_font(Name(b"F2"), 14.0);
            let sanitized_title = sanitize_text_for_pdf(title);
            content.show(Str(&sanitized_title));
            content.next_line(0.0, -20.0);
            content.set_font(Name(b"F1"), 10.0);
        }

        for line in chunk {
            let sanitized = sanitize_text_for_pdf(line);
            content.show(Str(&sanitized));
            content.next_line(0.0, -line_height);
        }
        content.end_text();

        pdf.stream(content_id, &content.finish());

        let mut page = pdf.page(page_id);
        page.media_box(Rect::new(0.0, 0.0, page_width, page_height));
        page.parent(pages_id);
        page.contents(content_id);

        let mut resources = page.resources();
        let mut fonts = resources.fonts();
        fonts.pair(Name(b"F1"), font_id);
        fonts.pair(Name(b"F2"), bold_font_id);
        fonts.finish();
        resources.finish();
        page.finish();

        page_ids.push(page_id);
    }

    let mut pages = pdf.pages(pages_id);
    pages.kids(page_ids.iter().copied());
    pages.count(page_ids.len() as i32);
    pages.finish();

    pdf.type1_font(font_id).base_font(Name(b"Helvetica"));
    pdf.type1_font(bold_font_id)
        .base_font(Name(b"Helvetica-Bold"));
    pdf.catalog(catalog_id).pages(pages_id);

    write_output_file(output_path, pdf.finish())?;
    Ok(())
}

/// Convert eBook files (EPUB, MOBI, AZW, AZW3, KFX, FB2, CBZ, KEPUB, LIT, etc.)
/// to PDF, plain text, HTML, or a raster image using `ebook-rs`.
pub fn run_ebook_conversion(
    input_path: &str,
    output_path: &str,
    output_type: OutputType,
    progress_cb: &(dyn Fn(f32, &str) + Sync),
) -> Result<()> {
    progress_cb(0.1, "Opening eBook document (ebook-rs)");

    let book = Book::from_file(input_path)
        .map_err(|e| FileConverterError::Invalid(format!("Failed to parse eBook: {:?}", e)))?;

    let meta_title = book.metadata().title.trim().to_string();
    let title = if !meta_title.is_empty() {
        meta_title
    } else {
        Path::new(input_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("eBook Document")
            .to_string()
    };

    progress_cb(0.25, "Extracting eBook sections");
    let mut html_body = String::new();
    let mut text_body = String::new();

    let sections = book.sections();
    let total_sections = sections.len().max(1);

    for (i, section) in sections.iter().enumerate() {
        if !section.raw_html.is_empty() {
            html_body.push_str(&section.raw_html);
            html_body.push_str("\n<hr/>\n");
        }

        if !section.plain_text.is_empty() {
            text_body.push_str(&section.plain_text);
            text_body.push_str("\n\n--- Section Break ---\n\n");
        } else if !section.raw_html.is_empty() {
            let plain = strip_html_tags(&section.raw_html);
            text_body.push_str(&plain);
            text_body.push_str("\n\n--- Section Break ---\n\n");
        }

        let prog = 0.25 + (i as f32 / total_sections as f32) * 0.6;
        progress_cb(prog, &format!("Processing Section {}/{}", i + 1, total_sections));
    }

    progress_cb(0.9, "Writing output file");
    write_document_output(
        &title,
        &text_body,
        Some(&html_body),
        output_path,
        output_type,
        progress_cb,
    )?;

    progress_cb(1.0, "Complete");
    Ok(())
}

/// Backward compatibility alias for EPUB conversions
pub fn run_epub_conversion(
    input_path: &str,
    output_path: &str,
    output_type: OutputType,
    progress_cb: &(dyn Fn(f32, &str) + Sync),
) -> Result<()> {
    run_ebook_conversion(input_path, output_path, output_type, progress_cb)
}

/// Convert a Markdown file to PDF, plain text, HTML, or a raster image.
pub fn run_markdown_conversion(
    input_path: &str,
    output_path: &str,
    output_type: OutputType,
    progress_cb: &(dyn Fn(f32, &str) + Sync),
) -> Result<()> {
    progress_cb(0.2, "Reading Markdown file");
    let content = fs::read_to_string(input_path)?;

    progress_cb(0.5, "Parsing Markdown (pulldown-cmark)");
    let parser = pulldown_cmark::Parser::new(&content);
    let mut html_output = String::new();
    pulldown_cmark::html::push_html(&mut html_output, parser);

    progress_cb(0.85, "Formatting output file");
    let file_stem = Path::new(input_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Document")
        .to_string();

    let plain_text = strip_html_tags(&html_output);

    write_document_output(
        &file_stem,
        &plain_text,
        Some(&html_output),
        output_path,
        output_type,
        progress_cb,
    )?;

    progress_cb(1.0, "Complete");
    Ok(())
}

/// Convert a Typst document to PDF or HTML (or any other supported output).
pub fn run_typst_conversion(
    input_path: &str,
    output_path: &str,
    output_type: OutputType,
    progress_cb: &(dyn Fn(f32, &str) + Sync),
) -> Result<()> {
    progress_cb(0.2, "Checking Typst compiler");

    let wants_pdf = output_type == OutputType::Pdf
        || (output_type == OutputType::None && output_path.to_lowercase().ends_with(".pdf"));

    if wants_pdf
        && let Some(typst_exe) = crate::path_helpers::find_in_path("typst.exe")
            .or_else(|| crate::path_helpers::find_in_path("typst"))
    {
        progress_cb(0.5, "Compiling document with Typst");
        if let Some(parent) = Path::new(output_path).parent() {
            let _ = fs::create_dir_all(parent);
        }

        let mut child = Command::new(&typst_exe)
            .arg("compile")
            .arg(input_path)
            .arg(output_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                FileConverterError::Invalid(format!("Failed to execute typst CLI: {:?}", e))
            })?;

        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(120);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if start.elapsed() > timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(FileConverterError::Timeout(
                            "Typst compilation timed out after 120s".to_string(),
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(FileConverterError::Invalid(format!(
                        "Error waiting for typst CLI: {:?}",
                        e
                    )));
                }
            }
        };

        if status.success() {
            progress_cb(1.0, "Complete");
            return Ok(());
        }

        let mut stderr_str = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr_str);
        }
        return Err(FileConverterError::Invalid(format!(
            "Typst compilation failed: {}",
            stderr_str.trim()
        )));
    }

    // Fallback when the Typst CLI is unavailable: treat the source as
    // text/markup so the requested output is still produced.
    progress_cb(0.5, "Formatting Typst source code");
    let content = fs::read_to_string(input_path)?;
    let parser = pulldown_cmark::Parser::new(&content);
    let mut html_output = String::new();
    pulldown_cmark::html::push_html(&mut html_output, parser);

    let file_stem = Path::new(input_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Typst Document")
        .to_string();

    let plain_text = strip_html_tags(&html_output);

    write_document_output(
        &file_stem,
        &plain_text,
        Some(&html_output),
        output_path,
        output_type,
        progress_cb,
    )?;

    progress_cb(1.0, "Complete (fallback renderer)");
    Ok(())
}

/// Extracts readable text from a plain-text / markup document.
///
/// Supports raw text (`txt`, `csv`, `json`, ...) as well as light markup
/// (`html`, `rtf`) whose tags are stripped. Returns `None` when the file is
/// binary and therefore not a text document at all.
fn read_text_document(input_path: &str) -> Option<String> {
    let bytes = fs::read(input_path).ok()?;

    // Reject obvious binary payloads (NUL bytes in the first 4 KiB).
    let probe = &bytes[..bytes.len().min(4096)];
    if probe.contains(&0u8) {
        return None;
    }

    let raw = String::from_utf8_lossy(&bytes).into_owned();
    let ext = Path::new(input_path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "htm" | "html" | "xhtml" | "rtf" => Some(decode_entities(&strip_html_tags(&raw))),
        _ => Some(raw),
    }
}

/// Minimal HTML entity decoder.
///
/// Numeric references plus the handful of named entities that matter for
/// rendered document text.
fn decode_entities(text: &str) -> String {
    const NAMED: &[(&str, char)] = &[
        ("amp", '&'),
        ("lt", '<'),
        ("gt", '>'),
        ("quot", '"'),
        ("apos", '\''),
        ("nbsp", '\u{00A0}'),
        ("hellip", '\u{2026}'),
        ("mdash", '\u{2014}'),
        ("ndash", '\u{2013}'),
        ("lsquo", '\u{2018}'),
        ("rsquo", '\u{2019}'),
        ("ldquo", '\u{201C}'),
        ("rdquo", '\u{201D}'),
        ("copy", '\u{00A9}'),
        ("reg", '\u{00AE}'),
        ("trade", '\u{2122}'),
        ("deg", '\u{00B0}'),
        ("euro", '\u{20AC}'),
        ("pound", '\u{00A3}'),
        ("yen", '\u{00A5}'),
        ("middot", '\u{00B7}'),
        ("bull", '\u{2022}'),
    ];

    const ACCENTED: &[(&str, char)] = &[
        ("aacute", '\u{00E1}'),
        ("agrave", '\u{00E0}'),
        ("acirc", '\u{00E2}'),
        ("auml", '\u{00E4}'),
        ("aring", '\u{00E5}'),
        ("aelig", '\u{00E6}'),
        ("ccedil", '\u{00E7}'),
        ("eacute", '\u{00E9}'),
        ("egrave", '\u{00E8}'),
        ("ecirc", '\u{00EA}'),
        ("euml", '\u{00EB}'),
        ("iacute", '\u{00ED}'),
        ("igrave", '\u{00EC}'),
        ("ntilde", '\u{00F1}'),
        ("oacute", '\u{00F3}'),
        ("ograve", '\u{00F2}'),
        ("ocirc", '\u{00F4}'),
        ("ouml", '\u{00F6}'),
        ("uacute", '\u{00FA}'),
        ("ugrave", '\u{00F9}'),
        ("ucirc", '\u{00FB}'),
        ("uuml", '\u{00FC}'),
    ];

    if !text.contains('&') {
        return text.to_string();
    }

    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'&'
            && let Some(semi) = memchr::memchr(b';', &bytes[i..])
        {
            let end = i + semi;
            let entity = &text[i + 1..end];
            let replacement = if let Some(num) = entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
            {
                u32::from_str_radix(num, 16).ok().and_then(char::from_u32)
            } else if let Some(num) = entity.strip_prefix('#') {
                num.parse::<u32>().ok().and_then(char::from_u32)
            } else if let Some((_, c)) = NAMED.iter().find(|(name, _)| *name == entity) {
                Some(*c)
            } else {
                ACCENTED
                    .iter()
                    .find(|(name, _)| *name == entity)
                    .map(|(_, c)| *c)
            };

            if let Some(c) = replacement {
                out.push(c);
                i = end + 1;
                continue;
            }
        }

        let ch_len = text[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        out.push_str(&text[i..i + ch_len]);
        i += ch_len;
    }

    out
}

fn escape_xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\t' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// Builds a printable A4 SVG page for the given text. Rasterized by `resvg`.
fn layout_text_as_svg(title: &str, text: &str) -> String {
    const PAGE_W: f32 = 1240.0;
    const PAGE_H: f32 = 1754.0;
    const MARGIN: f32 = 72.0;
    const FONT_SIZE: f32 = 24.0;
    const LINE_HEIGHT: f32 = 34.0;
    const TITLE_SIZE: f32 = 40.0;

    let content_width_px = PAGE_W - MARGIN * 2.0;
    // Rough monospace-ish advance estimate for a proportional UI font.
    let chars_per_line = ((content_width_px / (FONT_SIZE * 0.52)).floor() as usize).max(20);

    let mut lines: Vec<String> = Vec::new();
    let mut y = MARGIN + TITLE_SIZE;

    if !title.is_empty() {
        y += TITLE_SIZE;
        lines.push(format!(
            r##"<text x="{x:.1}" y="{y:.1}" font-family="Segoe UI, Helvetica, Arial, sans-serif" font-size="{ts}" font-weight="600" fill="#111827">{t}</text>"##,
            x = MARGIN,
            y = y,
            ts = TITLE_SIZE,
            t = escape_xml(title)
        ));
        y += LINE_HEIGHT;
    }

    let max_y = PAGE_H - MARGIN;
    for raw_line in text.lines() {
        if y >= max_y {
            break;
        }
        let trimmed = raw_line.trim_end();
        if trimmed.is_empty() {
            y += LINE_HEIGHT;
            continue;
        }

        for chunk in wrap_line(trimmed, chars_per_line) {
            if y >= max_y {
                break;
            }
            lines.push(format!(
                r##"<text x="{x:.1}" y="{y:.1}" font-family="Segoe UI, Helvetica, Arial, sans-serif" font-size="{fs}" fill="#1f2937" xml:space="preserve">{t}</text>"##,
                x = MARGIN,
                y = y,
                fs = FONT_SIZE,
                t = escape_xml(&chunk)
            ));
            y += LINE_HEIGHT;
        }
    }

    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">
<rect width="{w}" height="{h}" fill="#ffffff"/>
{body}
</svg>"##,
        w = PAGE_W,
        h = PAGE_H,
        body = lines.join("\n")
    )
}

/// Word-wraps a single line to `max_chars`, breaking over-long words.
fn wrap_line(line: &str, max_chars: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();

    for word in line.split_whitespace() {
        if current.is_empty() {
            if word.chars().count() > max_chars {
                let mut rest: Vec<char> = word.chars().collect();
                while rest.len() > max_chars {
                    out.push(rest[..max_chars].iter().collect());
                    rest = rest[max_chars..].to_vec();
                }
                current = rest.into_iter().collect();
            } else {
                current.push_str(word);
            }
        } else if current.chars().count() + 1 + word.chars().count() <= max_chars {
            current.push(' ');
            current.push_str(word);
        } else {
            out.push(std::mem::take(&mut current));
            if word.chars().count() > max_chars {
                let mut rest: Vec<char> = word.chars().collect();
                while rest.len() > max_chars {
                    out.push(rest[..max_chars].iter().collect());
                    rest = rest[max_chars..].to_vec();
                }
                current = rest.into_iter().collect();
            } else {
                current.push_str(word);
            }
        }
    }

    if !current.is_empty() {
        out.push(current);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Converts a plain-text / markup document (`.txt`, `.html`, `.csv`, `.rtf`, ...)
/// to PDF, plain text, HTML, or a raster image.
///
/// These extensions are handled here instead of being routed to the image engine,
/// which previously failed with "unknown image format" for every text document.
pub fn run_text_document_conversion(
    input_path: &str,
    output_path: &str,
    output_type: OutputType,
    progress_cb: &(dyn Fn(f32, &str) + Sync),
) -> Result<()> {
    progress_cb(0.2, "Reading document");

    let raw = read_text_document(input_path).ok_or_else(|| {
        FileConverterError::Invalid(format!(
            "'{}' is not a readable text document (binary data detected)",
            input_path
        ))
    })?;

    let file_stem = Path::new(input_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Document")
        .to_string();

    write_document_output(
        &file_stem,
        &raw,
        None,
        output_path,
        output_type,
        progress_cb,
    )?;

    progress_cb(1.0, "Complete");
    Ok(())
}

/// Writes a document body to the requested output format.
///
/// Shared by every text-producing engine (eBook, Markdown, Typst, plain text) so
/// that each one honours the preset's output type. Previously these engines wrote
/// HTML regardless of the requested extension, which produced e.g. an HTML file
/// named `.png`.
fn write_document_output(
    title: &str,
    plain_text: &str,
    html_body: Option<&str>,
    output_path: &str,
    output_type: OutputType,
    progress_cb: &(dyn Fn(f32, &str) + Sync),
) -> Result<()> {
    let lower_path = output_path.to_lowercase();

    let wants_pdf = output_type == OutputType::Pdf
        || (output_type == OutputType::None && lower_path.ends_with(".pdf"));
    let wants_txt = output_type == OutputType::Txt
        || (output_type == OutputType::None && lower_path.ends_with(".txt"));
    let wants_html = output_type == OutputType::Html
        || (output_type == OutputType::None && lower_path.ends_with(".html"));

    if wants_pdf {
        progress_cb(0.8, "Composing PDF");
        create_pdf_from_text(title, plain_text, output_path)
    } else if wants_txt {
        progress_cb(0.8, "Writing text file");
        write_output_file(output_path, plain_text)
    } else if wants_html {
        progress_cb(0.8, "Writing HTML file");
        let body = match html_body {
            Some(html) => html.to_string(),
            None => format!("<pre>{}</pre>", escape_xml(plain_text)),
        };
        write_output_file(output_path, wrap_html(title, &body))
    } else if output_type.is_raster_image() {
        progress_cb(0.6, "Laying out document page");
        let svg = layout_text_as_svg(title, plain_text);
        crate::image::rasterize_svg_page(&svg, output_path, output_type, progress_cb)
    } else {
        Err(FileConverterError::Invalid(format!(
            "Cannot convert a document to '{}' output",
            output_type.extension()
        )))
    }
}

/// SIMD-accelerated HTML tag stripper using `memchr`
pub fn strip_html_tags(html: &str) -> String {
    let bytes = html.as_bytes();
    let mut result = String::with_capacity(html.len());
    let mut i = 0;

    while i < bytes.len() {
        if let Some(tag_start) = memchr::memchr(b'<', &bytes[i..]) {
            let abs_start = i + tag_start;
            if let Ok(text_slice) = std::str::from_utf8(&bytes[i..abs_start]) {
                result.push_str(text_slice);
            }
            if let Some(tag_end) = memchr::memchr(b'>', &bytes[abs_start..]) {
                i = abs_start + tag_end + 1;
            } else {
                if let Ok(remaining) = std::str::from_utf8(&bytes[abs_start..]) {
                    result.push_str(remaining);
                }
                break;
            }
        } else {
            if let Ok(remaining) = std::str::from_utf8(&bytes[i..]) {
                result.push_str(remaining);
            }
            break;
        }
    }
    result
}

fn wrap_html(title: &str, body: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{}</title>
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
            line-height: 1.6;
            color: #24292e;
            max-width: 860px;
            margin: 0 auto;
            padding: 2rem 1.5rem;
            background-color: #ffffff;
        }}
        @media (prefers-color-scheme: dark) {{
            body {{
                background-color: #0d1117;
                color: #c9d1d9;
            }}
            a {{ color: #58a6ff; }}
            code, pre {{ background-color: #161b22; }}
            blockquote {{ border-left-color: #30363d; color: #8b949e; }}
        }}
        img {{ max-width: 100%; height: auto; border-radius: 6px; }}
        code {{ font-family: SFMono-Regular, Consolas, "Liberation Mono", Menlo, monospace; padding: 0.2em 0.4em; background-color: #afb8c133; border-radius: 6px; font-size: 85%; }}
        pre {{ padding: 16px; overflow: auto; background-color: #f6f8fa; border-radius: 6px; }}
        blockquote {{ margin: 0; padding: 0 1em; color: #57606a; border-left: .25em solid #d0d7de; }}
        table {{ border-collapse: collapse; width: 100%; margin: 1em 0; }}
        th, td {{ border: 1px solid #d0d7de; padding: 6px 13px; }}
        tr:nth-child(2n) {{ background-color: #f6f8fa; }}
    </style>
</head>
<body>
{}
</body>
</html>"#,
        title, body
    )
}