//! MTProto file downloader

use super::client::MtProtoClient;
use super::error::MtProtoError;
use super::file_id::DecodedFileId;
use grammers_tl_types as tl;
use std::path::Path;
use tokio::io::AsyncWriteExt;

/// Chunk size for file downloads (1MB)
const CHUNK_SIZE: i64 = 1024 * 1024;

/// Bot API response for getFile
#[derive(Debug, serde::Deserialize)]
struct BotApiResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

/// Bot API File object
#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
struct BotApiFile {
    file_id: String,
    file_unique_id: String,
    file_size: Option<i64>,
    file_path: Option<String>,
}

/// MTProto-based file downloader
pub struct MtProtoDownloader {
    client: MtProtoClient,
    bot_token: Option<String>,
}

impl MtProtoDownloader {
    /// Create a new downloader with the given client
    pub fn new(client: MtProtoClient) -> Self {
        Self {
            client,
            bot_token: None,
        }
    }

    /// Create a new downloader with Bot API token for file_reference refresh
    pub fn with_bot_token(client: MtProtoClient, bot_token: String) -> Self {
        Self {
            client,
            bot_token: Some(bot_token),
        }
    }

    /// Refresh file_reference by calling Bot API getFile
    /// This returns a fresh file_id with updated file_reference
    async fn refresh_file_reference(&self, file_id: &str) -> Result<String, MtProtoError> {
        let bot_token = self.bot_token.as_ref().ok_or(MtProtoError::FileReferenceExpired)?;

        let url = format!("https://api.telegram.org/bot{}/getFile?file_id={}", bot_token, file_id);

        let response = reqwest::get(&url)
            .await
            .map_err(|e| MtProtoError::Session(format!("Bot API request failed: {}", e)))?;

        let api_response: BotApiResponse<BotApiFile> = response
            .json()
            .await
            .map_err(|e| MtProtoError::Session(format!("Failed to parse Bot API response: {}", e)))?;

        if !api_response.ok {
            return Err(MtProtoError::Session(format!(
                "Bot API error: {}",
                api_response.description.unwrap_or_default()
            )));
        }

        let file = api_response
            .result
            .ok_or_else(|| MtProtoError::Session("Bot API returned no file".to_string()))?;

        log::info!("Refreshed file_id, file_path: {:?}", file.file_path);
        Ok(file.file_id)
    }

    /// Download a file by Bot API file_id
    ///
    /// This decodes the file_id and downloads directly via MTProto.
    /// If file_reference is expired, it will try to refresh it via Bot API.
    ///
    /// # Arguments
    /// * `file_id` - Bot API file_id string
    /// * `output_path` - Path to save the downloaded file
    ///
    /// # Returns
    /// Number of bytes downloaded
    pub async fn download_by_file_id(&self, file_id: &str, output_path: &Path) -> Result<u64, MtProtoError> {
        // Try to download, if FILE_REFERENCE_EXPIRED - refresh and retry
        match self.download_by_file_id_inner(file_id, output_path).await {
            Ok(size) => Ok(size),
            Err(MtProtoError::Invocation(ref e)) if e.to_string().contains("FILE_REFERENCE_EXPIRED") => {
                log::warn!("File reference expired, trying to refresh via Bot API...");

                // Try to refresh file_reference
                let refreshed_file_id = self.refresh_file_reference(file_id).await?;

                // Retry with refreshed file_id
                self.download_by_file_id_inner(&refreshed_file_id, output_path).await
            }
            Err(e) => Err(e),
        }
    }

    /// Internal download implementation
    async fn download_by_file_id_inner(&self, file_id: &str, output_path: &Path) -> Result<u64, MtProtoError> {
        log::info!("Decoding file_id: {}...", &file_id[..20.min(file_id.len())]);

        // Decode the file_id
        let decoded = DecodedFileId::decode(file_id)?;
        log::info!(
            "Decoded: type={:?}, dc={}, id={}, access_hash={}",
            decoded.file_type,
            decoded.dc_id,
            decoded.id,
            decoded.access_hash
        );

        // Get the appropriate InputFileLocation
        let location = decoded.to_input_file_location();

        // Download the file
        let bytes = self.download_file_location(location, decoded.dc_id).await?;

        // Write to file
        let mut file = tokio::fs::File::create(output_path).await?;
        file.write_all(&bytes).await?;
        file.flush().await?;

        log::info!("Downloaded {} bytes to {:?}", bytes.len(), output_path);

        Ok(bytes.len() as u64)
    }

