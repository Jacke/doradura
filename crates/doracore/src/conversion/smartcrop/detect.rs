//! YOLOv8n-face ONNX inference for face detection.

use super::super::ConversionError;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const MODEL_BYTES: &[u8] = include_bytes!("../../../../../assets/models/yolov8n-face.onnx");
const MODEL_INPUT_SIZE: u32 = 640;
const CONFIDENCE_THRESHOLD: f32 = 0.35;
const NMS_IOU_THRESHOLD: f32 = 0.45;

#[derive(Debug, Clone)]
pub struct BBox {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
}

impl BBox {
    pub fn area(&self) -> f32 {
        (self.x2 - self.x1).max(0.0) * (self.y2 - self.y1).max(0.0)
    }
    pub fn center_x(&self) -> f32 {
        (self.x1 + self.x2) / 2.0
    }
    pub fn center_y(&self) -> f32 {
        (self.y1 + self.y2) / 2.0
    }
    fn iou(&self, other: &BBox) -> f32 {
        let ix1 = self.x1.max(other.x1);
        let iy1 = self.y1.max(other.y1);
        let ix2 = self.x2.min(other.x2);
        let iy2 = self.y2.min(other.y2);
        let inter = (ix2 - ix1).max(0.0) * (iy2 - iy1).max(0.0);
        let union = self.area() + other.area() - inter;
        if union > 0.0 {
            inter / union
        } else {
            0.0
        }
    }
}

#[derive(Debug, Clone)]
pub enum DetectionClass {
    Face,
}

#[derive(Debug, Clone)]
pub struct Detection {
    pub class: DetectionClass,
    pub confidence: f32,
    pub bbox: BBox,
}

fn get_session() -> Result<&'static Mutex<ort::session::Session>, String> {
    static SESSION: OnceLock<Result<Mutex<ort::session::Session>, String>> = OnceLock::new();
    let result = SESSION.get_or_init(|| {
        let _ = ort::init().commit();
        let session = ort::session::Session::builder()
            .map_err(|e| format!("Session builder: {}", e))?
            .commit_from_memory(MODEL_BYTES)
            .map_err(|e| format!("Load model: {}", e))?;
        Ok(Mutex::new(session))
    });
    match result {
        Ok(s) => Ok(s),
        Err(e) => Err(e.clone()),
    }
}

