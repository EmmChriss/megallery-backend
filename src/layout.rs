use axum::extract::Path;
use axum::http::StatusCode;
use axum::Json;
use axum::{response::IntoResponse, Extension};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::{Collection, DbExtension, Image};
use crate::err::{Error, Result};
use crate::layout::dist::{DateTimeDist, PaletteCosDist, PaletteDist};
use crate::layout::sort::{CompareDist, SignedDist};
use crate::uuid_to_string_serialize;

use self::dist::{DistanceFunction, DistanceFunctionVariants};
use self::sort::{CompareFunction, CompareFunctionVariants};

mod dist;
mod sort;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct UuidString(#[serde(serialize_with = "uuid_to_string_serialize")] Uuid);

impl From<Uuid> for UuidString {
	fn from(value: Uuid) -> Self {
		Self(value)
	}
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
	TopLeft,
	TopRight,
	BottomLeft,
	BottomRight,
	Center,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GridDistanceFunction {
	Manhattan,
	PseudoPythegorean,
	Pythegorean,
}

impl GridDistanceFunction {
	fn get_function(&self) -> impl Fn(i32, i32) -> f32 {
		match self {
			Self::Manhattan => |i: i32, j: i32| (i.abs() + j.abs()) as f32,
			Self::PseudoPythegorean => |i: i32, j: i32| {
				let i = i32::abs(i);
				let j = i32::abs(j);
				(i.min(j) as f32) * 1.4 + (i.max(j) as f32 - i.min(j) as f32)
			},
			Self::Pythegorean => |i: i32, j: i32| {
				let i = i as f32;
				let j = j as f32;
				f32::sqrt(i * i + j * j)
			},
		}
	}
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ExpansionGridOptions {
	anchor: Option<Anchor>,
	grid_dist: Option<GridDistanceFunction>,
	compare: CompareFunctionVariants,
}

fn create_expansion_grid<C: CompareFunction>(
	compare: C,
	metadata: &mut [Image],
	opts: ExpansionGridOptions,
) -> Vec<Vec<Option<UuidString>>> {
	let mut a: usize = (metadata.len() as f32).sqrt().ceil() as usize;
	a += (a + 1) % 2;
	let mut m: Vec<Vec<Option<UuidString>>> = vec![vec![None; a]; a];

	let a = a as i32;
	let (range_a, range_b) = match opts.anchor.unwrap_or(Anchor::Center) {
		Anchor::BottomLeft => (0..a, 0..a),
		Anchor::BottomRight => (0..a, -a..0),
		Anchor::TopLeft => (-a..0, 0..a),
		Anchor::TopRight => (-a..0, 0..a),
		Anchor::Center => {
			let a_2 = a / 2;
			(-a_2..a_2, -a_2..a_2)
		}
	};

	let mut positions = range_a
		.clone()
		.cartesian_product(range_b.clone())
		.collect_vec();

	let cost = opts
		.grid_dist
		.unwrap_or(GridDistanceFunction::Pythegorean)
		.get_function();

	positions.sort_unstable_by(|(i1, j1), (i2, j2)| {
		cost(*i1, *j1).partial_cmp(&cost(*i2, *j2)).unwrap()
	});

	metadata.sort_unstable_by(|i1, i2| compare.compare(i1, i2));

	for (img, (i, j)) in metadata.iter().zip(positions) {
		let i = (i - range_a.start) as usize;
		let j = (j - range_b.start) as usize;
		m[i][j] = Some(img.id.into());
	}

	m
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SortOptions {
	pub compare: CompareFunctionVariants,
}

fn sort_by<C: CompareFunction>(compare: C, metadata: &mut [Image], _opts: SortOptions) {
	metadata.sort_unstable_by(move |m1, m2| compare.compare(m1, m2))
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TsneOptions {
	pub dist: DistanceFunctionVariants,
}

fn tsne<D: DistanceFunction + Send + Sync>(
	dist: D,
	metadata: &[Image],
	_opts: TsneOptions,
) -> Vec<(UuidString, f32, f32)> {
	let mut tsne = bhtsne::tSNE::new(metadata);
	tsne.barnes_hut(0.5, move |a, b| f32::abs(dist.dist(a, b)));
	// tsne.barnes_hut(0.1, move |a, b| dist.dist(a, b));

	let mut res = tsne
		.embedding()
		.chunks(2)
		.zip(metadata)
		.map(|(pos, image)| (UuidString(image.id), pos[0], pos[1]))
		.collect_vec();

	let (min_x, min_y, max_x, max_y) = res.iter().fold(
		(f32::MAX, f32::MAX, f32::MIN, f32::MIN),
		|(min_x, min_y, max_x, max_y), (_, x, y)| {
			(min_x.min(*x), min_y.min(*y), max_x.max(*x), max_y.max(*y))
		},
	);

	for (_, x, y) in res.iter_mut() {
		*x = (*x - min_x) / (max_x - min_x);
		*y = (*y - min_y) / (max_y - min_y);
	}

	res
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub enum LayoutOptions {
	Sort(SortOptions),
	GridExpansion(ExpansionGridOptions),
	Tsne(TsneOptions),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub enum Layout {
	Sort { data: Vec<UuidString> },
	Grid { data: Vec<Vec<Option<UuidString>>> },
	Pos { data: Vec<(UuidString, f32, f32)> },
}

pub async fn get_layout(
	Extension(db): DbExtension,
	Path(collection_id): Path<Uuid>,
	Json(layout): Json<LayoutOptions>,
) -> Result<impl IntoResponse> {
	measure_time::info_time!("calculating layout");

	let collection = Collection::get_by_id(&db, collection_id)
		.await?
		.ok_or(Error::NotFound("collection".into()))?;

	if !collection.finalized {
		return Err(Error::Custom(
			StatusCode::BAD_REQUEST,
			"collection not finalized".into(),
		));
	}

	let mut images = Image::get_all_for_collection(&db, collection_id).await?;

	let buf: Result<Vec<u8>> = tokio::task::spawn_blocking(move || match layout {
		LayoutOptions::GridExpansion(opts) => {
			let data = match opts.compare {
				CompareFunctionVariants::SignedDist { dist } => match dist {
					DistanceFunctionVariants::Palette => {
						create_expansion_grid(SignedDist { dist: PaletteDist }, &mut images, opts)
					}
					DistanceFunctionVariants::PaletteCos => create_expansion_grid(
						SignedDist {
							dist: PaletteCosDist,
						},
						&mut images,
						opts,
					),
					DistanceFunctionVariants::DateTime => {
						create_expansion_grid(SignedDist { dist: DateTimeDist }, &mut images, opts)
					}
				},
				CompareFunctionVariants::ComparativeDist { compared_to, dist } => {
					let compared_to = images
						.iter()
						.find(|i| i.id == compared_to)
						.cloned()
						.ok_or(Error::NotFound(format!("image with id {}", compared_to)))?;

					match dist {
						DistanceFunctionVariants::Palette => create_expansion_grid(
							CompareDist {
								compared_to,
								dist: PaletteDist,
							},
							&mut images,
							opts,
						),
						DistanceFunctionVariants::PaletteCos => create_expansion_grid(
							CompareDist {
								compared_to,
								dist: PaletteCosDist,
							},
							&mut images,
							opts,
						),
						DistanceFunctionVariants::DateTime => create_expansion_grid(
							CompareDist {
								compared_to,
								dist: DateTimeDist,
							},
							&mut images,
							opts,
						),
					}
				}
			};

			rmp_serde::to_vec_named(&Layout::Grid { data }).map_err(Into::into)
		}
		LayoutOptions::Sort(opts) => {
			match opts.compare {
				CompareFunctionVariants::SignedDist { dist } => match dist {
					DistanceFunctionVariants::Palette => {
						sort_by(SignedDist { dist: PaletteDist }, &mut images, opts)
					}
					DistanceFunctionVariants::PaletteCos => sort_by(
						SignedDist {
							dist: PaletteCosDist,
						},
						&mut images,
						opts,
					),
					DistanceFunctionVariants::DateTime => {
						sort_by(SignedDist { dist: DateTimeDist }, &mut images, opts)
					}
				},
				CompareFunctionVariants::ComparativeDist { compared_to, dist } => {
					let compared_to = images
						.iter()
						.find(|i| i.id == compared_to)
						.cloned()
						.ok_or(Error::NotFound(format!("image with id {}", compared_to)))?;
					match dist {
						DistanceFunctionVariants::Palette => sort_by(
							CompareDist {
								compared_to,
								dist: PaletteDist,
							},
							&mut images,
							opts,
						),
						DistanceFunctionVariants::PaletteCos => sort_by(
							CompareDist {
								compared_to,
								dist: PaletteCosDist,
							},
							&mut images,
							opts,
						),
						DistanceFunctionVariants::DateTime => sort_by(
							CompareDist {
								compared_to,
								dist: DateTimeDist,
							},
							&mut images,
							opts,
						),
					}
				}
			};

			let data = images
				.iter()
				.map(|image| UuidString(image.id))
				.collect_vec();

			rmp_serde::to_vec_named(&Layout::Sort { data }).map_err(Into::into)
		}
		LayoutOptions::Tsne(opts) => {
			let data = match opts.dist {
				DistanceFunctionVariants::Palette => tsne(PaletteDist, &mut images, opts),
				DistanceFunctionVariants::PaletteCos => tsne(PaletteCosDist, &mut images, opts),
				DistanceFunctionVariants::DateTime => tsne(DateTimeDist, &mut images, opts),
			};

			rmp_serde::to_vec_named(&Layout::Pos { data }).map_err(Into::into)
		}
	})
	.await?;

	Ok(buf)
}
