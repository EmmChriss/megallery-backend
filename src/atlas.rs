use std::fs::File;
use std::io::{BufReader, Cursor};
use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use image::io::Reader as ImageReader;
use image::ImageBuffer;
use uuid::Uuid;

use crate::db::{Collection, Db, ImageFileKind};
use crate::err::{Error, Result};
use crate::{
	get_static_atlas_path, uuid_to_string_serialize, DbExtension, Image, ImageFile,
	STATIC_ATLAS_PATH,
};

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
			let image_entry =
				match ImageFile::get_by_id(&db, m.id, m.width, m.height, ImageFileKind::Thumbnail)
					.await?
				{
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

const MAX_SIZE: u32 = 4000;

pub async fn regenerate_static_atlas(db: &Db, collection_id: Uuid) -> Result<()> {
	let mut metadata = Image::get_all_for_collection(&db, collection_id).await?;

	for meta in metadata.iter_mut() {
		let image_file = ImageFile::get_smallest(db, meta.id).await?;
		if let Some(image_file) = image_file {
			meta.width = image_file.width;
			meta.height = image_file.height;
		}
	}

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

	let file = File::create(crate::get_static_atlas_path(collection_id))?;
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
	Path(collection_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
	let collection = Collection::get_by_id(&db, collection_id)
		.await?
		.ok_or(Error::NotFound("collection".into()))?;

	if !collection.finalized {
		return Err(Error::Custom(
			StatusCode::BAD_REQUEST,
			"collection not finalized".into(),
		));
	}

	let path = get_static_atlas_path(collection.id);
	let exists = path.try_exists()?;

	if !exists {
		regenerate_static_atlas(&db, collection_id).await?;
	}

	let atlas_file = tokio::fs::File::open(path).await?;
	let reader = tokio::io::BufReader::new(atlas_file);
	let stream = tokio_util::io::ReaderStream::new(reader);

	Ok(axum::body::StreamBody::new(stream).into_response())
}
