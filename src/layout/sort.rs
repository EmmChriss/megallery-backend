use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::Image;

use super::dist::{DistanceFunction, DistanceFunctionVariants};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub enum CompareFunctionVariants {
	SignedDist {
		dist: DistanceFunctionVariants,
	},
	ComparativeDist {
		compared_to: Uuid,
		dist: DistanceFunctionVariants,
	},
}

pub trait CompareFunction {
	fn compare(&self, m1: &Image, m2: &Image) -> Ordering;
}

pub struct SignedDist<D: DistanceFunction> {
	pub dist: D,
}

impl<D: DistanceFunction> CompareFunction for SignedDist<D> {
	#[inline(always)]
	fn compare(&self, m1: &Image, m2: &Image) -> Ordering {
		self.dist.dist(m1, m2).partial_cmp(&0.0).unwrap()
	}
}

pub struct CompareDist<D: DistanceFunction> {
	pub dist: D,
	pub compared_to: Image,
}

impl<D: DistanceFunction> CompareFunction for CompareDist<D> {
	fn compare(&self, m1: &Image, m2: &Image) -> Ordering {
		self.dist
			.dist(&self.compared_to, m1)
			.partial_cmp(&self.dist.dist(&self.compared_to, m2))
			.unwrap()
	}
}
