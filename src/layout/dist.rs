use serde::{self, Deserialize, Serialize};

use crate::db::Image;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub enum DistanceFunctionVariants {
	Palette,
	PaletteCos,
	DateTime,
}

pub trait DistanceFunction: Send + Sync {
	fn dist(&self, m1: &Image, m2: &Image) -> f32;
}

pub struct PaletteDist;

impl DistanceFunction for PaletteDist {
	#[inline(always)]
	fn dist(&self, m1: &Image, m2: &Image) -> f32 {
		match (&m1.metadata.palette, &m2.metadata.palette) {
			(None, _) | (_, None) => f32::INFINITY,
			(Some(p1), Some(p2)) => {
				let mut sum = 0.0;
				let mut multiplier = 1.0;
				let start = p1.len().min(p2.len()) - 1;

				for i in start..=0 {
					let r = p1[i].0 as f32 - p2[i].0 as f32;
					let g = p1[i].1 as f32 - p2[i].1 as f32;
					let b = p1[i].2 as f32 - p2[i].2 as f32;

					sum += (r + g + b) * multiplier;
					multiplier *= 100.0;
				}

				sum
			}
		}
	}
}

pub struct PaletteCosDist;

impl DistanceFunction for PaletteCosDist {
	fn dist(&self, m1: &Image, m2: &Image) -> f32 {
		match (&m1.metadata.palette, &m2.metadata.palette) {
			(None, _) | (_, None) => f32::INFINITY,
			(Some(p1), Some(p2)) => {
				let mut count = 0.0;
				let mut sq_a = 0.0;
				let mut sq_b = 0.0;

				for (c1, c2) in p1.iter().zip(p2.iter()) {
					let c1 = (c1.0 as f32, c1.1 as f32, c1.2 as f32);
					let c2 = (c2.0 as f32, c2.1 as f32, c2.2 as f32);

					count += c1.0 + c1.1 + c1.2 + c2.0 + c2.1 + c2.2;
					sq_a += c1.0.powi(2) + c1.1.powi(2) + c1.2.powi(2);
					sq_b += c2.0.powi(2) + c2.1.powi(2) + c2.2.powi(2);
				}

				(count / sq_a.sqrt() * sq_b.sqrt()).recip()
			}
		}
	}
}

pub struct DateTimeDist;

impl DistanceFunction for DateTimeDist {
	fn dist(&self, m1: &Image, m2: &Image) -> f32 {
		match (&m1.metadata.date_time, &m2.metadata.date_time) {
			(None, _) | (_, None) => f32::INFINITY,
			(Some(dt1), Some(dt2)) => (dt1.timestamp() - dt2.timestamp()) as f32,
		}
	}
}
