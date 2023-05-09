use std::io::Cursor;

use axum::{http::StatusCode, Extension, Json};
use fast_image_resize as resize;
use image::io::Reader as ImageReader;
use tokio::{
	fs::File,
	io::{AsyncWriteExt, BufWriter},
};
use uuid::Uuid;

use crate::{
	db::{Db, DbExtension, Image, ImageFile, NewImage},
	err::{Error, Result},
};

lazy_static::lazy_static! {
	static ref RESIZE_CPU_EXTENSION: resize::CpuExtensions = {
		if resize::CpuExtensions::Avx2.is_supported() {
			resize::CpuExtensions::Avx2
		} else if resize::CpuExtensions::Sse4_1.is_supported() {
			resize::CpuExtensions::Sse4_1
		} else {
			resize::CpuExtensions::None
		}
	};
}

pub async fn save_image(
	db: &Db,
	buf: &[u8],
	width: u32,
	height: u32,
	id: Uuid,
	format: image::ImageFormat,
	color: image::ColorType,
) -> Result<(), Error> {
	// Write destination image as JPG-file
	let image_file = ImageFile {
		image_id: id,
		width,
		height,
		extension: format.extensions_str()[0].to_owned(),
	};

	let path = image_file.get_path();
	image::save_buffer_with_format(path, buf, width, height, color, format)?;

	// If this succeeded, save entry in db
	image_file.insert_one(db).await?;

	Ok(())
}

pub async fn save_image_thumbnails(
	db: &Db,
	meta: Image,
	img: image::DynamicImage,
) -> Result<(), Error> {
	measure_time::warn_time!("saving images");

	// make sure format is rgb8
	let img: image::DynamicImage = match img.as_rgb8() {
		Some(_) => img,
		None => img.to_rgb8().into(),
	};

	let width_ = std::num::NonZeroU32::new(img.width()).unwrap();
	let height_ = std::num::NonZeroU32::new(img.height()).unwrap();

	let src_image = resize::Image::from_vec_u8(
		width_,
		height_,
		img.to_rgb8().into_raw(),
		resize::PixelType::U8x3,
	)?;

	let mut width = img.width();
	let mut height = img.height();
	loop {
		width /= 2;
		height /= 2;

		if width < 5 || height < 5 {
			break;
		}

		measure_time::warn_time!(
			"resizing {}x{} -> {}x{}",
			img.width(),
			img.height(),
			width,
			height
		);

		let dst_width = std::num::NonZeroU32::new(width).unwrap();
		let dst_height = std::num::NonZeroU32::new(height).unwrap();
		let mut dst_image = resize::Image::new(dst_width, dst_height, src_image.pixel_type());

		let mut dst_view = dst_image.view_mut();

		let mut resizer = resize::Resizer::new(resize::ResizeAlg::Nearest);

		// @SAFETY
		// an unsupported CPU extension will only be set if it is incorrectly reported
		// RESIZE_CPU_EXTENSION checks at runtime, and only keeps supported extensions
		unsafe {
			resizer.set_cpu_extensions(**&RESIZE_CPU_EXTENSION);
		}
		resizer.resize(&src_image.view(), &mut dst_view).unwrap();

		save_image(
			&db,
			dst_image.buffer(),
			width,
			height,
			meta.id,
			image::ImageFormat::Jpeg,
			image::ColorType::Rgb8,
		)
		.await?;
	}

	Ok(())
}

pub async fn upload_image(
	Extension(db): DbExtension,
	mut req: axum::extract::Multipart,
) -> Result<Json<Image>> {
	measure_time::warn_time!("responding");

	// read multipart data
	// @TODO: ward off duplicate values
	// @TODO: limit file size
	// @TODO: write to fs while receiving
	let mut name = None;
	let mut data = None;
	{
		measure_time::warn_time!("receiving data");

		while let Some(field) = req.next_field().await? {
			let field_name = field.name().ok_or(Error::MultipartMissingName)?;
			match field_name {
				"name" => name = Some(field.text().await?),
				"data" => data = Some(field.bytes().await?),
				_ => {
					return Err(Error::Custom(
						StatusCode::BAD_REQUEST,
						format!("unknown field: {}", field_name),
					))
				}
			}
		}
	}

	// if either field is missing
	let name = name.ok_or(Error::MultipartMissingField("name".into()))?;
	let data = data.ok_or(Error::MultipartMissingField("data".into()))?;

	// read image, make sure format is correct
	let img = ImageReader::new(Cursor::new(&data)).with_guessed_format()?;
	let format = img.format();
	let img = img.decode()?;

	// construct new dto for insertion, return metadata
	let meta = NewImage {
		name: name.into(),
		width: img.width(),
		height: img.height(),
	}
	.insert_one(&db)
	.await?;

	// save original version without modifying anything
	let extension = format.unwrap().extensions_str()[0].to_owned();
	let image_file = ImageFile {
		image_id: meta.id,
		width: img.width(),
		height: img.height(),
		extension,
	};
	let path = image_file.get_path();

	std::fs::create_dir_all(&path)?;
	let mut writer = BufWriter::new(File::create(path).await?);
	writer.write_all(&data).await?;
	image_file.insert_one(&db).await?;

	// create and save image versions
	let meta_ = meta.clone();
	tokio::spawn(async move {
		let res = save_image_thumbnails(&db.clone(), meta_, img).await;
		if let Err(e) = res {
			log::error!("error during saving image versions: {}", e);
		}
	});

	Ok(Json(meta))
}
