//! Converts a SmartCropPlan into an ffmpeg filter expression.

use super::{SmartCropPlan, VIDEO_NOTE_SIZE};

pub fn plan_to_filter(plan: &SmartCropPlan) -> String {
    let regions = &plan.regions;

    if regions.is_empty() {
        return format!(
            "scale={}:{}:force_original_aspect_ratio=increase,crop={}:{},format=yuv420p",
            VIDEO_NOTE_SIZE, VIDEO_NOTE_SIZE, VIDEO_NOTE_SIZE, VIDEO_NOTE_SIZE
        );
    }

    if regions.len() == 1 {
        return format!(
            "scale={}:{}:force_original_aspect_ratio=increase,crop={}:{}:{}:{},format=yuv420p",
            plan.scaled_w, plan.scaled_h, VIDEO_NOTE_SIZE, VIDEO_NOTE_SIZE, regions[0].x, regions[0].y
        );
    }

    let x_expr = build_lerp_expr(regions, |r| r.x);
    let y_expr = build_lerp_expr(regions, |r| r.y);

    format!(
        "scale={}:{}:force_original_aspect_ratio=increase,crop={}:{}:{}:{},format=yuv420p",
        plan.scaled_w, plan.scaled_h, VIDEO_NOTE_SIZE, VIDEO_NOTE_SIZE, x_expr, y_expr
    )
}

fn build_lerp_expr(regions: &[super::CropRegion], value_fn: impl Fn(&super::CropRegion) -> u32) -> String {
    let mut parts = Vec::new();
    for i in 0..regions.len() - 1 {
        let t0 = regions[i].timestamp_secs;
        let t1 = regions[i + 1].timestamp_secs;
        let v0 = value_fn(&regions[i]);
        let v1 = value_fn(&regions[i + 1]);
        if v0 == v1 {
            parts.push(format!("if(between(t\\,{:.3}\\,{:.3})\\,{}", t0, t1, v0));
        } else {
            let slope = (v1 as f64 - v0 as f64) / (t1 - t0);
            parts.push(format!(
                "if(between(t\\,{:.3}\\,{:.3})\\,{}+{:.3}*(t-{:.3})",
                t0, t1, v0, slope, t0
            ));
        }
    }
    let last_val = value_fn(regions.last().unwrap());
    if parts.is_empty() {
        return format!("{}", last_val);
    }
    let mut expr = format!("{}", last_val);
    for part in parts.iter().rev() {
        expr = format!("{}\\,{})", part, expr);
    }
    format!("'{}'", expr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversion::smartcrop::CropRegion;

    #[test]
    fn test_empty_regions_fallback() {
        let plan = SmartCropPlan {
            scaled_w: 640,
            scaled_h: 640,
            regions: vec![],
        };
        let filter = plan_to_filter(&plan);
        assert!(filter.contains("crop=640:640"));
    }

    #[test]
    fn test_single_region_static_crop() {
        let plan = SmartCropPlan {
            scaled_w: 1138,
            scaled_h: 640,
            regions: vec![CropRegion {
                timestamp_secs: 0.0,
                x: 249,
                y: 0,
            }],
        };
        let filter = plan_to_filter(&plan);
        assert!(filter.contains("crop=640:640:249:0"));
    }

    #[test]
    fn test_two_regions_interpolation() {
        let plan = SmartCropPlan {
            scaled_w: 1138,
            scaled_h: 640,
            regions: vec![
                CropRegion {
                    timestamp_secs: 0.0,
                    x: 0,
                    y: 0,
                },
                CropRegion {
                    timestamp_secs: 1.0,
                    x: 100,
                    y: 0,
                },
            ],
        };
        let filter = plan_to_filter(&plan);
        assert!(filter.contains("between(t"));
    }
}
