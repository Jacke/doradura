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
}

impl DocumentFormat {
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_lowercase().as_str() {
            "docx" => Some(DocumentFormat::Docx),
            "doc" => Some(DocumentFormat::Doc),
            "odt" => Some(DocumentFormat::Odt),
            "rtf" => Some(DocumentFormat::Rtf),
            "txt" => Some(DocumentFormat::Txt),
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
    let output = Command::new("libreoffice")
        .args(["--headless", "--convert-to", "pdf", "--outdir"])
        .arg(&output_dir)
        .arg(input)
        .output()
        .await?;

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
        assert_eq!(DocumentFormat::from_extension("pdf"), None);
        assert_eq!(DocumentFormat::from_extension("xlsx"), None);
    }

    #[test]
    fn test_is_supported_for_pdf() {
        assert!(is_supported_for_pdf("/tmp/test.docx"));
        assert!(is_supported_for_pdf("/tmp/test.doc"));
        assert!(is_supported_for_pdf("/tmp/test.odt"));
        assert!(!is_supported_for_pdf("/tmp/test.xlsx"));
        assert!(!is_supported_for_pdf("/tmp/test.pdf"));
    }
}
