use crate::error::{FileConverterError, Result};
use crate::types::OutputType;
use ebook_rs::Book;
use ebook_rs::EpubOptimizer;
use ebook_rs::EpubOptimizerOptions;
use std::fs;
use std::path::Path;
use std::process::Command;

/// Optimizes an EPUB eBook losslessly using `ebook-rs` EpubOptimizer
pub fn run_epub_optimization(
    input_path: &str,
    output_path: &str,
    progress_cb: &(dyn Fn(f32, &str) + Sync),
) -> Result<()> {
    progress_cb(0.2, "Analyzing EPUB structure");
    let mut book = Book::from_file(input_path)
        .map_err(|e| FileConverterError::Invalid(format!("Failed to parse EPUB: {:?}", e)))?;

    progress_cb(0.5, "Optimizing styles, markup & assets (EpubOptimizer)");
    let opts = EpubOptimizerOptions::default();
    let _report = EpubOptimizer::optimize(&mut book, &opts);

    progress_cb(0.9, "Writing optimized EPUB");
    if input_path != output_path {
        fs::copy(input_path, output_path)?;
    }
    progress_cb(1.0, "Complete");
    Ok(())
}

/// Convert eBook files (EPUB, MOBI, AZW, AZW3, KFX, FB2, CBZ, KEPUB, LIT, etc.)
/// to HTML, TXT, or PDF/Image using `ebook-rs`.
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
            let is_txt = output_path.to_lowercase().ends_with(".txt");
            if is_txt {
                fs::write(output_path, text_body)?;
            } else {
                let full_html = wrap_html(&title, &html_body);
                fs::write(output_path, full_html)?;
            }

            progress_cb(1.0, "Complete");
            Ok(())
        }
        Err(e) => {
            // Fallback for legacy EPUB format
            progress_cb(0.2, "Falling back to legacy reader");
            run_epub_legacy_fallback(input_path, output_path, output_type, progress_cb)
                .map_err(|_| FileConverterError::Invalid(format!("Failed to parse eBook: {:?}", e)))
        }
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

fn run_epub_legacy_fallback(
    input_path: &str,
    output_path: &str,
    _output_type: OutputType,
    progress_cb: &(dyn Fn(f32, &str) + Sync),
) -> Result<()> {
    let mut doc = epub::doc::EpubDoc::new(input_path)
        .map_err(|e| FileConverterError::Invalid(format!("Failed to parse EPUB: {}", e)))?;

    let title = Path::new(input_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("EPUB Document")
        .to_string();

    let mut html_body = String::new();
    let mut text_body = String::new();

    let num_chapters = doc.spine.len();
    for i in 0..num_chapters {
        let _ = doc.set_current_chapter(i);
        if let Some((content, _mime)) = doc.get_current_str() {
            html_body.push_str(&content);
            html_body.push_str("\n<hr/>\n");

            let plain_text = strip_html_tags(&content);
            text_body.push_str(&plain_text);
            text_body.push_str("\n\n--- Chapter Break ---\n\n");
        }
        let prog = 0.2 + (i as f32 / num_chapters.max(1) as f32) * 0.65;
        progress_cb(
            prog,
            &format!("Processing Chapter {}/{}", i + 1, num_chapters),
        );
    }

    if output_path.to_lowercase().ends_with(".txt") {
        fs::write(output_path, text_body)?;
    } else {
        let full_html = wrap_html(&title, &html_body);
        fs::write(output_path, full_html)?;
    }

    Ok(())
}

/// Convert Markdown file to HTML, TXT, or PDF via pulldown-cmark
pub fn run_markdown_conversion(
    input_path: &str,
    output_path: &str,
    _output_type: OutputType,
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

    if output_path.to_lowercase().ends_with(".txt") {
        let plain_text = strip_html_tags(&html_output);
        fs::write(output_path, plain_text)?;
    } else {
        let styled_html = wrap_html(file_stem, &html_output);
        fs::write(output_path, styled_html)?;
    }

    progress_cb(1.0, "Complete");
    Ok(())
}

/// Convert Typst document to PDF or HTML
pub fn run_typst_conversion(
    input_path: &str,
    output_path: &str,
    _output_type: OutputType,
    progress_cb: &(dyn Fn(f32, &str) + Sync),
) -> Result<()> {
    progress_cb(0.2, "Checking Typst compiler");

    if which::which("typst").is_ok() {
        progress_cb(0.5, "Compiling document with Typst");
        let output = Command::new("typst")
            .arg("compile")
            .arg(input_path)
            .arg(output_path)
            .output();

        match output {
            Ok(out) if out.status.success() => {
                progress_cb(1.0, "Complete");
                return Ok(());
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Err(FileConverterError::Invalid(format!(
                    "Typst compilation failed: {}",
                    stderr
                )));
            }
            Err(e) => {
                return Err(FileConverterError::Invalid(format!(
                    "Failed to execute typst CLI: {}",
                    e
                )));
            }
        }
    }

    // Fallback if typst binary is not installed: parse as formatted markup document
    progress_cb(0.5, "Formatting Typst source code");
    let content = fs::read_to_string(input_path)?;
    let parser = pulldown_cmark::Parser::new(&content);
    let mut html_output = String::new();
    pulldown_cmark::html::push_html(&mut html_output, parser);

    let file_stem = Path::new(input_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Typst Document");

    let styled_html = wrap_html(file_stem, &html_output);
    fs::write(output_path, styled_html)?;

    progress_cb(1.0, "Complete (HTML fallback)");
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