    /// Download a file by chat_id and message_id
    ///
    /// This is an alternative approach that doesn't require file_id decoding.
    /// It fetches the message and downloads any attached media.
    ///
    /// # Arguments
    /// * `chat_id` - Telegram chat/user ID
    /// * `message_id` - Message ID containing the media
    /// * `output_path` - Path to save the downloaded file
    ///
    /// # Returns
    /// Number of bytes downloaded
    pub async fn download_by_message(
        &self,
        chat_id: i64,
        message_id: i32,
        output_path: &Path,
    ) -> Result<u64, MtProtoError> {
        log::info!("Downloading from chat {} message {}", chat_id, message_id);

        // Resolve the peer (chat/user)
        let _input_peer = tl::enums::InputPeer::User(tl::types::InputPeerUser {
            user_id: chat_id,
            access_hash: 0, // For users who messaged the bot, access_hash of 0 works
        });

        // Get the message
        let messages = self
            .client
            .inner()
            .invoke(&tl::functions::messages::GetMessages {
                id: vec![tl::enums::InputMessage::Id(tl::types::InputMessageId {
                    id: message_id,
                })],
            })
            .await
            .map_err(MtProtoError::Invocation)?;

        // Extract the message
        let message = match messages {
            tl::enums::messages::Messages::Messages(m) => m.messages.into_iter().next(),
            tl::enums::messages::Messages::Slice(m) => m.messages.into_iter().next(),
            tl::enums::messages::Messages::ChannelMessages(m) => m.messages.into_iter().next(),
            tl::enums::messages::Messages::NotModified(_) => None,
        };

        let message = message.ok_or(MtProtoError::MessageNotFound)?;

        // Extract media from message
        let media = match message {
            tl::enums::Message::Message(m) => m.media,
            _ => None,
        };

        let media = media.ok_or(MtProtoError::NoMediaInMessage)?;

        // Download based on media type
        match media {
            tl::enums::MessageMedia::Document(doc_media) => {
                if let Some(tl::enums::Document::Document(doc)) = doc_media.document {
                    let location =
                        tl::enums::InputFileLocation::InputDocumentFileLocation(tl::types::InputDocumentFileLocation {
                            id: doc.id,
                            access_hash: doc.access_hash,
                            file_reference: doc.file_reference,
                            thumb_size: String::new(),
                        });

                    let bytes = self.download_file_location(location, doc.dc_id).await?;

                    let mut file = tokio::fs::File::create(output_path).await?;
                    file.write_all(&bytes).await?;
                    file.flush().await?;

                    log::info!("Downloaded {} bytes", bytes.len());
                    return Ok(bytes.len() as u64);
                }
            }
            tl::enums::MessageMedia::Photo(photo_media) => {
                if let Some(tl::enums::Photo::Photo(photo)) = photo_media.photo {
                    // Get the largest photo size
                    let size = photo
                        .sizes
                        .iter()
                        .filter_map(|s| match s {
                            tl::enums::PhotoSize::Size(ps) => Some(ps),
                            _ => None,
                        })
                        .max_by_key(|s| s.size);

                    if let Some(size) = size {
                        let location =
                            tl::enums::InputFileLocation::InputPhotoFileLocation(tl::types::InputPhotoFileLocation {
                                id: photo.id,
                                access_hash: photo.access_hash,
                                file_reference: photo.file_reference.clone(),
                                thumb_size: size.r#type.clone(),
                            });

                        let bytes = self.download_file_location(location, photo.dc_id).await?;

                        let mut file = tokio::fs::File::create(output_path).await?;
                        file.write_all(&bytes).await?;
                        file.flush().await?;

                        log::info!("Downloaded {} bytes", bytes.len());
                        return Ok(bytes.len() as u64);
                    }
                }
            }
            _ => {}
        }

        Err(MtProtoError::NoMediaInMessage)
    }

