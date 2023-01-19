mod err;
use err::{Error, Result};

use axum::{
	self, extract::Query, http::StatusCode, response::IntoResponse, routing::get, Extension, Json,
};
use dotenv;
use env_logger;
use fast_image_resize as resize;
use image::{codecs::png::PngEncoder, io::Reader as ImageReader, ImageBuffer, ImageEncoder};
use lazy_static::lazy_static;
use log;
use sqlx::postgres::PgPool;
use std::{
	fs::File,
	io::{BufReader, BufWriter, Cursor},
	net::SocketAddr,
	path::{Path, PathBuf},
	sync::Arc,
};
use uuid::Uuid;

type Db = sqlx::postgres::PgPool;
type DbExtension = Extension<Arc<Db>>;

static IMAGES_PATH: &str = "./images";

lazy_static! {
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

#[tokio::main]
async fn main() {
	// get environment, crash if missing
	let addr = dotenv::var("SERVER_ADDRESS").unwrap();
	let port = dotenv::var("SERVER_PORT").unwrap();
	let db_url = dotenv::var("DATABASE_URL").unwrap();

	// Init logger
	env_logger::init();

	// init database
	let pool: Db = PgPool::connect(&db_url).await.unwrap();
	sqlx::migrate!().run(&pool).await.unwrap();
	log::info!("Database migrations ran successfully");

	// make sure "images" directory exists
	let images = Path::new(IMAGES_PATH);
	if !images
		.try_exists()
		.expect("could not check if ./images exists")
	{
		std::fs::create_dir(images).expect("could not create ./images directory");
	}
	if !images.is_dir() {
		panic!("./images is not a directory");
	}

	// define app routes
	let state: DbExtension = Extension(Arc::new(pool));
	let app = axum::Router::new()
		.route("/images", get(get_image_metadata).post(upload_image))
		.route("/images/data", get(get_image_data))
		.layer(state);

	// start built-in server
	let addr = SocketAddr::new(addr.parse().unwrap(), port.parse().unwrap());
	axum::Server::bind(&addr)
		.serve(app.into_make_service())
		.await
		.expect("Server error");
}

struct NewImage {
	name: String,
	width: u32,
	height: u32,
}

impl NewImage {
	pub async fn insert_one(self, db: &Db) -> Result<ImageMetadata, Error> {
		let id = Uuid::new_v4();

		sqlx::query("insert into images values ($1, $2, $3, $4)")
			.bind(id)
			.bind(&self.name)
			.bind(self.width as i32)
			.bind(self.height as i32)
			.execute(db)
			.await?;

		Ok(ImageMetadata {
			id,
			name: self.name,
			width: self.width as i32,
			height: self.height as i32,
		})
	}
}

#[derive(sqlx::FromRow)]
struct ImageFile {
	image_id: Uuid,
	width: i32,
	height: i32,
	file_name: String,
}

impl ImageFile {
	pub async fn insert_one(self, db: &Db) -> Result<(), sqlx::Error> {
		sqlx::query("insert into image_files values ($1, $2, $3, $4)")
			.bind(self.image_id)
			.bind(self.width)
			.bind(self.height)
			.bind(self.file_name)
			.execute(db)
			.await
			.map(|_| ())
	}

	pub async fn get_by_id(
		db: &Db,
		id: Uuid,
		width: u32,
		height: u32,
	) -> Result<Option<Self>, sqlx::Error> {
		sqlx::query_as(
			"select * from image_files where image_id = $1 AND width = $2 AND height = $3",
		)
		.bind(id)
		.bind(width as i32)
		.bind(height as i32)
		.fetch_optional(db)
		.await
	}
}

async fn save_image(db: &Db, buf: &[u8], width: u32, height: u32, id: Uuid) -> Result<(), Error> {
	// Write destination image as PNG-file
	let mut id_buf = Uuid::encode_buffer();
	let id_str = id.hyphenated().encode_lower(&mut id_buf);

	let file_name = format!("{}/{}x{}.png", id_str, width, height);

	let mut path = PathBuf::new();
	path.push(IMAGES_PATH);
	path.push(id_str);
	std::fs::create_dir_all(&path)?;

	path.push(format!("{}x{}", width, height));
	path.set_extension("png");

	let mut result_buf = BufWriter::new(File::create(&path)?);
	PngEncoder::new(&mut result_buf).write_image(buf, width, height, image::ColorType::Rgb8)?;

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

async fn upload_image(
	Extension(db): DbExtension,
	mut req: axum::extract::Multipart,
) -> Result<Json<ImageMetadata>> {
	measure_time::debug_time!("responding");

	// read multipart data
	// @TODO: ward off duplicate values
	// @TODO: limit file size
	// @TODO: write to fs while receiving
	let mut name = None;
	let mut data = None;
	{
		measure_time::debug_time!("receiving data");

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
	// @TODO: do this in a background task
	{
		measure_time::debug_time!("saving images");

		let transaction = db.begin().await?;

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

			if width == 0 || height == 0 {
				break;
			}

			measure_time::debug_time!(
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
		transaction.commit().await?;
	}

	Ok(Json(meta))
}

#[derive(sqlx::FromRow, serde::Serialize, Default)]
struct ImageMetadata {
	id: sqlx::types::Uuid,
	name: String,
	width: i32,
	height: i32,
}

impl ImageMetadata {
	pub async fn get_all(db: &Db) -> sqlx::Result<Vec<Self>> {
		sqlx::query_as("select id, name, width, height from images")
			.fetch_all(db)
			.await
	}

	pub async fn get_by_id(db: &Db, id: sqlx::types::Uuid) -> sqlx::Result<Option<Self>> {
		sqlx::query_as("select * from images where id = $1")
			.bind(id)
			.fetch_optional(db)
			.await
	}
}

async fn get_image_metadata(Extension(db): DbExtension) -> Result<Json<Vec<ImageMetadata>>> {
	Ok(Json(ImageMetadata::get_all(&db).await?))
}

#[derive(sqlx::FromRow, serde::Serialize, Default)]
struct Image {
	id: Uuid,
	name: String,
	image: Vec<u8>,
	width: i32,
	height: i32,
}

impl Image {
	pub async fn get_by_id(db: &Db, id: Uuid) -> sqlx::Result<Option<Self>> {
		sqlx::query_as("select * from images where id = $1")
			.bind(id)
			.fetch_optional(db)
			.await
	}
}

#[derive(serde::Serialize)]
struct ImageResponseMetadata {
	id: sqlx::types::Uuid,
	name: String,
	width: u32,
	height: u32,
	x: u32,
	y: u32,
}

#[derive(serde::Serialize)]
struct ImageResponse {
	metadata: Vec<ImageResponseMetadata>,
	width: u32,
	height: u32,
	data: Vec<u8>,
}

#[derive(serde::Deserialize)]
struct ImageRequest {
	// name: Option<String>,
	// name_like: Option<String>,
	id: Option<Uuid>,
	limit: Option<usize>,
	width: i32,
	height: i32,
}

async fn get_image_data(
	Extension(db): DbExtension,
	Query(req): Query<ImageRequest>,
) -> Result<impl IntoResponse> {
	if req.width <= 1 || req.height <= 1 {
		return Err(Error::Custom(
			StatusCode::BAD_REQUEST,
			"bad image size".into(),
		));
	}

	// query metadata
	let mut metadata = if let Some(id) = req.id {
		let m = ImageMetadata::get_by_id(&db, id)
			.await?
			.ok_or(Error::Custom(
				StatusCode::NOT_FOUND,
				"no such image id".into(),
			))?;
		vec![m]
	} else {
		ImageMetadata::get_all(&db).await?
	};

	// truncate metadata list if it contains too many items
	// @TODO: add LIMIT option to query
	if let Some(limit) = req.limit {
		metadata.truncate(limit);
	}

	// size down images until they fit the specified size
	for m in metadata.iter_mut() {
		while m.width > req.width || m.height > req.height {
			m.width /= 2;
			m.height /= 2;
		}
	}

	// sort images by height to reduce amount of wasted space
	metadata.sort_by_key(|m| m.height);

	// approximate total image width by area
	let total_area: u64 = metadata.iter().map(|m| m.height * m.width).sum::<i32>() as u64;
	let row_width = f64::sqrt(total_area as f64).trunc() as u32;

	// place images in row-first order
	// allows large images to take up more space than available, growing buf_width
	let resp_metadata: Vec<_>;
	let mut buf_width: u32;
	let mut buf_height: u32;
	{
		let mut start_x = 0u32;
		let mut start_y = 0u32;
		let mut row_height = 0u32;

		buf_width = 0u32;
		buf_height = 0u32;

		resp_metadata = metadata
			.into_iter()
			.map(|m| {
				if start_x > 0 && start_x + m.width as u32 > row_width {
					start_x = 0;
					start_y += row_height;
					buf_height += row_height;
					row_height = 0;
				}
				buf_width = buf_width.max(start_x + m.width as u32);
				row_height = row_height.max(m.height as u32);

				let v = ImageResponseMetadata {
					id: m.id,
					name: m.name,
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

	// construct image buffer
	// copy resized images into it
	// @TODO: parallelize
	let mut img_atlas: image::RgbaImage = ImageBuffer::new(buf_width, buf_height);
	for m in resp_metadata.iter() {
		let image_entry =
			match ImageFile::get_by_id(&db, m.id, m.width as u32, m.height as u32).await? {
				None => continue, // @TODO: handle missing data
				Some(s) => s,
			};

		// load and resize image to the given bounds
		let mut path = PathBuf::new();
		path.push(IMAGES_PATH);
		path.push(&image_entry.file_name);

		let file = match File::open(&path) {
			Ok(o) => o,
			Err(_) => continue, // @TODO: handle missing files
		};
		let reader = BufReader::new(file);
		let img = ImageReader::with_format(reader, image::ImageFormat::Png).decode()?;

		// copy image into atlas buffer
		image::imageops::replace(&mut img_atlas, &img, m.x as i64, m.y as i64);
	}

	// write image atlas into buffer
	let mut image_buf = Vec::new();
	img_atlas.write_to(
		&mut Cursor::new(&mut image_buf),
		image::ImageOutputFormat::Png,
	)?;

	// write it all to a byte buffer
	let mut buf = Vec::new();
	rmp::encode::write_map_len(&mut buf, 4)?;

	// write metadata into buffer
	rmp::encode::write_str(&mut buf, "metadata")?;
	rmp_serde::encode::write_named(&mut buf, &resp_metadata)?;

	// write data into buffer
	rmp::encode::write_str(&mut buf, "data")?;

	rmp::encode::write_bin(&mut buf, &image_buf)?;

	// write dimensions
	rmp::encode::write_str(&mut buf, "width")?;
	rmp::encode::write_u32(&mut buf, img_atlas.width())?;

	rmp::encode::write_str(&mut buf, "height")?;
	rmp::encode::write_u32(&mut buf, img_atlas.height())?;

	log::info!("{}", image_buf.len());

	Ok(buf)
}
