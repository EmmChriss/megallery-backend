use std::io::Cursor;

use axum::{extract::Path, http::StatusCode, response::IntoResponse, Extension, Json};
use fast_image_resize as resize;
use image::io::Reader as ImageReader;
use tokio::{
	fs::File,
	io::{AsyncWriteExt, BufWriter},
};
use uuid::Uuid;

use crate::{
	atlas::regenerate_static_atlas,
	db::{Collection, Db, DbExtension, Image, ImageFile, ImageFileKind, NewImage},
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
		kind: ImageFileKind::Thumbnail,
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

	let width = img.width();
	let height = img.height();

	let largest_that_fits = |(w, h): (u32, u32)| {
		let wr = (width as f32) / (w as f32);
		let hr = (height as f32) / (h as f32);
		let m = f32::max(wr, hr);

		// don't attempt upscaling
		if m <= 1. {
			None
		} else {
			Some(((width as f32 / m) as u32, (height as f32 / m) as u32))
		}
	};

	let sizes = [
		// save original size reencoded in jpg
		Some((width, height)),
		// save small thumbnail
		largest_that_fits((100, 100)),
		// save large thumbnail
		largest_that_fits((500, 500)),
		// save giga thumbnail
		largest_that_fits((1000, 1000)),
	];

	for size in sizes {
		measure_time::warn_time!(
			"resizing {}x{} -> {}x{}",
			img.width(),
			img.height(),
			width,
			height
		);

		// only count in sizes smaller than the original image
		let (width, height) = match size {
			Some(size) => size,
			None => continue,
		};

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
	Path(collection_id): Path<Uuid>,
	mut req: axum::extract::Multipart,
) -> Result<Json<Image>> {
	measure_time::warn_time!("responding");

	// make sure collection exists and is not finalized
	let mut collection = Collection::get_by_id(&db, collection_id)
		.await?
		.ok_or(Error::NotFound("collection".into()))?;

	if collection.finalized {
		collection.finalized = false;
		collection.save(&db).await?;
	}

	// read multipart data
	// @TODO: ward off duplicate values
	// @TODO: limit file size
	// @TODO: write to fs while receiving
	let mut file_name = None;
	let mut data = None;
	{
		measure_time::warn_time!("receiving data");

		while let Some(field) = req.next_field().await? {
			let field_name = field.name().ok_or(Error::MultipartMissingName)?;
			if field_name != "image" {
				return Err(Error::Custom(
					StatusCode::BAD_REQUEST,
					format!("unknown field: {}", field_name),
				));
			}

			file_name = field.file_name().map(String::from);
			data = Some(field.bytes().await?);
		}
	}

	// if either field is missing
	// let name = name.ok_or(Error::MultipartMissingField("name".into()))?;
	let data = data.ok_or(Error::MultipartMissingField("data".into()))?;

	// read image, make sure format is correct
	let img = ImageReader::new(Cursor::new(&data)).with_guessed_format()?;
	let format = img.format();
	let img = img.decode()?;

	// construct new dto for insertion, return metadata
	let meta = NewImage {
		width: img.width(),
		height: img.height(),
		collection_id,
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
		kind: ImageFileKind::Original,
	};
	let path = image_file.get_path();

	let mut dirname = path.clone();
	dirname.pop();

	std::fs::create_dir_all(&dirname)?;
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

pub async fn finalize_collection(
	Extension(db): DbExtension,
	Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
	let collection = Collection::get_by_id(&db, id)
		.await?
		.ok_or(Error::NotFound("collection".into()))?;

	if collection.finalized {
		return Err(Error::Custom(
			StatusCode::BAD_REQUEST,
			"collection already finalized".into(),
		));
	}

	regenerate_static_atlas(&db, id).await?;

	Ok(())
}
