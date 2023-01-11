use axum::{self, routing::get, Extension, Json};
// use axum_macros::debug_handler;
use dotenv;
use env_logger;
use log;
use sqlx::postgres::PgPool;
use std::{net::SocketAddr, sync::Arc};

type Db = sqlx::postgres::PgPool;
type DbExtension = Extension<Arc<Db>>;

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

	// define app routes
	let state: DbExtension = Extension(Arc::new(pool));
	let app = axum::Router::new()
		.route("/images", get(get_image_metadata))
		.layer(state);

	// start built-in server
	let addr = SocketAddr::new(addr.parse().unwrap(), port.parse().unwrap());
	axum::Server::bind(&addr)
		.serve(app.into_make_service())
		.await
		.expect("Server error");
}

#[derive(sqlx::FromRow, serde::Serialize, Default)]
struct ImageMetadata {
	id: sqlx::types::Uuid,
	name: String,
}

impl ImageMetadata {
	pub async fn get_all(db: &Db) -> Vec<Self> {
		sqlx::query_as("select id, name from images")
			.fetch_all(db)
			.await
			.unwrap()
	}

	pub async fn get_by_id(db: &Db, id: sqlx::types::Uuid) -> Option<Self> {
		// sqlx::query_as!(ImageMetadata, "select * from images where id = ?", id)
		sqlx::query_as("select * from images where id = ?")
			.bind(id)
			.fetch_optional(db)
			.await
			.unwrap()
	}
}

async fn get_image_metadata(Extension(db): DbExtension) -> Json<Vec<ImageMetadata>> {
	Json(ImageMetadata::get_all(&db).await)
}