    /// Download file by InputFileLocation using upload.getFile
    async fn download_file_location(
        &self,
        location: tl::enums::InputFileLocation,
        _dc_id: i32,
    ) -> Result<Vec<u8>, MtProtoError> {
        let mut data = Vec::new();
        let mut offset = 0i64;

        log::info!("Starting chunked download...");

        loop {
            let result = self
                .client
                .inner()
                .invoke(&tl::functions::upload::GetFile {
                    precise: false,
                    cdn_supported: false,
                    location: location.clone(),
                    offset,
                    limit: CHUNK_SIZE as i32,
                })
                .await
                .map_err(MtProtoError::Invocation)?;

            match result {
                tl::enums::upload::File::File(file) => {
                    if file.bytes.is_empty() {
                        log::info!("Download complete, total {} bytes", data.len());
                        break;
                    }

                    let chunk_len = file.bytes.len();
                    data.extend_from_slice(&file.bytes);
                    offset += chunk_len as i64;

                    log::debug!("Downloaded chunk: {} bytes (total: {} bytes)", chunk_len, data.len());

                    // If we got less than requested, we're done
                    if (chunk_len as i64) < CHUNK_SIZE {
                        log::info!("Download complete, total {} bytes", data.len());
                        break;
                    }
                }
                tl::enums::upload::File::CdnRedirect(_) => {
                    return Err(MtProtoError::CdnRedirectNotSupported);
                }
            }
        }

        Ok(data)
    }

    /// Get information about a file_id without downloading
    pub fn decode_file_id(&self, file_id: &str) -> Result<DecodedFileId, MtProtoError> {
        DecodedFileId::decode(file_id)
    }

    /// Get a specific message by ID to extract fresh media info
    ///
    /// Bots cannot use messages.getHistory, but can use messages.getMessages
    /// with specific message IDs.
    ///
    /// # Arguments
    /// * `message_ids` - List of message IDs to fetch
    ///
    /// # Returns
    /// Vector of MediaInfo with fresh file data
    pub async fn get_messages_media(&self, message_ids: &[i32]) -> Result<Vec<MediaInfo>, MtProtoError> {
        log::info!("Getting {} messages by ID", message_ids.len());

        let input_messages: Vec<_> = message_ids
            .iter()
            .map(|&id| tl::enums::InputMessage::Id(tl::types::InputMessageId { id }))
            .collect();

        let messages = self
            .client
            .inner()
            .invoke(&tl::functions::messages::GetMessages { id: input_messages })
            .await
            .map_err(MtProtoError::Invocation)?;

        let message_list = match messages {
            tl::enums::messages::Messages::Messages(m) => m.messages,
            tl::enums::messages::Messages::Slice(m) => m.messages,
            tl::enums::messages::Messages::ChannelMessages(m) => m.messages,
            tl::enums::messages::Messages::NotModified(_) => vec![],
        };

        let mut media_list = Vec::new();

        for msg in message_list {
            if let tl::enums::Message::Message(message) = msg {
                if let Some(media) = message.media {
                    if let Some(info) = self.extract_media_info(&media, message.id, message.date) {
                        media_list.push(info);
                    }
                }
            }
        }

        log::info!("Found {} media items", media_list.len());
        Ok(media_list)
    }

    /// Get fresh file_reference for a specific message
    ///
    /// This fetches the message and extracts media with fresh file_reference.
    ///
    /// # Arguments
    /// * `message_id` - The message ID containing the media
    ///
    /// # Returns
    /// MediaInfo with fresh file_reference, or error if not found
    pub async fn get_fresh_media_info(&self, message_id: i32) -> Result<MediaInfo, MtProtoError> {
        let media_list = self.get_messages_media(&[message_id]).await?;
        media_list.into_iter().next().ok_or(MtProtoError::NoMediaInMessage)
    }

