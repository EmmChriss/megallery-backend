mod atlas;
mod bulk;
mod db;
mod err;
mod metadata;
mod upload;

use db::{DbExtension, Image, ImageFile};
use err::Result;

use axum::{
	self,
	routing::{get, post},
	Extension,
};
use dotenv;
use env_logger;

use std::{
	net::SocketAddr,
	path::{Path, PathBuf},
	sync::Arc,
};

use tower_http::cors::CorsLayer;
use uuid::Uuid;

const IMAGES_PATH: &str = "./images";
const RESPONSE_MAX_SIZE: u64 = 512 * 1024 * 1024;
const STATIC_ATLASES_DIR: &str = "./images/atlases";

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

fn get_static_atlas_path(collection_id: Uuid) -> PathBuf {
	let mut path = PathBuf::new();
	path.push(STATIC_ATLASES_DIR);
	path.push(uuid_to_string(&collection_id));
	path.set_extension("msgp");
	path
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
	let pool = db::init(&db_url).await;

	// make sure "images" directory exists
	let images = Path::new(IMAGES_PATH);
	if !images.exists() {
		std::fs::create_dir(images).expect("could not create ./images directory");
	}
	if !images.is_dir() {
		panic!("./images is not a directory");
	}

	// make sure "atlases" directory exists
	let images = Path::new(STATIC_ATLASES_DIR);
	if !images.exists() {
		std::fs::create_dir(images).expect("could not create ./images directory");
	}
	if !images.is_dir() {
		panic!("./images is not a directory");
	}

	// define app routes
	let db_extension: DbExtension = Extension(Arc::new(pool));

	let app = axum::Router::new()
		.route(
			"/collections",
			get(crate::metadata::get_collections).post(crate::metadata::create_collection),
		)
		.route("/:id", get(crate::metadata::get_image_metadata))
		.route("/:id/upload", post(crate::upload::upload_image))
		.route("/:id/finalize", post(crate::upload::finalize_collection))
		.route("/:id/bulk", post(crate::bulk::get_images_bulk))
		.route("/:id/atlas", get(crate::atlas::get_static_atlas))
		.layer(db_extension)
		.layer(CorsLayer::permissive());

	// start built-in server
	let addr = SocketAddr::new(addr.parse().unwrap(), port.parse().unwrap());
	axum::Server::bind(&addr)
		.serve(app.into_make_service())
		.await
		.expect("Server error");
}
