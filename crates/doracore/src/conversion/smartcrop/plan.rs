//! Converts per-frame detections into a smoothed crop plan.

use super::detect::Detection;
use super::{CropRegion, SmartCropPlan, VIDEO_NOTE_SIZE};

const EMA_ALPHA: f64 = 0.3;

pub fn build_crop_plan(frames: &[(f64, Vec<Detection>)], video_w: u32, video_h: u32) -> Option<SmartCropPlan> {
    if frames.is_empty() {
        return None;
    }

    let (scaled_w, scaled_h) = if video_w >= video_h {
        let sw = (video_w as f64 * VIDEO_NOTE_SIZE as f64 / video_h as f64).round() as u32;
        (sw, VIDEO_NOTE_SIZE)
    } else {
        let sh = (video_h as f64 * VIDEO_NOTE_SIZE as f64 / video_w as f64).round() as u32;
        (VIDEO_NOTE_SIZE, sh)
    };

    let mut centroids: Vec<(f64, Option<(f64, f64)>)> = Vec::new();
    for (ts, detections) in frames {
        if detections.is_empty() {
            centroids.push((*ts, None));
            continue;
        }
        let mut sum_x = 0.0f64;
        let mut sum_y = 0.0f64;
        let mut sum_w = 0.0f64;
        for det in detections {
            let weight = det.confidence as f64 * det.bbox.area() as f64;
            let cx = det.bbox.center_x() as f64 * scaled_w as f64 / video_w as f64;
            let cy = det.bbox.center_y() as f64 * scaled_h as f64 / video_h as f64;
            sum_x += cx * weight;
            sum_y += cy * weight;
            sum_w += weight;
        }
        if sum_w > 0.0 {
            centroids.push((*ts, Some((sum_x / sum_w, sum_y / sum_w))));
        } else {
            centroids.push((*ts, None));
        }
    }

    if !centroids.iter().any(|(_, c)| c.is_some()) {
        return None;
    }

    let default_cx = scaled_w as f64 / 2.0;
    let default_cy = scaled_h as f64 / 2.0;
    let mut prev_x = default_cx;
    let mut prev_y = default_cy;

    for (_, centroid) in &centroids {
        if let Some((cx, cy)) = centroid {
            prev_x = *cx;
            prev_y = *cy;
            break;
        }
    }

    let mut smoothed: Vec<CropRegion> = Vec::new();
    for (ts, centroid) in &centroids {
        let (raw_x, raw_y) = match centroid {
            Some((cx, cy)) => (*cx, *cy),
            None => (prev_x, prev_y),
        };
        let smooth_x = EMA_ALPHA * raw_x + (1.0 - EMA_ALPHA) * prev_x;
        let smooth_y = EMA_ALPHA * raw_y + (1.0 - EMA_ALPHA) * prev_y;
        let half = VIDEO_NOTE_SIZE as f64 / 2.0;
        let crop_x = (smooth_x - half).max(0.0).min((scaled_w - VIDEO_NOTE_SIZE) as f64);
        let crop_y = (smooth_y - half).max(0.0).min((scaled_h - VIDEO_NOTE_SIZE) as f64);
        smoothed.push(CropRegion {
            timestamp_secs: *ts,
            x: crop_x.round() as u32,
            y: crop_y.round() as u32,
        });
        prev_x = smooth_x;
        prev_y = smooth_y;
    }

    Some(SmartCropPlan {
        scaled_w,
        scaled_h,
        regions: smoothed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversion::smartcrop::detect::{BBox, Detection, DetectionClass};

    fn face(x1: f32, y1: f32, x2: f32, y2: f32, conf: f32) -> Detection {
        Detection {
            class: DetectionClass::Face,
            confidence: conf,
            bbox: BBox { x1, y1, x2, y2 },
        }
    }

    #[test]
    fn test_no_detections_returns_none() {
        let frames = vec![(0.0, vec![]), (1.0, vec![])];
        assert!(build_crop_plan(&frames, 1920, 1080).is_none());
    }

    #[test]
    fn test_empty_frames_returns_none() {
        let frames: Vec<(f64, Vec<Detection>)> = vec![];
        assert!(build_crop_plan(&frames, 1920, 1080).is_none());
    }

    #[test]
    fn test_single_centered_face() {
        let frames = vec![(0.0, vec![face(910.0, 490.0, 1010.0, 590.0, 0.9)])];
        let plan = build_crop_plan(&frames, 1920, 1080).unwrap();
        assert_eq!(plan.scaled_h, 640);
        assert!(plan.scaled_w > 640);
        assert_eq!(plan.regions.len(), 1);
    }

    #[test]
    fn test_ema_smoothing() {
        let frames = vec![
            (0.0, vec![face(100.0, 540.0, 200.0, 640.0, 0.9)]),
            (1.0, vec![face(1700.0, 540.0, 1800.0, 640.0, 0.9)]),
            (2.0, vec![face(1700.0, 540.0, 1800.0, 640.0, 0.9)]),
        ];
        let plan = build_crop_plan(&frames, 1920, 1080).unwrap();
        assert_eq!(plan.regions.len(), 3);
        assert!(plan.regions[1].x > plan.regions[0].x);
        assert!(plan.regions[1].x < plan.regions[2].x);
    }
}
