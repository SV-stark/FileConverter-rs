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
/// to HTML, TXT, or PDF using `ebook-rs`.
pub fn run_ebook_conversion(
    input_path: &str,
    output_path: &str,
    output_type: OutputType,
    progress_cb: &(dyn Fn(f32, &str) + Sync),
) -> Result<()> {
    progress_cb(0.1, "Opening eBook document (ebook-rs)");

    match Book::from_file(input_path) {
        Ok(book) => {
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
            let total_sections = sections.len();

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

                let prog = 0.25 + (i as f32 / total_sections.max(1) as f32) * 0.6;
                progress_cb(
                    prog,
                    &format!("Processing Section {}/{}", i + 1, total_sections),
                );
            }

            progress_cb(0.9, "Writing output file");
            let is_pdf =
                output_type == OutputType::Pdf || output_path.to_lowercase().ends_with(".pdf");
            let is_txt =
                output_type == OutputType::None && output_path.to_lowercase().ends_with(".txt");

            if is_pdf {
                create_pdf_from_text(&title, &text_body, output_path)?;
            } else if is_txt {
                write_output_file(output_path, text_body)?;
            } else {
                let full_html = wrap_html(&title, &html_body);
                write_output_file(output_path, full_html)?;
            }

            progress_cb(1.0, "Complete");
            Ok(())
        }
        Err(e) => Err(FileConverterError::Invalid(format!(
            "Failed to parse eBook: {:?}",
            e
        ))),
    }
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

/// Convert Markdown file to HTML, TXT, or PDF via pulldown-cmark
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
        .unwrap_or("Document");

    let is_pdf = output_type == OutputType::Pdf || output_path.to_lowercase().ends_with(".pdf");
    let is_txt = output_type == OutputType::None && output_path.to_lowercase().ends_with(".txt");

    if is_pdf {
        let plain_text = strip_html_tags(&html_output);
        create_pdf_from_text(file_stem, &plain_text, output_path)?;
    } else if is_txt {
        let plain_text = strip_html_tags(&html_output);
        write_output_file(output_path, plain_text)?;
    } else {
        let styled_html = wrap_html(file_stem, &html_output);
        write_output_file(output_path, styled_html)?;
    }

    progress_cb(1.0, "Complete");
    Ok(())
}

/// Convert Typst document to PDF or HTML
pub fn run_typst_conversion(
    input_path: &str,
    output_path: &str,
    output_type: OutputType,
    progress_cb: &(dyn Fn(f32, &str) + Sync),
) -> Result<()> {
    progress_cb(0.2, "Checking Typst compiler");

    let is_pdf = output_type == OutputType::Pdf || output_path.to_lowercase().ends_with(".pdf");

    if let Some(typst_exe) = crate::path_helpers::find_in_path("typst") {
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
        let timeout = std::time::Duration::from_secs(60);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if start.elapsed() > timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(FileConverterError::Timeout(
                            "Typst compilation timed out after 60s".to_string(),
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
        } else {
            let mut stderr_str = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr_str);
            }
            return Err(FileConverterError::Invalid(format!(
                "Typst compilation failed: {}",
                stderr_str
            )));
        }
    }

    // Fallback if typst binary is not installed: parse source as text/markup
    progress_cb(0.5, "Formatting Typst source code");
    let content = fs::read_to_string(input_path)?;
    let parser = pulldown_cmark::Parser::new(&content);
    let mut html_output = String::new();
    pulldown_cmark::html::push_html(&mut html_output, parser);

    let file_stem = Path::new(input_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Typst Document");

    if is_pdf {
        let plain = strip_html_tags(&html_output);
        create_pdf_from_text(file_stem, &plain, output_path)?;
    } else {
        let styled_html = wrap_html(file_stem, &html_output);
        write_output_file(output_path, styled_html)?;
    }

    progress_cb(1.0, "Complete (Fallback)");
    Ok(())
}

/// SIMD-accelerated HTML tag stripper using `memchr`
pub(crate) fn strip_html_tags(html: &str) -> String {
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
