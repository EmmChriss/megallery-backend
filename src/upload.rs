use std::{
	fs::File,
	io::{BufWriter, Cursor},
	path::PathBuf,
};

use axum::{http::StatusCode, Extension, Json};
use fast_image_resize as resize;
use image::{codecs::jpeg::JpegEncoder, io::Reader as ImageReader, ImageEncoder};
use uuid::Uuid;

use crate::{
	db::{Db, DbExtension, Image, ImageFile, NewImage},
	err::{Error, Result},
	uuid_to_string, IMAGES_PATH,
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
) -> Result<(), Error> {
	// Write destination image as JPG-file
	let id_str = uuid_to_string(&id);
	let file_name = format!("{}/{}x{}.jpg", id_str, width, height);

	let mut path = PathBuf::new();
	path.push(IMAGES_PATH);
	path.push(id_str);
	std::fs::create_dir_all(&path)?;

	path.push(format!("{}x{}", width, height));
	path.set_extension("jpg");

	let mut result_buf = BufWriter::new(File::create(&path)?);
	JpegEncoder::new(&mut result_buf).write_image(buf, width, height, image::ColorType::Rgb8)?;

	// If this succeeded, save entry in db
	ImageFile {
		image_id: id,
		width: width.try_into().unwrap(),
		height: height.try_into().unwrap(),
		file_name,
	}
	.insert_one(db)
	.await?;

	Ok(())
}

pub async fn save_image_versions(
	db: &Db,
	meta: Image,
	img: image::DynamicImage,
) -> Result<(), Error> {
	measure_time::warn_time!("saving images");

	// first off, save original version
	save_image(
		&db,
		img.as_rgb8().unwrap(),
		img.width(),
		img.height(),
		meta.id,
	)
	.await?;

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

		save_image(&db, dst_image.buffer(), width, height, meta.id).await?;
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
	let img = ImageReader::new(Cursor::new(&data))
		.with_guessed_format()?
		.decode()?;

	// construct new dto for insertion, return metadata
	let meta = NewImage {
		name: name.into(),
		width: img.width(),
		height: img.height(),
	}
	.insert_one(&db)
	.await?;

	// make sure format is rgb8
	let img: image::DynamicImage = match img.as_rgb8() {
		Some(_) => img,
		None => img.to_rgb8().into(),
	};

	// create and save image versions
	let meta_ = meta.clone();
	tokio::spawn(async move {
		let res = save_image_versions(&db.clone(), meta_, img).await;
		if let Err(e) = res {
			log::error!("error during saving image versions: {}", e);
		}
	});

	Ok(Json(meta))
}
