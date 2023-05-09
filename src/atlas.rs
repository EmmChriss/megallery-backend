use std::fs::File;
use std::io::{BufReader, Cursor};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use image::io::Reader as ImageReader;
use image::ImageBuffer;
use uuid::Uuid;

use crate::db::Db;
use crate::err::{Error, Result};
use crate::{uuid_to_string_serialize, DbExtension, Image, ImageFile, STATIC_ATLAS_PATH};

pub const ATLAS_REGEN_TIME: Duration = Duration::from_secs(60 * 15);

pub type AtlasTriggerExtension = Extension<Arc<tokio::sync::Mutex<AtlasTrigger>>>;

pub struct AtlasTrigger {
	last_update: Instant,
}

impl AtlasTrigger {
	pub fn new() -> Self {
		Self {
			last_update: Instant::now(),
		}
	}

	pub fn should_start_regen(&self) -> bool {
		let now = Instant::now();
		if now.duration_since(self.last_update) > ATLAS_REGEN_TIME {
			true
		} else {
			false
		}
	}

	pub fn start_regen(&mut self) {
		self.last_update = Instant::now();
	}
}

#[derive(serde::Deserialize)]
pub struct DynamicAtlasRequest {
	// filter
	// name_exact: Option<String>,
	// name_like: Option<String>,
	id: Option<Uuid>,
	id_list: Option<Vec<Uuid>>,
	limit: Option<u32>,

	// params
	icon_max_width: u32,
	icon_max_height: u32,
	atlas_max_area: u32,
}

#[derive(serde::Serialize, Clone)]
pub struct AtlasMapping {
	#[serde(serialize_with = "uuid_to_string_serialize")]
	id: Uuid,
	width: u32,
	height: u32,
	x: u32,
	y: u32,
}

#[derive(serde::Serialize)]
pub struct AtlasResponse {
	#[serde(with = "serde_bytes")]
	data: Vec<u8>,
	mapping: Vec<AtlasMapping>,
}

#[derive(serde::Serialize)]
pub struct AtlasFormat<'a> {
	#[serde(with = "serde_bytes")]
	data: &'a [u8],
	mapping: Vec<AtlasMapping>,
}

fn gen_atlas(meta: &[Image], max_size: u32) -> (Vec<AtlasMapping>, (u32, u32)) {
	let total_area = meta.iter().map(|m| m.height * m.width).sum::<u32>();
	let row_width = f64::sqrt(total_area as f64).trunc() as u32;
	let row_width = row_width.min(max_size);

	let mut mapping = vec![];
	let mut current_meta = &meta[..];
	let mut buf_height = 0;
	let mut buf_width = row_width;

	loop {
		let mut width = 0;
		let mut height = 0;
		let row: Vec<_> = current_meta
			.iter()
			.take_while(|m| {
				if width + m.width < buf_width {
					width += m.width;
					height = height.max(m.height);
					buf_width = buf_width.max(width);
					true
				} else {
					false
				}
			})
			.collect();

		// shift current_meta
		current_meta = &current_meta[row.len()..];

		// break if row too large or empty
		if buf_height + height > max_size || row.len() == 0 {
			break;
		}

		// emplace images in buffer
		let mut x = 0;
		for m in row {
			mapping.push(AtlasMapping {
				id: m.id,
				width: m.width,
				height: m.height,
				x,
				y: buf_height,
			});

			x += m.width;
		}
		buf_height += height;
	}

	(mapping, (buf_width, buf_height))
}

async fn build_atlas(
	db: &Db,
	mapping: &[AtlasMapping],
	width: u32,
	height: u32,
) -> Result<image::RgbaImage> {
	// construct image buffer and copy resized images into it
	let mut img_atlas: image::RgbaImage = ImageBuffer::new(width, height);

	let img_atlas_mutex = Arc::new(futures::lock::Mutex::new(&mut img_atlas));
	let iter_future = mapping
		.iter()
		.map(|m| (m, img_atlas_mutex.clone(), db.clone()))
		.map(|(m, image_atlas, db)| async move {
			// load image entry from db
			let image_entry = match ImageFile::get_by_id(&db, m.id, m.width, m.height).await? {
				None => return Ok::<(), Error>(()), // @TODO handle missing image entry
				Some(s) => s,
			};

			// load and resize image to the given bounds
			let path = image_entry.get_path();

			// read file in background task
			let img = tokio::task::spawn_blocking(move || {
				let file = File::open(&path)?; // @TODO: handle open error
				let reader = BufReader::new(file);
				let img = ImageReader::with_format(reader, image::ImageFormat::Jpeg).decode()?;
				return Ok::<_, Error>(img);
			})
			.await?;

			let img = match img {
				Err(Error::ImageError(_)) => return Ok(()),
				Err(err) => return Err(err),
				Ok(ok) => Ok::<_, Error>(ok),
			}?;

			// lock underlying data and write to it
			let mut img_atlas = image_atlas.lock().await;

			// copy image into atlas buffer
			image::imageops::replace(*img_atlas, &img, m.x as i64, m.y as i64);

			Ok(())
		});

	futures_util::future::try_join_all(iter_future).await?;

	Ok(img_atlas)
}

