use axum::extract::Path;
use axum::http::StatusCode;
use axum::Json;
use axum::{response::IntoResponse, Extension};
use chrono::NaiveDateTime;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::{Collection, DbExtension, Image};
use crate::err::{Error, Result};
use crate::layout::dist::{DateTimeDist, PaletteCosDist, PaletteDist};
use crate::layout::sort::{CompareDist, SignedDist};
use crate::uuid_to_string_serialize;

use self::dist::{DistanceFunction, DistanceFunctionVariants};
use self::filter::Filter;
use self::sort::{CompareFunction, CompareFunctionVariants};

mod dist;
mod filter;
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

// metadata is assumed to be sorted already
fn create_expansion_grid(
	metadata: &[Image],
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
	tsne.epochs(1000)
		.barnes_hut(0.5, move |a, b| f32::abs(dist.dist(a, b)));

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

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TimeHistOptions {
	pub resolution: TimeHistResolution,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeHistResolution {
	Hour,
	Day,
	Week,
	Month,
	Year,
}

fn time_hist(metadata: &[Image], opts: TimeHistOptions) -> Vec<Vec<Option<UuidString>>> {
	use TimeHistResolution::*;
	let group_by_fn = match opts.resolution {
		Hour => |dt: NaiveDateTime| dt.format("%Y-%j %H").to_string(),
		Day => |dt: NaiveDateTime| dt.format("%Y-%j").to_string(),
		Week => |dt: NaiveDateTime| dt.format("%Y-%W").to_string(),
		Month => |dt: NaiveDateTime| dt.format("%Y-%m").to_string(),
		Year => |dt: NaiveDateTime| dt.format("%Y").to_string(),
	};

	let mut with_metadata = metadata
		.iter()
		.filter_map(|img| img.metadata.date_time.map(|dt| (img.id, dt)))
		.collect_vec();

	with_metadata.sort_unstable_by_key(|(_, dt)| *dt);

	let groups = with_metadata
		.into_iter()
		.group_by(|(_, dt)| group_by_fn(*dt));

	groups
		.into_iter()
		.map(|(_, group)| group.map(|(id, _)| UuidString(id)).map(Some).collect_vec())
		.collect_vec()
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub enum LayoutOptions {
	Sort(SortOptions),
	GridExpansion(ExpansionGridOptions),
	TimeHist(TimeHistOptions),
	Tsne(TsneOptions),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LayoutRequest {
	#[serde(flatten)]
	opts: LayoutOptions,

	filter: Option<Filter>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub enum Layout {
	Sort {
		data: Vec<UuidString>,
	},
	Grid {
		data: Vec<Vec<Option<UuidString>>>,
		invert: bool,
	},
	Pos {
		data: Vec<(UuidString, f32, f32)>,
	},
}

pub async fn get_layout(
	Extension(db): DbExtension,
	Path(collection_id): Path<Uuid>,
	Json(layout): Json<LayoutRequest>,
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

	// Get all images
	let mut images = Image::get_all_for_collection(&db, collection_id).await?;

	// Perform filtering
	if let Some(ref filter) = layout.filter {
		images = images.into_iter().filter(|m| filter.filter(m)).collect();

		if let Some(limit) = filter.limit {
			images = Vec::from(&images[..limit]);
		}
	}

	let resp = tokio::task::spawn_blocking(move || do_layout(layout, &mut images)).await??;
	let msgp = rmp_serde::to_vec_named(&resp)?;

	Ok(msgp)
}

fn do_layout(req: LayoutRequest, images: &mut [Image]) -> Result<Layout> {
	match req.opts {
		LayoutOptions::GridExpansion(opts) => {
			// Sort images in another dispatch
			do_layout(
				LayoutRequest {
					opts: LayoutOptions::Sort(SortOptions {
						compare: opts.compare,
					}),
					filter: None,
				},
				images,
			)?;

			let data = create_expansion_grid(images, opts);

			Ok(Layout::Grid {
				data,
				invert: false,
			})
		}
		LayoutOptions::TimeHist(opts) => Ok(Layout::Grid {
			data: time_hist(images, opts),
			invert: true,
		}),
		LayoutOptions::Sort(opts) => {
			match opts.compare {
				CompareFunctionVariants::SignedDist { dist } => match dist {
					DistanceFunctionVariants::Palette => {
						sort_by(SignedDist { dist: PaletteDist }, images, opts)
					}
					DistanceFunctionVariants::PaletteCos => sort_by(
						SignedDist {
							dist: PaletteCosDist,
						},
						images,
						opts,
					),
					DistanceFunctionVariants::DateTime => {
						sort_by(SignedDist { dist: DateTimeDist }, images, opts)
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
							images,
							opts,
						),
						DistanceFunctionVariants::PaletteCos => sort_by(
							CompareDist {
								compared_to,
								dist: PaletteCosDist,
							},
							images,
							opts,
						),
						DistanceFunctionVariants::DateTime => sort_by(
							CompareDist {
								compared_to,
								dist: DateTimeDist,
							},
							images,
							opts,
						),
					}
				}
			};

			let data = images
				.iter()
				.map(|image| UuidString(image.id))
				.collect_vec();

			Ok(Layout::Sort { data })
		}
		LayoutOptions::Tsne(opts) => {
			let data = match opts.dist {
				DistanceFunctionVariants::Palette => tsne(PaletteDist, images, opts),
				DistanceFunctionVariants::PaletteCos => tsne(PaletteCosDist, images, opts),
				DistanceFunctionVariants::DateTime => tsne(DateTimeDist, images, opts),
			};

			Ok(Layout::Pos { data })
		}
	}
}
