//! Document conversion utilities
//!
//! Provides conversions:
//! - DOCX to PDF (via LibreOffice)
//! - Other document formats supported by LibreOffice

use super::{ConversionError, ConversionResult};
use std::path::Path;
use tokio::process::Command;

/// Supported input document formats
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DocumentFormat {
    Docx,
    Doc,
    Odt,
    Rtf,
    Txt,
    Md,
    Html,
    Rst,
    Epub,
    Tex,
}

impl DocumentFormat {
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_lowercase().as_str() {
            "docx" => Some(DocumentFormat::Docx),
            "doc" => Some(DocumentFormat::Doc),
            "odt" => Some(DocumentFormat::Odt),
            "rtf" => Some(DocumentFormat::Rtf),
            "txt" => Some(DocumentFormat::Txt),
            "md" | "markdown" => Some(DocumentFormat::Md),
            "html" | "htm" => Some(DocumentFormat::Html),
            "rst" => Some(DocumentFormat::Rst),
            "epub" => Some(DocumentFormat::Epub),
            "tex" | "latex" => Some(DocumentFormat::Tex),
            _ => None,
        }
    }

    pub fn is_convertible_to_pdf(&self) -> bool {
        // All supported formats can be converted to PDF
        true
    }
}

/// Convert document to PDF using LibreOffice
///
/// # Arguments
/// * `input_path` - Path to input document file
///
/// # Returns
/// Path to the converted PDF file
///
/// # Requirements
/// LibreOffice must be installed and accessible via `libreoffice` command
pub async fn to_pdf<P: AsRef<Path>>(input_path: P) -> ConversionResult<std::path::PathBuf> {
    let input = input_path.as_ref();

    if !input.exists() {
        return Err(ConversionError::InputNotFound(input.display().to_string()));
    }

    // Check if format is supported
    let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");

    if DocumentFormat::from_extension(ext).is_none() {
        return Err(ConversionError::UnsupportedFormat(format!(
            "Unsupported document format: {}",
            ext
        )));
    }

    // Create temp output directory
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let rand: u32 = rand::random();
    let output_dir = std::path::PathBuf::from(format!("/tmp/libreoffice_{}_{:x}", timestamp, rand));

    tokio::fs::create_dir_all(&output_dir).await?;

    // Run LibreOffice headless conversion
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        Command::new("libreoffice")
            .args(["--headless", "--convert-to", "pdf", "--outdir"])
            .arg(&output_dir)
            .arg(input)
            .output(),
    )
    .await
    .map_err(|_| ConversionError::LibreOfficeError("LibreOffice timed out after 120s".to_string()))?
    .map_err(|e| ConversionError::LibreOfficeError(format!("Failed to run LibreOffice: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::error!("LibreOffice conversion error: {}", stderr);
        // Clean up temp directory
        let _ = tokio::fs::remove_dir_all(&output_dir).await;
        return Err(ConversionError::LibreOfficeError(stderr.to_string()));
    }

    // Find the output PDF file
    let input_stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("document");
    let pdf_path = output_dir.join(format!("{}.pdf", input_stem));

    if !pdf_path.exists() {
        // Clean up temp directory
        let _ = tokio::fs::remove_dir_all(&output_dir).await;
        return Err(ConversionError::OutputFailed("PDF file was not created".to_string()));
    }

    // Move to standard temp location
    let final_path = std::path::PathBuf::from(format!("/tmp/converted_{}_{:x}.pdf", timestamp, rand));
    tokio::fs::rename(&pdf_path, &final_path).await?;

    // Clean up temp directory
    let _ = tokio::fs::remove_dir_all(&output_dir).await;

    Ok(final_path)
}

/// Convert document to PDF using Pandoc
///
/// Supports: Markdown, HTML, RST, EPUB, LaTeX
///
/// # Requirements
/// Pandoc must be installed and accessible via `pandoc` command
pub async fn pandoc_to_pdf<P: AsRef<Path>>(input_path: P) -> ConversionResult<std::path::PathBuf> {
    let input = input_path.as_ref();

    if !input.exists() {
        return Err(ConversionError::InputNotFound(input.display().to_string()));
    }

    let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
    let format = DocumentFormat::from_extension(ext);

    // Pandoc handles: md, html, rst, epub, tex
    let is_pandoc_format = matches!(
        format,
        Some(DocumentFormat::Md)
            | Some(DocumentFormat::Html)
            | Some(DocumentFormat::Rst)
            | Some(DocumentFormat::Epub)
            | Some(DocumentFormat::Tex)
    );

    if !is_pandoc_format {
        return Err(ConversionError::UnsupportedFormat(format!(
            "Pandoc does not support: {}",
            ext
        )));
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let rand: u32 = rand::random();
    let output_path = std::path::PathBuf::from(format!("/tmp/pandoc_{}_{:x}.pdf", timestamp, rand));

    let output = Command::new("pandoc")
        .arg(input)
        .arg("-o")
        .arg(&output_path)
        .arg("--pdf-engine=xelatex")
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::error!("Pandoc conversion error: {}", stderr);
        // Clean up on failure
        let _ = tokio::fs::remove_file(&output_path).await;
        return Err(ConversionError::FfmpegError(format!("Pandoc error: {}", stderr)));
    }

    if !output_path.exists() {
        return Err(ConversionError::OutputFailed(
            "PDF file was not created by pandoc".to_string(),
        ));
    }

    Ok(output_path)
}

/// Check if pandoc is available
pub async fn check_pandoc() -> bool {
    Command::new("pandoc")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if a file format is supported for Pandoc PDF conversion
pub fn is_pandoc_supported<P: AsRef<Path>>(path: P) -> bool {
    path.as_ref()
        .extension()
        .and_then(|e| e.to_str())
        .and_then(DocumentFormat::from_extension)
        .map(|f| {
            matches!(
                f,
                DocumentFormat::Md
                    | DocumentFormat::Html
                    | DocumentFormat::Rst
                    | DocumentFormat::Epub
                    | DocumentFormat::Tex
            )
        })
        .unwrap_or(false)
}

/// Check if a file format is supported for PDF conversion
pub fn is_supported_for_pdf<P: AsRef<Path>>(path: P) -> bool {
    path.as_ref()
        .extension()
        .and_then(|e| e.to_str())
        .and_then(DocumentFormat::from_extension)
        .map(|f| f.is_convertible_to_pdf())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_document_format_from_extension() {
        assert_eq!(DocumentFormat::from_extension("docx"), Some(DocumentFormat::Docx));
        assert_eq!(DocumentFormat::from_extension("DOCX"), Some(DocumentFormat::Docx));
        assert_eq!(DocumentFormat::from_extension("doc"), Some(DocumentFormat::Doc));
        assert_eq!(DocumentFormat::from_extension("odt"), Some(DocumentFormat::Odt));
        assert_eq!(DocumentFormat::from_extension("rtf"), Some(DocumentFormat::Rtf));
        assert_eq!(DocumentFormat::from_extension("txt"), Some(DocumentFormat::Txt));
        assert_eq!(DocumentFormat::from_extension("md"), Some(DocumentFormat::Md));
        assert_eq!(DocumentFormat::from_extension("markdown"), Some(DocumentFormat::Md));
        assert_eq!(DocumentFormat::from_extension("html"), Some(DocumentFormat::Html));
        assert_eq!(DocumentFormat::from_extension("htm"), Some(DocumentFormat::Html));
        assert_eq!(DocumentFormat::from_extension("rst"), Some(DocumentFormat::Rst));
        assert_eq!(DocumentFormat::from_extension("epub"), Some(DocumentFormat::Epub));
        assert_eq!(DocumentFormat::from_extension("tex"), Some(DocumentFormat::Tex));
        assert_eq!(DocumentFormat::from_extension("latex"), Some(DocumentFormat::Tex));
        assert_eq!(DocumentFormat::from_extension("pdf"), None);
        assert_eq!(DocumentFormat::from_extension("xlsx"), None);
    }

    #[test]
    fn test_document_format_case_insensitive() {
        assert_eq!(DocumentFormat::from_extension("DOCX"), Some(DocumentFormat::Docx));
        assert_eq!(DocumentFormat::from_extension("Md"), Some(DocumentFormat::Md));
        assert_eq!(DocumentFormat::from_extension("HTML"), Some(DocumentFormat::Html));
        assert_eq!(DocumentFormat::from_extension("HTM"), Some(DocumentFormat::Html));
        assert_eq!(DocumentFormat::from_extension("TEX"), Some(DocumentFormat::Tex));
        assert_eq!(DocumentFormat::from_extension("LATEX"), Some(DocumentFormat::Tex));
        assert_eq!(DocumentFormat::from_extension("EPUB"), Some(DocumentFormat::Epub));
    }

    #[test]
    fn test_all_formats_convertible_to_pdf() {
        let formats = [
            DocumentFormat::Docx,
            DocumentFormat::Doc,
            DocumentFormat::Odt,
            DocumentFormat::Rtf,
            DocumentFormat::Txt,
            DocumentFormat::Md,
            DocumentFormat::Html,
            DocumentFormat::Rst,
            DocumentFormat::Epub,
            DocumentFormat::Tex,
        ];
        for fmt in &formats {
            assert!(fmt.is_convertible_to_pdf(), "{:?} should be convertible to PDF", fmt);
        }
    }

    #[test]
    fn test_is_supported_for_pdf() {
        assert!(is_supported_for_pdf("/tmp/test.docx"));
        assert!(is_supported_for_pdf("/tmp/test.doc"));
        assert!(is_supported_for_pdf("/tmp/test.odt"));
        assert!(is_supported_for_pdf("/tmp/test.md"));
        assert!(is_supported_for_pdf("/tmp/test.html"));
        assert!(is_supported_for_pdf("/tmp/test.rst"));
        assert!(is_supported_for_pdf("/tmp/test.epub"));
        assert!(is_supported_for_pdf("/tmp/test.tex"));
        assert!(is_supported_for_pdf("/tmp/test.rtf"));
        assert!(is_supported_for_pdf("/tmp/test.txt"));
        assert!(!is_supported_for_pdf("/tmp/test.xlsx"));
        assert!(!is_supported_for_pdf("/tmp/test.pdf"));
        assert!(!is_supported_for_pdf("/tmp/test"));
    }

    #[test]
    fn test_is_pandoc_supported() {
        assert!(is_pandoc_supported("/tmp/test.md"));
        assert!(is_pandoc_supported("/tmp/test.html"));
        assert!(is_pandoc_supported("/tmp/test.htm"));
        assert!(is_pandoc_supported("/tmp/test.rst"));
        assert!(is_pandoc_supported("/tmp/test.epub"));
        assert!(is_pandoc_supported("/tmp/test.tex"));
        assert!(is_pandoc_supported("/tmp/test.latex"));
        // LibreOffice formats should NOT be pandoc-supported
        assert!(!is_pandoc_supported("/tmp/test.docx"));
        assert!(!is_pandoc_supported("/tmp/test.doc"));
        assert!(!is_pandoc_supported("/tmp/test.odt"));
        assert!(!is_pandoc_supported("/tmp/test.rtf"));
        assert!(!is_pandoc_supported("/tmp/test.txt"));
        assert!(!is_pandoc_supported("/tmp/test.pdf"));
        assert!(!is_pandoc_supported("/tmp/test.xlsx"));
        assert!(!is_pandoc_supported("/tmp/test"));
    }

    #[test]
    fn test_pandoc_and_libreoffice_partition() {
        // Pandoc formats: md, html, rst, epub, tex
        // LibreOffice formats: docx, doc, odt, rtf, txt
        // Together they should cover all DocumentFormat variants
        let pandoc_exts = ["md", "html", "rst", "epub", "tex"];
        let libre_exts = ["docx", "doc", "odt", "rtf", "txt"];

        for ext in &pandoc_exts {
            let path = format!("/tmp/test.{}", ext);
            assert!(is_pandoc_supported(&path), "{} should be pandoc-supported", ext);
            assert!(is_supported_for_pdf(&path), "{} should be PDF-supported", ext);
        }
        for ext in &libre_exts {
            let path = format!("/tmp/test.{}", ext);
            assert!(!is_pandoc_supported(&path), "{} should NOT be pandoc-supported", ext);
            assert!(is_supported_for_pdf(&path), "{} should be PDF-supported", ext);
        }
    }

    #[tokio::test]
    async fn test_to_pdf_input_not_found() {
        let result = to_pdf("/tmp/nonexistent_doc_file_12345.docx").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ConversionError::InputNotFound(path) => {
                assert!(path.contains("nonexistent_doc_file_12345"));
            }
            other => panic!("Expected InputNotFound, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_pandoc_to_pdf_input_not_found() {
        let result = pandoc_to_pdf("/tmp/nonexistent_md_file_12345.md").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ConversionError::InputNotFound(path) => {
                assert!(path.contains("nonexistent_md_file_12345"));
            }
            other => panic!("Expected InputNotFound, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_pandoc_to_pdf_unsupported_format() {
        // Create a temp file with an unsupported extension
        let path = "/tmp/test_pandoc_unsupported.docx";
        tokio::fs::write(path, "test content").await.unwrap();

        let result = pandoc_to_pdf(path).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ConversionError::UnsupportedFormat(msg) => {
                assert!(msg.contains("docx"), "Error should mention format: {}", msg);
            }
            other => panic!("Expected UnsupportedFormat, got: {:?}", other),
        }

        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn test_to_pdf_unsupported_extension() {
        let path = "/tmp/test_libre_unsupported.xyz";
        tokio::fs::write(path, "test content").await.unwrap();

        let result = to_pdf(path).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ConversionError::UnsupportedFormat(msg) => {
                assert!(msg.contains("xyz"), "Error should mention format: {}", msg);
            }
            other => panic!("Expected UnsupportedFormat, got: {:?}", other),
        }

        let _ = tokio::fs::remove_file(path).await;
    }
}