pub async fn get_dynamic_atlas(
	Extension(db): DbExtension,
	Json(req): Json<DynamicAtlasRequest>,
) -> Result<impl IntoResponse> {
	if req.atlas_max_area <= 1 {
		return Err(Error::Custom(
			StatusCode::BAD_REQUEST,
			"bad image size".into(),
		));
	}

	// dispatch metadata query based on request
	let mut metadata = match (req.limit, req.id, &req.id_list) {
		(_, Some(id), _) => vec![Image::get_by_id(&db, id).await?.ok_or(Error::Custom(
			StatusCode::NOT_FOUND,
			"no such image id".into(),
		))?],
		(_, _, Some(id_list)) => Image::get_by_id_list(&db, &id_list).await?,
		(Some(limit), _, _) => Image::get_all_with_limit(&db, limit).await?,
		(_, _, _) => Image::get_all(&db).await?,
	};

	// size down images until they fit the specified icon size
	for m in metadata.iter_mut() {
		while m.width > req.icon_max_width || m.height > req.icon_max_height {
			m.width /= 2;
			m.height /= 2;
		}
	}

	// approximate total atlas area
	let mut total_area = metadata.iter().map(|m| m.height * m.width).sum::<u32>();

	let mut downsize_factor = 0;
	while total_area > req.atlas_max_area {
		downsize_factor += 1;
		total_area /= 4;
	}

	if downsize_factor > 0 {
		for m in metadata.iter_mut() {
			for _ in 0..downsize_factor {
				m.width /= 2;
				m.height /= 2;
			}
		}
	}

	// sort images by height to reduce amount of wasted space
	metadata.sort_unstable_by_key(|m| u32::MAX - m.height);

	let atlas_max_sidelen = (req.atlas_max_area as f64).sqrt().trunc() as u32;
	let (mapping, (buf_width, buf_height)) = gen_atlas(&metadata, atlas_max_sidelen);

	let img_atlas = build_atlas(&db, &mapping, buf_width, buf_height).await?;

	let mut image_buf = Vec::with_capacity((img_atlas.width() * img_atlas.height() * 4) as usize);

	// write image atlas into buffer
	img_atlas.write_to(
		&mut Cursor::new(&mut image_buf),
		image::ImageOutputFormat::Png,
	)?;

	let mut buf = Vec::with_capacity(image_buf.len() + 10000);
	{
		measure_time::info_time!("serialization of {} elements", mapping.len(),);

		let response = AtlasResponse {
			mapping,
			data: image_buf,
		};

		// write it all to a byte buffer
		rmp_serde::encode::write_named(&mut buf, &response)?;
	}

	Ok(buf)
}

const MAX_AREA: u32 = 1500;
const MAX_SIZE: u32 = 4000;

pub async fn regenerate_static_atlas(db: &Db) -> Result<()> {
	let mut metadata = Image::get_all(&db).await?;

	metadata.iter_mut().for_each(|meta| {
		while meta.width * meta.height > MAX_AREA {
			meta.width /= 2;
			meta.height /= 2;
		}
	});

	metadata.sort_unstable_by_key(|m| u32::MAX - m.height);

	let mut mappings = vec![];
	let mut offset = 0;
	loop {
		let mapping = gen_atlas(&metadata[offset..], MAX_SIZE);

		let size = mapping.0.len();
		offset += size;

		mappings.push(mapping);
		if offset >= metadata.len() {
			break;
		}
	}

	let file = File::create(STATIC_ATLAS_PATH)?;
	let mut writer = std::io::BufWriter::new(file);
	rmp::encode::write_array_len(&mut writer, mappings.len() as u32)?;

	let mut img_buf = vec![];
	for (mapping, (width, height)) in mappings {
		let img_atlas = build_atlas(&db, &mapping, width, height).await?;

		img_buf.clear();
		img_atlas.write_to(
			&mut Cursor::new(&mut img_buf),
			image::ImageOutputFormat::Jpeg(255),
		)?;

		rmp_serde::encode::write_named(
			&mut writer,
			&AtlasFormat {
				data: &img_buf,
				mapping,
			},
		)?;
	}

	Ok(())
}

pub async fn get_static_atlas(
	Extension(db): DbExtension,
	Extension(atlas_trigger): AtlasTriggerExtension,
) -> Result<impl IntoResponse> {
	let mut atlas_trigger = atlas_trigger.lock_owned().await;
	let atlas_exists = std::path::Path::new(STATIC_ATLAS_PATH).exists();
	if !atlas_exists || atlas_trigger.should_start_regen() {
		atlas_trigger.start_regen();

		let db = db.clone();
		let handle = tokio::spawn(async move {
			match regenerate_static_atlas(&db).await {
				Ok(_) => (),
				Err(e) => log::error!("could not regenerate static atlas: {}", e),
			}
		});

		if !atlas_exists {
			handle.await?;
		}
	}

	let atlas_file = tokio::fs::File::open(STATIC_ATLAS_PATH).await?;
	let reader = tokio::io::BufReader::new(atlas_file);
	let stream = tokio_util::io::ReaderStream::new(reader);
	Ok(axum::body::StreamBody::new(stream).into_response())
}
