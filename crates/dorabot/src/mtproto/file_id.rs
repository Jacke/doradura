//! Bot API file_id decoder
//!
//! Decodes Telegram Bot API file_id strings into their constituent parts
//! that can be used with MTProto API.
//!
//! Based on: https://github.com/pyrogram/pyrogram/blob/master/pyrogram/file_id.py

use super::error::MtProtoError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

/// File type enumeration matching Telegram's internal types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FileType {
    Thumbnail = 0,
    ProfilePhoto = 1,
    Photo = 2,
    Voice = 3,
    Video = 4,
    Document = 5,
    Encrypted = 6,
    Temp = 7,
    Sticker = 8,
    Audio = 9,
    Animation = 10,
    EncryptedThumbnail = 11,
    Wallpaper = 12,
    VideoNote = 13,
    SecureRaw = 14,
    Secure = 15,
    Background = 16,
    DocumentAsFile = 17,
    Ringtone = 18,
    CallLog = 19,
    PhotoStory = 20,
    VideoStory = 21,
    Unknown = 255,
}

impl From<i32> for FileType {
    fn from(value: i32) -> Self {
        match value {
            0 => FileType::Thumbnail,
            1 => FileType::ProfilePhoto,
            2 => FileType::Photo,
            3 => FileType::Voice,
            4 => FileType::Video,
            5 => FileType::Document,
            6 => FileType::Encrypted,
            7 => FileType::Temp,
            8 => FileType::Sticker,
            9 => FileType::Audio,
            10 => FileType::Animation,
            11 => FileType::EncryptedThumbnail,
            12 => FileType::Wallpaper,
            13 => FileType::VideoNote,
            14 => FileType::SecureRaw,
            15 => FileType::Secure,
            16 => FileType::Background,
            17 => FileType::DocumentAsFile,
            18 => FileType::Ringtone,
            19 => FileType::CallLog,
            20 => FileType::PhotoStory,
            21 => FileType::VideoStory,
            _ => FileType::Unknown,
        }
    }
}

impl FileType {
    /// Check if this file type is a document-like type (uses InputDocumentFileLocation)
    pub fn is_document(&self) -> bool {
        matches!(
            self,
            FileType::Document
                | FileType::Audio
                | FileType::Video
                | FileType::Voice
                | FileType::VideoNote
                | FileType::Animation
                | FileType::Sticker
                | FileType::DocumentAsFile
                | FileType::Ringtone
        )
    }

    /// Check if this file type is a photo-like type (uses InputPhotoFileLocation)
    pub fn is_photo(&self) -> bool {
        matches!(
            self,
            FileType::Photo | FileType::ProfilePhoto | FileType::Thumbnail | FileType::PhotoStory
        )
    }
}

/// Flags embedded in the type field
const WEB_LOCATION_FLAG: i32 = 1 << 24;
const FILE_REFERENCE_FLAG: i32 = 1 << 25;

/// Decoded Bot API file_id structure
#[derive(Debug, Clone)]
pub struct DecodedFileId {
    /// File ID major version (typically 4)
    pub version: u8,
    /// Sub-version for tdlib compatibility
    pub sub_version: u8,
    /// Datacenter ID where file is stored
    pub dc_id: i32,
    /// File type
    pub file_type: FileType,
    /// Unique file identifier (media_id)
    pub id: i64,
    /// Access hash for authorization
    pub access_hash: i64,
    /// File reference bytes (may expire)
    pub file_reference: Vec<u8>,
    /// Photo size type (for photos only)
    pub photo_size_type: Option<String>,
    /// Volume ID (for old photo format)
    pub volume_id: Option<i64>,
    /// Local ID (for old photo format)
    pub local_id: Option<i32>,
}

