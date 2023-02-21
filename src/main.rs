mod err;
use err::{Error, Result};

use axum::{
	self,
	body::StreamBody,
	extract::WebSocketUpgrade,
	http::StatusCode,
	response::IntoResponse,
	routing::{get, post},
	Extension, Json,
};
use dotenv;
use env_logger;
use fast_image_resize as resize;
use futures::{stream::FuturesOrdered, FutureExt, StreamExt, TryStreamExt};
use image::{codecs::png::PngEncoder, io::Reader as ImageReader, ImageBuffer, ImageEncoder};
use lazy_static::lazy_static;
use log;
use sqlx::postgres::PgPool;
use std::{
	cell::RefCell,
	fs::File,
	io::{BufReader, BufWriter, Cursor},
	net::SocketAddr,
	path::{Path, PathBuf},
	sync::{atomic::AtomicU32, Arc},
};
use tokio::{
	io::{AsyncBufRead, AsyncReadExt},
	sync::{Mutex, RwLock},
};
use tower_http::compression::CompressionLayer;
use uuid::Uuid;

type Db = sqlx::postgres::PgPool;
type DbExtension = Extension<Arc<Db>>;

static IMAGES_PATH: &str = "./images";

const RESPONSE_MAX_SIZE: u64 = 512 * 1024 * 1024;

fn uuid_to_string(id: &Uuid) -> String {
	let mut id_buf = Uuid::encode_buffer();
	let id_str = id.hyphenated().encode_lower(&mut id_buf);
	return id_str.to_string();
}

fn uuid_to_string_serialize<S>(id: &Uuid, ser: S) -> Result<S::Ok, S::Error>
where
	S: serde::Serializer,
{
	let id_str = uuid_to_string(id);
	ser.serialize_str(&id_str)
}

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
		.route("/images/data", post(get_image_data))
		.route("/images/data_new", post(get_image_data_for_images))
		.layer(state)
		.layer(CompressionLayer::new());

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
	pub async fn insert_one(self, db: &Db) -> Result<Image, Error> {
		let id = Uuid::new_v4();

		sqlx::query("insert into images values ($1, $2, $3, $4)")
			.bind(id)
			.bind(&self.name)
			.bind(self.width as i32)
			.bind(self.height as i32)
			.execute(db)
			.await?;

		Ok(Image {
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

	pub async fn get_by_max_size(
		db: &Db,
		id: Uuid,
		max_width: u32,
		max_height: u32,
	) -> Result<Option<Self>, sqlx::Error> {
		sqlx::query_as(
			"
			SELECT * FROM image_files
			WHERE image_id = $1 AND width <= $2 AND height <= $3
			ORDER BY width DESC, height DESC
			LIMIT 1",
		)
		.bind(id)
		.bind(max_width as i32)
		.bind(max_height as i32)
		.fetch_optional(db)
		.await
	}
}

async fn save_image(db: &Db, buf: &[u8], width: u32, height: u32, id: Uuid) -> Result<(), Error> {
	// Write destination image as PNG-file
	let id_str = uuid_to_string(&id);
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
	// @TODO: do this in a background task
	{
		measure_time::warn_time!("saving images");

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
		transaction.commit().await?;
	}

	Ok(Json(meta))
}

#[derive(sqlx::FromRow, serde::Serialize, Default)]
struct Image {
	id: sqlx::types::Uuid,
	name: String,
	width: i32,
	height: i32,
}

impl Image {
	pub async fn get_all(db: &Db) -> sqlx::Result<Vec<Self>> {
		sqlx::query_as("select id, name, width, height from images")
			.fetch_all(db)
			.await
	}

	pub async fn get_all_with_limit(db: &Db, limit: u32) -> sqlx::Result<Vec<Self>> {
		sqlx::query_as("select id, name, width, height from images limit $1")
			.bind(limit as i64)
			.fetch_all(db)
			.await
	}

	pub async fn get_by_id(db: &Db, id: sqlx::types::Uuid) -> sqlx::Result<Option<Self>> {
		sqlx::query_as("select * from images where id = $1")
			.bind(id)
			.fetch_optional(db)
			.await
	}

	pub async fn get_by_id_list(
		db: &Db,
		id_list: Vec<sqlx::types::Uuid>,
	) -> sqlx::Result<Vec<Self>> {
		sqlx::query_as("select * from images where id in $1")
			.bind(id_list)
			.fetch_all(db)
			.await
	}
}

async fn get_image_metadata(Extension(db): DbExtension) -> Result<Json<Vec<Image>>> {
	Ok(Json(Image::get_all(&db).await?))
}

#[derive(serde::Deserialize)]
struct ImageDataRequest {
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
struct AtlasMapping {
	#[serde(serialize_with = "uuid_to_string_serialize")]
	id: Uuid,
	width: u32,
	height: u32,
	x: u32,
	y: u32,
}

#[derive(serde::Serialize)]
struct ImageDataResponse {
	#[serde(with = "serde_bytes")]
	data: Vec<u8>,
	mapping: Vec<AtlasMapping>,
}

async fn get_image_data(
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
	let mut total_area = metadata.iter().map(|m| m.height * m.width).sum::<i32>() as u32;

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
	metadata.sort_by_key(|m| -m.height);

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

		let response = ImageDataResponse {
			mapping,
			data: image_buf,
		};

		// write it all to a byte buffer
		rmp_serde::encode::write_named(&mut buf, &response)?;
	}

	Ok(buf)
}

#[derive(serde::Deserialize, Clone, Copy)]
struct ImageDataRequestNew {
	id: Uuid,
	max_width: u32,
	max_height: u32,
}

async fn get_image_data_for_images(
	Extension(db): DbExtension,
	Json(req): Json<Vec<ImageDataRequestNew>>,
) -> Result<impl IntoResponse> {
	// TODO: find a way to select a list of ids
	// for now, the list is manually filtered

	let len = req.len();
	let header_stream = async move {
		let mut buf = vec![];
		rmp::encode::write_array_len(&mut buf, len as u32)?;
		Ok(buf)
	}
	.into_stream();

	let counter = Arc::new(RwLock::new(0u64));
	let counter_move = counter.clone();
	let stream = futures::stream::iter(req)
		.map(move |r| {
			let db = db.clone();
			let counter = counter_move.clone();
			async move {
				{
					let count = counter.read().await;
					if *count > RESPONSE_MAX_SIZE {
						return Err(Error::GenericInternalError);
					}
				}

				let mut buf = vec![];
				let image_file =
					match ImageFile::get_by_max_size(&db, r.id, r.max_width, r.max_height).await? {
						Some(s) => s,
						None => {
							rmp::encode::write_nil(&mut buf)?;
							return Ok(buf);
						}
					};

				// load and resize image to the given bounds
				let mut path = PathBuf::new();
				path.push(IMAGES_PATH);
				path.push(&image_file.file_name);

				let file = match tokio::fs::File::open(&path).await {
					Ok(f) => f,
					Err(_) => {
						rmp::encode::write_nil(&mut buf)?;
						return Ok(buf);
					}
				};
				let size = file.metadata().await?.len();
				rmp::encode::write_bin_len(&mut buf, size as u32)?;

				*counter.write().await += size;

				let mut reader = tokio::io::BufReader::new(file);

				reader.read_to_end(&mut buf).await?;

				Ok(buf)
			}
		})
		.buffered(128);

	let count = *counter.read().await;
	if count > RESPONSE_MAX_SIZE {
		return Err(Error::PayloadTooLarge(count));
	}

	let stream_body = StreamBody::new(header_stream.chain(stream));

	Ok(stream_body)
}
