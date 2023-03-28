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

use std::{net::SocketAddr, path::Path, sync::Arc};

use tower_http::cors::CorsLayer;
use uuid::Uuid;

const IMAGES_PATH: &str = "./images";
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
		.route(
			"/images",
			get(metadata::get_image_metadata).post(upload::upload_image),
		)
		.route("/images/atlas", post(crate::atlas::get_atlas))
		.route("/images/bulk", post(crate::bulk::get_images_bulk))
		.layer(state)
		.layer(CorsLayer::permissive());

	// start built-in server
	let addr = SocketAddr::new(addr.parse().unwrap(), port.parse().unwrap());
	axum::Server::bind(&addr)
		.serve(app.into_make_service())
		.await
		.expect("Server error");
}