impl DecodedFileId {
    /// Decode a Bot API file_id string
    pub fn decode(file_id: &str) -> Result<Self, MtProtoError> {
        // Step 1: Base64 decode with URL-safe alphabet
        let decoded = URL_SAFE_NO_PAD
            .decode(file_id)
            .map_err(|e| MtProtoError::FileIdDecode(format!("Base64 decode failed: {}", e)))?;

        if decoded.len() < 8 {
            return Err(MtProtoError::FileIdDecode("Data too short".to_string()));
        }

        // Step 2: RLE decode
        let data = rle_decode(&decoded);

        if data.len() < 8 {
            return Err(MtProtoError::FileIdDecode(format!(
                "RLE decoded data too short: {} bytes",
                data.len()
            )));
        }

        // Step 3: Read version from the end
        let (version, sub_version, data_end) = if data.len() >= 2 {
            let major = data[data.len() - 1];
            if major >= 4 {
                let minor = data[data.len() - 2];
                (major, minor, data.len() - 2)
            } else {
                (major, 0u8, data.len() - 1)
            }
        } else {
            (2u8, 0u8, data.len())
        };

        let data = &data[..data_end];

        // Step 4: Parse the structure
        let mut reader = ByteReader::new(data);

        // Read type_id and dc_id as two i32 (little endian)
        let type_id = reader.read_i32_le()?;
        let dc_id = reader.read_i32_le()?;

        // Extract flags from type_id
        let has_web_location = (type_id & WEB_LOCATION_FLAG) != 0;
        let has_file_reference = (type_id & FILE_REFERENCE_FLAG) != 0;

        // Get actual file type (lower bits)
        let file_type = FileType::from(type_id & 0xFF);

        if has_web_location {
            return Err(MtProtoError::FileIdDecode(
                "Web location file_ids not supported".to_string(),
            ));
        }

        // Read file reference if present
        let file_reference = if has_file_reference {
            read_tl_bytes(&mut reader)?
        } else {
            Vec::new()
        };

        // Read media_id and access_hash
        let id = reader.read_i64_le()?;
        let access_hash = reader.read_i64_le()?;

        // For photo types, try to read additional fields
        let (volume_id, local_id, photo_size_type) = if file_type.is_photo() && reader.remaining() >= 8 {
            let vol = reader.read_i64_le().ok();
            // There may be thumbnail_source and other fields, but we mainly need volume_id
            let local = if reader.remaining() >= 4 {
                reader.read_i32_le().ok()
            } else {
                None
            };
            // Photo size type (like 'x', 'y', 'w' etc)
            let size_type = if reader.remaining() >= 1 {
                reader.read_u8().ok().map(|b| (b as char).to_string())
            } else {
                None
            };
            (vol, local, size_type)
        } else {
            (None, None, None)
        };

        Ok(DecodedFileId {
            version,
            sub_version,
            dc_id,
            file_type,
            id,
            access_hash,
            file_reference,
            photo_size_type,
            volume_id,
            local_id,
        })
    }

    /// Convert to InputDocumentFileLocation for MTProto download
    pub fn to_input_document_location(&self) -> grammers_tl_types::enums::InputFileLocation {
        grammers_tl_types::enums::InputFileLocation::InputDocumentFileLocation(
            grammers_tl_types::types::InputDocumentFileLocation {
                id: self.id,
                access_hash: self.access_hash,
                file_reference: self.file_reference.clone(),
                thumb_size: String::new(),
            },
        )
    }

    /// Convert to InputPhotoFileLocation for MTProto download
    pub fn to_input_photo_location(&self) -> grammers_tl_types::enums::InputFileLocation {
        grammers_tl_types::enums::InputFileLocation::InputPhotoFileLocation(
            grammers_tl_types::types::InputPhotoFileLocation {
                id: self.id,
                access_hash: self.access_hash,
                file_reference: self.file_reference.clone(),
                thumb_size: self.photo_size_type.clone().unwrap_or_default(),
            },
        )
    }

