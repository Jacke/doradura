use std::time::Duration;

use bytes::Bytes;
use prost::Message;
use tokio::time::timeout;
use tonic::body::BoxBody;
use tonic::client::Grpc;
use tonic::codec::ProstCodec;
use tonic::codegen::{Body, StdError};
use tonic::transport::{Channel, Endpoint};
use tonic::{Code, Request, Response, Status};

use crate::core::config;

pub mod proto {
    use super::*;

    #[derive(Clone, PartialEq, Message)]
    pub struct User {
        #[prost(string, tag = "1")]
        pub telegram_id: String,
        #[prost(string, tag = "2")]
        pub phone: String,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct Media {
        #[prost(string, tag = "1")]
        pub url: String,
        #[prost(string, tag = "2")]
        pub title: String,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct SummaryRequest {
        #[prost(message, optional, tag = "1")]
        pub user: Option<User>,
        #[prost(message, optional, tag = "2")]
        pub media: Option<Media>,
        #[prost(string, tag = "3")]
        pub language: String,
        #[prost(int32, tag = "4")]
        pub max_length_tokens: i32,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct SummarySection {
        #[prost(string, tag = "1")]
        pub title: String,
        #[prost(string, tag = "2")]
        pub text: String,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct SummaryResponse {
        #[prost(string, tag = "1")]
        pub summary: String,
        #[prost(string, repeated, tag = "2")]
        pub highlights: Vec<String>,
        #[prost(message, repeated, tag = "3")]
        pub sections: Vec<SummarySection>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct SubtitlesRequest {
        #[prost(message, optional, tag = "1")]
        pub user: Option<User>,
        #[prost(message, optional, tag = "2")]
        pub media: Option<Media>,
        #[prost(string, tag = "3")]
        pub language: String,
        #[prost(string, tag = "4")]
        pub format: String,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct SubtitleSegment {
        #[prost(int64, tag = "1")]
        pub start_ms: i64,
        #[prost(int64, tag = "2")]
        pub end_ms: i64,
        #[prost(string, tag = "3")]
        pub text: String,
        #[prost(string, tag = "4")]
        pub speaker: String,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct SubtitlesResponse {
        #[prost(string, tag = "1")]
        pub raw_subtitles: String,
        #[prost(string, tag = "2")]
        pub format: String,
        #[prost(message, repeated, tag = "3")]
        pub segments: Vec<SubtitleSegment>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct HealthRequest {}

    #[derive(Clone, PartialEq, Message)]
    pub struct HealthResponse {
        #[prost(string, tag = "1")]
        pub status: String,
        #[prost(string, tag = "2")]
        pub version: String,
        #[prost(string, tag = "3")]
        pub message: String,
        #[prost(string, tag = "4")]
        pub uptime: String,
    }

    pub mod downsub_service_client {
        use super::*;
        use tonic::codegen::http::uri::PathAndQuery;

        #[derive(Debug, Clone)]
        pub struct DownsubServiceClient<T> {
            inner: Grpc<T>,
        }

        impl DownsubServiceClient<Channel> {
            pub fn new(channel: Channel) -> Self {
                Self {
                    inner: Grpc::new(channel),
                }
            }
        }

        impl<T> DownsubServiceClient<T>
        where
            T: tonic::client::GrpcService<BoxBody>,
            T::ResponseBody: Body<Data = Bytes> + Send + 'static,
            <T::ResponseBody as Body>::Error: Into<StdError> + Send + Sync,
            T::Error: Into<StdError> + Send + Sync,
        {
            pub async fn get_summary(
                &mut self,
                request: impl tonic::IntoRequest<SummaryRequest>,
            ) -> Result<Response<SummaryResponse>, Status> {
                self.inner
                    .ready()
                    .await
                    .map_err(|e| Status::new(Code::Unknown, format!("Service was not ready: {}", e.into())))?;
                let path = PathAndQuery::from_static("/downsub.v1.DownsubService/GetSummary");
                self.inner
                    .unary(request.into_request(), path, ProstCodec::default())
                    .await
            }

            pub async fn get_subtitles(
                &mut self,
                request: impl tonic::IntoRequest<SubtitlesRequest>,
            ) -> Result<Response<SubtitlesResponse>, Status> {
                self.inner
                    .ready()
                    .await
                    .map_err(|e| Status::new(Code::Unknown, format!("Service was not ready: {}", e.into())))?;
                let path = PathAndQuery::from_static("/downsub.v1.DownsubService/GetSubtitles");
                self.inner
                    .unary(request.into_request(), path, ProstCodec::default())
                    .await
            }

            pub async fn check_health(
                &mut self,
                request: impl tonic::IntoRequest<HealthRequest>,
            ) -> Result<Response<HealthResponse>, Status> {
                self.inner
                    .ready()
                    .await
                    .map_err(|e| Status::new(Code::Unknown, format!("Service was not ready: {}", e.into())))?;
                let path = PathAndQuery::from_static("/downsub.v1.DownsubService/CheckHealth");
                self.inner
                    .unary(request.into_request(), path, ProstCodec::default())
                    .await
            }
        }
    }
}

const DEFAULT_SUMMARY_TOKENS: i32 = 400;

#[derive(Debug, Clone)]
pub struct DownsubGateway {
    client: Option<proto::downsub_service_client::DownsubServiceClient<Channel>>,
    timeout: Duration,
    api_key: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum DownsubError {
    #[error("Downsub is disabled")]
    Unavailable,
    #[error("Downsub request timed out")]
    Timeout,
    #[error("Downsub gRPC error: {0}")]
    Status(#[from] Status),
    #[error("Downsub transport error: {0}")]
    Transport(#[from] tonic::transport::Error),
}

#[derive(Debug)]
pub struct SummaryResult {
    pub summary: String,
    pub highlights: Vec<String>,
    pub sections: Vec<SummarySection>,
}

#[derive(Debug)]
pub struct SummarySection {
    pub title: Option<String>,
    pub text: String,
}

#[derive(Debug)]
pub struct SubtitlesResult {
    pub raw_subtitles: String,
    pub format: String,
    pub segments: Vec<SubtitleSegment>,
}

#[derive(Debug)]
pub struct HealthResult {
    pub status: String,
    pub version: String,
    pub message: Option<String>,
    pub uptime: Option<String>,
}

#[derive(Debug)]
pub struct SubtitleSegment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub speaker: Option<String>,
}

impl DownsubGateway {
    pub fn from_env() -> Self {
        let timeout = config::downsub::timeout();
        let mut client = None;

        if let Some(endpoint_url) = config::DOWNSUB_GRPC_ENDPOINT
            .as_ref()
            .filter(|url| !url.trim().is_empty())
        {
            match Endpoint::from_shared(endpoint_url.clone()) {
                Ok(endpoint) => {
                    let channel = endpoint.connect_timeout(timeout).connect_lazy();
                    client = Some(proto::downsub_service_client::DownsubServiceClient::new(channel));
                    log::info!("Downsub gRPC gateway configured (lazy connect): {}", endpoint_url);
                }
                Err(err) => {
                    log::warn!("Invalid Downsub gRPC endpoint {:?}: {}", endpoint_url, err);
                }
            }
        }

        Self {
            client,
            timeout,
            api_key: std::env::var("DOWNSUB_API_KEY").ok(),
        }
    }

    pub fn is_available(&self) -> bool {
        self.client.is_some()
    }

    pub async fn summarize_url(
        &self,
        telegram_id: i64,
        phone: Option<String>,
        url: impl Into<String>,
        language: Option<String>,
    ) -> Result<SummaryResult, DownsubError> {
        let mut client = self.client.clone().ok_or(DownsubError::Unavailable)?;

        let media = proto::Media {
            url: url.into(),
            title: String::new(),
        };

        let request = proto::SummaryRequest {
            user: Some(proto::User {
                telegram_id: telegram_id.to_string(),
                phone: phone.unwrap_or_default(),
            }),
            media: Some(media),
            language: language.unwrap_or_default(),
            max_length_tokens: DEFAULT_SUMMARY_TOKENS,
        };

        let mut grpc_request = Request::new(request);
        if let Some(ref key) = self.api_key {
            grpc_request
                .metadata_mut()
                .insert("authorization", format!("Bearer {}", key).parse().unwrap());
        }

        let response = timeout(self.timeout, client.get_summary(grpc_request))
            .await
            .map_err(|_| DownsubError::Timeout)?
            .map_err(DownsubError::Status)?
            .into_inner();

        Ok(SummaryResult {
            summary: response.summary,
            highlights: response.highlights,
            sections: response
                .sections
                .into_iter()
                .map(|section| SummarySection {
                    title: if section.title.is_empty() {
                        None
                    } else {
                        Some(section.title)
                    },
                    text: section.text,
                })
                .collect(),
        })
    }

    pub async fn fetch_subtitles(
        &self,
        telegram_id: i64,
        phone: Option<String>,
        url: impl Into<String>,
        format_hint: Option<String>,
        language: Option<String>,
    ) -> Result<SubtitlesResult, DownsubError> {
        let mut client = self.client.clone().ok_or(DownsubError::Unavailable)?;

        let format = format_hint.unwrap_or_else(|| "srt".to_string());
        let media = proto::Media {
            url: url.into(),
            title: String::new(),
        };

        let request = proto::SubtitlesRequest {
            user: Some(proto::User {
                telegram_id: telegram_id.to_string(),
                phone: phone.unwrap_or_default(),
            }),
            media: Some(media),
            language: language.unwrap_or_default(),
            format: format.clone(),
        };

        let mut grpc_request = Request::new(request);
        if let Some(ref key) = self.api_key {
            grpc_request
                .metadata_mut()
                .insert("authorization", format!("Bearer {}", key).parse().unwrap());
        }

        let response = timeout(self.timeout, client.get_subtitles(grpc_request))
            .await
            .map_err(|_| DownsubError::Timeout)?
            .map_err(DownsubError::Status)?
            .into_inner();

        let segments = response
            .segments
            .into_iter()
            .map(|segment| SubtitleSegment {
                start_ms: segment.start_ms,
                end_ms: segment.end_ms,
                text: segment.text,
                speaker: if segment.speaker.is_empty() {
                    None
                } else {
                    Some(segment.speaker)
                },
            })
            .collect();

        let format_value = if response.format.is_empty() {
            format.clone()
        } else {
            response.format
        };

        Ok(SubtitlesResult {
            raw_subtitles: response.raw_subtitles,
            format: format_value,
            segments,
        })
    }

    pub async fn check_health(&self) -> Result<HealthResult, DownsubError> {
        let mut client = self.client.clone().ok_or(DownsubError::Unavailable)?;
        let response = timeout(self.timeout, client.check_health(Request::new(proto::HealthRequest {})))
            .await
            .map_err(|_| DownsubError::Timeout)?
            .map_err(DownsubError::Status)?
            .into_inner();

        Ok(HealthResult {
            status: response.status,
            version: response.version,
            message: if response.message.is_empty() {
                None
            } else {
                Some(response.message)
            },
            uptime: if response.uptime.is_empty() {
                None
            } else {
                Some(response.uptime)
            },
        })
    }
}