pub async fn get_video_dimensions(path: &Path) -> Result<(u32, u32), ConversionError> {
    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=s=x:p=0",
        ])
        .arg(path)
        .output()
        .await?;
    if !output.status.success() {
        return Err(ConversionError::FfmpegError(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = text.trim().split('x').collect();
    if parts.len() != 2 {
        return Err(ConversionError::FfmpegError(format!(
            "Unexpected ffprobe output: {}",
            text.trim()
        )));
    }
    let w: u32 = parts[0]
        .parse()
        .map_err(|_| ConversionError::FfmpegError("Parse width".into()))?;
    let h: u32 = parts[1]
        .parse()
        .map_err(|_| ConversionError::FfmpegError("Parse height".into()))?;
    Ok((w, h))
}

pub async fn extract_frames(
    input: &Path,
    duration: f64,
    start: Option<f64>,
    tmp_dir: &Path,
) -> Result<Vec<(f64, PathBuf)>, ConversionError> {
    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
    if let Some(ss) = start {
        cmd.arg("-ss").arg(format!("{}", ss));
    }
    cmd.arg("-i").arg(input);
    cmd.arg("-t").arg(format!("{}", duration));
    cmd.args(["-vf", "fps=1", "-q:v", "2"]);
    cmd.arg(tmp_dir.join("frame_%04d.jpg"));
    let output = cmd.output().await?;
    if !output.status.success() {
        return Err(ConversionError::FfmpegError(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    let mut frames = Vec::new();
    let mut i = 1u32;
    loop {
        let frame_path = tmp_dir.join(format!("frame_{:04}.jpg", i));
        if !frame_path.exists() {
            break;
        }
        frames.push(((i - 1) as f64, frame_path));
        i += 1;
    }
    Ok(frames)
}

pub fn detect_faces(frame_path: &Path, video_w: u32, video_h: u32) -> Result<Vec<Detection>, String> {
    let session_mutex = get_session()?;
    let mut session = session_mutex.lock().map_err(|e| format!("Lock: {}", e))?;

    let img = image::open(frame_path)
        .map_err(|e| format!("Image load: {}", e))?
        .resize_exact(
            MODEL_INPUT_SIZE,
            MODEL_INPUT_SIZE,
            image::imageops::FilterType::Triangle,
        )
        .to_rgb8();

    let hw = (MODEL_INPUT_SIZE * MODEL_INPUT_SIZE) as usize;
    let mut data = vec![0.0f32; 3 * hw];
    let pixels = img.as_raw();
    for i in 0..hw {
        data[i] = pixels[i * 3] as f32 / 255.0;
        data[hw + i] = pixels[i * 3 + 1] as f32 / 255.0;
        data[2 * hw + i] = pixels[i * 3 + 2] as f32 / 255.0;
    }

    let shape = vec![1usize, 3, MODEL_INPUT_SIZE as usize, MODEL_INPUT_SIZE as usize];
    let tensor = ort::value::Tensor::<f32>::from_array((shape, data.into_boxed_slice()))
        .map_err(|e| format!("Tensor: {}", e))?;

    let input_values = ort::inputs!["images" => tensor];
    let outputs = session.run(input_values).map_err(|e| format!("Inference: {}", e))?;

    let output_value = outputs.get("output0").ok_or_else(|| "No output0".to_string())?;
    let (out_shape, flat) = output_value
        .try_extract_tensor::<f32>()
        .map_err(|e| format!("Extract: {}", e))?;

    // Shape derefs to SmallVec<[i64; 4]>
    if out_shape.len() != 3 {
        return Err(format!("Unexpected shape len: {}", out_shape.len()));
    }
    let num_attrs = out_shape[1] as usize;
    let num_boxes = out_shape[2] as usize;

    let scale_x = video_w as f32 / MODEL_INPUT_SIZE as f32;
    let scale_y = video_h as f32 / MODEL_INPUT_SIZE as f32;

    let mut detections = Vec::new();
    for box_i in 0..num_boxes {
        let conf = flat[4 * num_boxes + box_i];
        if conf < CONFIDENCE_THRESHOLD {
            continue;
        }

        // YOLOv8 output layout: [cx, cy, w, h, conf, ...] × num_boxes
        let cx = flat[box_i];
        let cy = flat[num_boxes + box_i];
        let bw = flat[2 * num_boxes + box_i];
        let bh = flat[3 * num_boxes + box_i];

        let x1 = (cx - bw / 2.0) * scale_x;
        let y1 = (cy - bh / 2.0) * scale_y;
        let x2 = (cx + bw / 2.0) * scale_x;
        let y2 = (cy + bh / 2.0) * scale_y;

        let mut best_conf = conf;
        if num_attrs > 5 {
            for attr_i in 5..num_attrs {
                let cc = flat[attr_i * num_boxes + box_i];
                if cc > best_conf {
                    best_conf = cc;
                }
            }
        }

        detections.push(Detection {
            class: DetectionClass::Face,
            confidence: best_conf.min(1.0),
            bbox: BBox {
                x1: x1.max(0.0).min(video_w as f32),
                y1: y1.max(0.0).min(video_h as f32),
                x2: x2.max(0.0).min(video_w as f32),
                y2: y2.max(0.0).min(video_h as f32),
            },
        });
    }

    nms(&mut detections);
    Ok(detections)
}

fn nms(detections: &mut Vec<Detection>) {
    detections.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut keep: Vec<usize> = Vec::new();
    for i in 0..detections.len() {
        let mut should_keep = true;
        for &k in &keep {
            if detections[i].bbox.iou(&detections[k].bbox) > NMS_IOU_THRESHOLD {
                should_keep = false;
                break;
            }
        }
        if should_keep {
            keep.push(i);
        }
    }
    let kept: Vec<Detection> = keep.into_iter().map(|i| detections[i].clone()).collect();
    *detections = kept;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bbox_area() {
        let bbox = BBox {
            x1: 0.0,
            y1: 0.0,
            x2: 100.0,
            y2: 100.0,
        };
        assert_eq!(bbox.area(), 10000.0);
    }

    #[test]
    fn test_bbox_center() {
        let bbox = BBox {
            x1: 100.0,
            y1: 200.0,
            x2: 300.0,
            y2: 400.0,
        };
        assert_eq!(bbox.center_x(), 200.0);
        assert_eq!(bbox.center_y(), 300.0);
    }

    #[test]
    fn test_bbox_iou_no_overlap() {
        let a = BBox {
            x1: 0.0,
            y1: 0.0,
            x2: 10.0,
            y2: 10.0,
        };
        let b = BBox {
            x1: 20.0,
            y1: 20.0,
            x2: 30.0,
            y2: 30.0,
        };
        assert_eq!(a.iou(&b), 0.0);
    }

    #[test]
    fn test_bbox_iou_identical() {
        let a = BBox {
            x1: 0.0,
            y1: 0.0,
            x2: 10.0,
            y2: 10.0,
        };
        assert!((a.iou(&a) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_nms_removes_overlapping() {
        let mut dets = vec![
            Detection {
                class: DetectionClass::Face,
                confidence: 0.9,
                bbox: BBox {
                    x1: 0.0,
                    y1: 0.0,
                    x2: 100.0,
                    y2: 100.0,
                },
            },
            Detection {
                class: DetectionClass::Face,
                confidence: 0.7,
                bbox: BBox {
                    x1: 5.0,
                    y1: 5.0,
                    x2: 105.0,
                    y2: 105.0,
                },
            },
        ];
        nms(&mut dets);
        assert_eq!(dets.len(), 1);
    }

    #[test]
    fn test_nms_keeps_non_overlapping() {
        let mut dets = vec![
            Detection {
                class: DetectionClass::Face,
                confidence: 0.9,
                bbox: BBox {
                    x1: 0.0,
                    y1: 0.0,
                    x2: 100.0,
                    y2: 100.0,
                },
            },
            Detection {
                class: DetectionClass::Face,
                confidence: 0.8,
                bbox: BBox {
                    x1: 200.0,
                    y1: 200.0,
                    x2: 300.0,
                    y2: 300.0,
                },
            },
        ];
        nms(&mut dets);
        assert_eq!(dets.len(), 2);
    }
}