    /// Extract media information from a MessageMedia
    fn extract_media_info(&self, media: &tl::enums::MessageMedia, message_id: i32, date: i32) -> Option<MediaInfo> {
        match media {
            tl::enums::MessageMedia::Document(doc_media) => {
                if let Some(tl::enums::Document::Document(doc)) = &doc_media.document {
                    // Extract filename and mime type from attributes
                    let mut filename = None;
                    let mime_type = doc.mime_type.clone();
                    let mut duration = None;

                    for attr in &doc.attributes {
                        match attr {
                            tl::enums::DocumentAttribute::Filename(f) => {
                                filename = Some(f.file_name.clone());
                            }
                            tl::enums::DocumentAttribute::Audio(a) => {
                                duration = Some(a.duration);
                                if filename.is_none() {
                                    filename = a.title.clone();
                                }
                            }
                            tl::enums::DocumentAttribute::Video(v) => {
                                duration = Some(v.duration as i32);
                            }
                            _ => {}
                        }
                    }

                    return Some(MediaInfo {
                        message_id,
                        date,
                        media_type: MediaType::Document,
                        id: doc.id,
                        access_hash: doc.access_hash,
                        file_reference: doc.file_reference.clone(),
                        dc_id: doc.dc_id,
                        size: doc.size,
                        filename,
                        mime_type: Some(mime_type),
                        duration,
                    });
                }
            }
            tl::enums::MessageMedia::Photo(photo_media) => {
                if let Some(tl::enums::Photo::Photo(photo)) = &photo_media.photo {
                    // Get largest size
                    let size = photo
                        .sizes
                        .iter()
                        .filter_map(|s| match s {
                            tl::enums::PhotoSize::Size(ps) => Some(ps.size as i64),
                            _ => None,
                        })
                        .max()
                        .unwrap_or(0);

                    return Some(MediaInfo {
                        message_id,
                        date,
                        media_type: MediaType::Photo,
                        id: photo.id,
                        access_hash: photo.access_hash,
                        file_reference: photo.file_reference.clone(),
                        dc_id: photo.dc_id,
                        size,
                        filename: None,
                        mime_type: Some("image/jpeg".to_string()),
                        duration: None,
                    });
                }
            }
            _ => {}
        }
        None
    }

    /// Download media by MediaInfo (with fresh file_reference)
    pub async fn download_media(&self, media: &MediaInfo, output_path: &Path) -> Result<u64, MtProtoError> {
        log::info!("Downloading media id={} to {:?}", media.id, output_path);

        let location = match media.media_type {
            MediaType::Document => {
                tl::enums::InputFileLocation::InputDocumentFileLocation(tl::types::InputDocumentFileLocation {
                    id: media.id,
                    access_hash: media.access_hash,
                    file_reference: media.file_reference.clone(),
                    thumb_size: String::new(),
                })
            }
            MediaType::Photo => {
                tl::enums::InputFileLocation::InputPhotoFileLocation(tl::types::InputPhotoFileLocation {
                    id: media.id,
                    access_hash: media.access_hash,
                    file_reference: media.file_reference.clone(),
                    thumb_size: "y".to_string(), // largest size
                })
            }
        };

        let bytes = self.download_file_location(location, media.dc_id).await?;

        let mut file = tokio::fs::File::create(output_path).await?;
        file.write_all(&bytes).await?;
        file.flush().await?;

        log::info!("Downloaded {} bytes to {:?}", bytes.len(), output_path);
        Ok(bytes.len() as u64)
    }
}

/// Type of media
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaType {
    Document,
    Photo,
}

/// Information about media extracted from a message
#[derive(Debug, Clone)]
pub struct MediaInfo {
    /// Message ID containing this media
    pub message_id: i32,
    /// Unix timestamp of the message
    pub date: i32,
    /// Type of media
    pub media_type: MediaType,
    /// Document/Photo ID
    pub id: i64,
    /// Access hash
    pub access_hash: i64,
    /// Fresh file reference
    pub file_reference: Vec<u8>,
    /// Datacenter ID
    pub dc_id: i32,
    /// File size in bytes
    pub size: i64,
    /// Original filename (if available)
    pub filename: Option<String>,
    /// MIME type
    pub mime_type: Option<String>,
    /// Duration in seconds (for audio/video)
    pub duration: Option<i32>,
}
