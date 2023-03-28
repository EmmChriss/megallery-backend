use std::fs::File;
use std::io::{BufReader, Cursor};
use std::path::PathBuf;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use image::io::Reader as ImageReader;
use image::ImageBuffer;
use uuid::Uuid;

use crate::err::{Error, Result};
use crate::{uuid_to_string_serialize, DbExtension, Image, ImageFile, IMAGES_PATH};

#[derive(serde::Deserialize)]
pub struct ImageDataRequest {
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

#[derive(serde::Serialize)]
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

pub async fn get_atlas(
	Extension(db): DbExtension,
	Json(req): Json<ImageDataRequest>,
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
		// (_, _, Some(id_list)) => Image::get_by_id_list(&db, id_list).await?,
		(Some(limit), _, _) => Image::get_all_with_limit(&db, limit).await?,
		(_, _, _) => Image::get_all(&db).await?,
	};

	// temporary
	if let Some(id_list) = req.id_list {
		metadata = metadata
			.into_iter()
			.filter(|m| id_list.contains(&m.id))
			.collect();
	}

	// size down images until they fit the specified icon size
	for m in metadata.iter_mut() {
		while m.width as u32 > req.icon_max_width || m.height as u32 > req.icon_max_height {
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

	let row_width = f64::sqrt(total_area as f64).trunc() as u32;

	// place images in row-first order
	// allows large images to take up more space than available, growing buf_width
	let mapping: Vec<_>;
	let mut buf_width: u32;
	let mut buf_height: u32;
	{
		let mut start_x = 0u32;
		let mut start_y = 0u32;
		let mut row_height = 0u32;

		buf_width = row_width;
		buf_height = 0u32;

		mapping = metadata
			.into_iter()
			.map(|m| {
				// allow placing any sized image on start of the row
				if start_x > 0 && start_x + m.width as u32 > buf_width {
					start_x = 0;
					start_y += row_height;
					buf_height += row_height;
					row_height = 0;
				}
				buf_width = buf_width.max(start_x + m.width as u32);
				row_height = row_height.max(m.height as u32);

				let v = AtlasMapping {
					id: m.id,
					width: m.width as u32,
					height: m.height as u32,
					x: start_x,
					y: start_y,
				};

				start_x += m.width as u32;

				v
			})
			.collect();

		buf_height += row_height;
	}

	// construct image buffer and copy resized images into it
	let mut img_atlas: image::RgbaImage = ImageBuffer::new(buf_width, buf_height);
	{
		measure_time::info_time!(
			"placing {} images on an image atlas of {}x{}",
			mapping.len(),
			img_atlas.width(),
			img_atlas.height()
		);

		let img_atlas_mutex = Arc::new(futures::lock::Mutex::new(&mut img_atlas));
		let iter_future = mapping
			.iter()
			.map(|m| (m, img_atlas_mutex.clone(), db.clone()))
			.map(|(m, image_atlas, db)| async move {
				// load image entry from db
				let image_entry =
					match ImageFile::get_by_id(&db, m.id, m.width as u32, m.height as u32).await? {
						None => return Ok::<(), Error>(()), // @TODO handle missing image entry
						Some(s) => s,
					};

				// load and resize image to the given bounds
				let mut path = PathBuf::new();
				path.push(IMAGES_PATH);
				path.push(&image_entry.file_name);

				// read file in background task
				let img = tokio::task::spawn_blocking(move || {
					let file = File::open(&path).ok()?; // @TODO: handle open error
					let reader = BufReader::new(file);
					let img = ImageReader::with_format(reader, image::ImageFormat::Png)
						.decode()
						.ok()?;
					return Some(img);
				})
				.await
				.map_err(|_| Error::GenericInternalError)?
				.ok_or(Error::GenericInternalError)?;

				// lock underlying data and write to it
				let mut img_atlas = image_atlas.lock().await;

				// copy image into atlas buffer
				image::imageops::replace(*img_atlas, &img, m.x as i64, m.y as i64);

				Ok(())
			});

		futures_util::future::try_join_all(iter_future).await?;
	}

	let mut image_buf = Vec::with_capacity((img_atlas.width() * img_atlas.height() * 4) as usize);
	{
		measure_time::info_time!(
			"encoding image atlas of {}x{}",
			img_atlas.width(),
			img_atlas.height()
		);

		// write image atlas into buffer
		img_atlas.write_to(
			&mut Cursor::new(&mut image_buf),
			image::ImageOutputFormat::Png,
		)?;
	}

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
