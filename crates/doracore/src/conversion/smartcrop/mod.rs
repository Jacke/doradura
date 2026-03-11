//! Smart crop for circle video notes using YOLOv8n-face ONNX detection.

pub mod detect;
pub mod ffmpeg;
pub mod plan;

use super::video::VIDEO_NOTE_SIZE;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct CropRegion {
    pub timestamp_secs: f64,
    pub x: u32,
    pub y: u32,
}

#[derive(Debug, Clone)]
pub struct SmartCropPlan {
    pub scaled_w: u32,
    pub scaled_h: u32,
    pub regions: Vec<CropRegion>,
}

pub async fn compute_smart_crop(input: &Path, duration: f64, start: Option<f64>) -> Option<SmartCropPlan> {
    let (video_w, video_h) = match detect::get_video_dimensions(input).await {
        Ok(dims) => dims,
        Err(e) => {
            log::warn!("Smart crop: dimensions: {}", e);
            return None;
        }
    };

    if video_w == video_h || (video_w <= VIDEO_NOTE_SIZE && video_h <= VIDEO_NOTE_SIZE) {
        return None;
    }

    let tmp_dir = std::env::temp_dir().join(format!("smartcrop_{:x}", rand::random::<u64>()));
    if let Err(e) = tokio::fs::create_dir_all(&tmp_dir).await {
        log::warn!("Smart crop: temp dir: {}", e);
        return None;
    }

    let frame_paths = match detect::extract_frames(input, duration, start, &tmp_dir).await {
        Ok(f) if f.is_empty() => {
            cleanup(&tmp_dir).await;
            return None;
        }
        Ok(f) => f,
        Err(e) => {
            log::warn!("Smart crop: frames: {}", e);
            cleanup(&tmp_dir).await;
            return None;
        }
    };

    log::info!("Smart crop: {} frames, {}x{}", frame_paths.len(), video_w, video_h);

    let mut frame_detections: Vec<(f64, Vec<detect::Detection>)> = Vec::new();
    for (ts, frame_path) in &frame_paths {
        let fp = frame_path.clone();
        let (w, h) = (video_w, video_h);
        let detections =
            match tokio::task::spawn_blocking(move || std::panic::catch_unwind(|| detect::detect_faces(&fp, w, h)))
                .await
            {
                Ok(Ok(Ok(dets))) => dets,
                Ok(Ok(Err(e))) => {
                    log::warn!("Smart crop: detect t={}: {}", ts, e);
                    vec![]
                }
                Ok(Err(_)) => {
                    log::warn!("Smart crop: panic t={}", ts);
                    vec![]
                }
                Err(e) => {
                    log::warn!("Smart crop: spawn: {}", e);
                    vec![]
                }
            };
        frame_detections.push((*ts, detections));
    }

    cleanup(&tmp_dir).await;

    let plan = plan::build_crop_plan(&frame_detections, video_w, video_h);
    if let Some(ref p) = plan {
        log::info!("Smart crop: {} regions", p.regions.len());
    } else {
        log::info!("Smart crop: no faces, center crop fallback");
    }
    plan
}

async fn cleanup(dir: &Path) {
    let _ = tokio::fs::remove_dir_all(dir).await;
}