    /// Get the appropriate InputFileLocation based on file type
    pub fn to_input_file_location(&self) -> grammers_tl_types::enums::InputFileLocation {
        if self.file_type.is_document() {
            self.to_input_document_location()
        } else {
            self.to_input_photo_location()
        }
    }
}

/// RLE decode Telegram's custom format
/// Zeros are encoded as 0x00 followed by count
fn rle_decode(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len());
    let mut i = 0;

    while i < data.len() {
        if data[i] == 0 && i + 1 < data.len() {
            // Zero followed by count
            let count = data[i + 1] as usize;
            result.extend(std::iter::repeat_n(0u8, count));
            i += 2;
        } else {
            result.push(data[i]);
            i += 1;
        }
    }

    result
}

/// Read TL-style bytes (length-prefixed)
fn read_tl_bytes(reader: &mut ByteReader) -> Result<Vec<u8>, MtProtoError> {
    let first_byte = reader.read_u8()?;

    let len = if first_byte < 254 {
        first_byte as usize
    } else {
        // Read 3 more bytes for length
        let b1 = reader.read_u8()? as usize;
        let b2 = reader.read_u8()? as usize;
        let b3 = reader.read_u8()? as usize;
        b1 | (b2 << 8) | (b3 << 16)
    };

    let bytes = reader.read_bytes(len)?;

    // Skip padding to align to 4 bytes
    let total_read = if first_byte < 254 { 1 + len } else { 4 + len };
    let padding = (4 - (total_read % 4)) % 4;
    for _ in 0..padding {
        let _ = reader.read_u8();
    }

    Ok(bytes)
}

/// Simple byte reader for parsing binary data
struct ByteReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ByteReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn read_u8(&mut self) -> Result<u8, MtProtoError> {
        if self.pos >= self.data.len() {
            return Err(MtProtoError::FileIdDecode("Unexpected end of data".to_string()));
        }
        let val = self.data[self.pos];
        self.pos += 1;
        Ok(val)
    }

    fn read_i32_le(&mut self) -> Result<i32, MtProtoError> {
        if self.pos + 4 > self.data.len() {
            return Err(MtProtoError::FileIdDecode("Unexpected end of data".to_string()));
        }
        let val = i32::from_le_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ]);
        self.pos += 4;
        Ok(val)
    }

    fn read_i64_le(&mut self) -> Result<i64, MtProtoError> {
        if self.pos + 8 > self.data.len() {
            return Err(MtProtoError::FileIdDecode("Unexpected end of data".to_string()));
        }
        let val = i64::from_le_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
            self.data[self.pos + 4],
            self.data[self.pos + 5],
            self.data[self.pos + 6],
            self.data[self.pos + 7],
        ]);
        self.pos += 8;
        Ok(val)
    }

    fn read_bytes(&mut self, count: usize) -> Result<Vec<u8>, MtProtoError> {
        if self.pos + count > self.data.len() {
            return Err(MtProtoError::FileIdDecode("Unexpected end of data".to_string()));
        }
        let bytes = self.data[self.pos..self.pos + count].to_vec();
        self.pos += count;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rle_decode_no_zeros() {
        let input = vec![1, 2, 3, 4];
        let output = rle_decode(&input);
        assert_eq!(output, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_rle_decode_with_zeros() {
        let input = vec![1, 0, 3, 2]; // 1, then 3 zeros, then 2
        let output = rle_decode(&input);
        assert_eq!(output, vec![1, 0, 0, 0, 2]);
    }

    #[test]
    fn test_file_type_is_document() {
        assert!(FileType::Document.is_document());
        assert!(FileType::Audio.is_document());
        assert!(FileType::Video.is_document());
        assert!(FileType::VideoNote.is_document());
        assert!(!FileType::Photo.is_document());
    }

    #[test]
    fn test_file_type_is_photo() {
        assert!(FileType::Photo.is_photo());
        assert!(FileType::ProfilePhoto.is_photo());
        assert!(FileType::Thumbnail.is_photo());
        assert!(!FileType::Document.is_photo());
    }
}
