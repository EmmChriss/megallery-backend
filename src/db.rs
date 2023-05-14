use std::{path::PathBuf, sync::Arc};

use axum::Extension;
use uuid::Uuid;

use crate::{
	err::{Error, Result},
	IMAGES_PATH,
};

pub type Db = sqlx::postgres::PgPool;
pub type DbExtension = Extension<Arc<Db>>;

pub async fn init(db_url: &str) -> Db {
	let pool: Db = sqlx::postgres::PgPoolOptions::new()
		.min_connections(4)
		.max_connections(16)
		.test_before_acquire(true)
		.connect_lazy(&db_url)
		.unwrap();

	sqlx::migrate!().run(&pool).await.unwrap();

	pool
}

#[derive(serde::Serialize)]
pub struct NewCollection {
	pub name: String,
}

impl NewCollection {
	pub async fn insert_one(self, db: &Db) -> sqlx::Result<Collection> {
		let id = Uuid::new_v4();

		sqlx::query("insert into collections values ($1, $2)")
			.bind(id)
			.bind(&self.name)
			.execute(db)
			.await?;

		Ok(Collection {
			id,
			name: self.name,
			finalized: false,
		})
	}
}

#[derive(sqlx::FromRow, serde::Serialize)]
pub struct Collection {
	pub id: sqlx::types::Uuid,
	pub name: String,
	pub finalized: bool,
}

impl Collection {
	pub async fn get_by_id(db: &Db, id: Uuid) -> sqlx::Result<Option<Collection>> {
		sqlx::query_as("select * from collections where id = $1")
			.bind(id)
			.fetch_optional(db)
			.await
	}

	pub async fn get_all(db: &Db) -> sqlx::Result<Vec<Collection>> {
		sqlx::query_as("select * from collections")
			.fetch_all(db)
			.await
	}

	pub async fn save(&self, db: &Db) -> sqlx::Result<()> {
		sqlx::query("update collections set name = $1, finalized = $2 where id = $1")
			.bind(&self.name)
			.bind(self.finalized)
			.bind(self.id)
			.execute(db)
			.await?;

		Ok(())
	}
}

#[derive(sqlx::FromRow, serde::Serialize, Default, Clone)]
pub struct Image {
	pub id: sqlx::types::Uuid,
	pub collection_id: sqlx::types::Uuid,
	pub name: String,
	#[sqlx(try_from = "i32")]
	pub width: u32,
	#[sqlx(try_from = "i32")]
	pub height: u32,
}

impl Image {
	pub async fn get_all_for_collection(db: &Db, collection_id: Uuid) -> sqlx::Result<Vec<Image>> {
		sqlx::query_as("select * from images where collection_id = $1")
			.bind(collection_id)
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

pub struct NewImage {
	pub name: String,
	pub width: u32,
	pub height: u32,
	pub collection_id: sqlx::types::Uuid,
}

impl NewImage {
	pub async fn insert_one(self, db: &Db) -> Result<Image, Error> {
		let id = Uuid::new_v4();

		sqlx::query("insert into images values ($1, $2, $3, $4, $5)")
			.bind(id)
			.bind(&self.name)
			.bind(self.width as i32)
			.bind(self.height as i32)
			.bind(self.collection_id)
			.execute(db)
			.await?;

		Ok(Image {
			id,
			name: self.name,
			width: self.width,
			height: self.height,
			collection_id: self.collection_id,
		})
	}
}

#[derive(sqlx::FromRow)]
pub struct ImageFile {
	pub image_id: Uuid,
	#[sqlx(try_from = "i32")]
	pub width: u32,
	#[sqlx(try_from = "i32")]
	pub height: u32,
	pub extension: String,
}

impl ImageFile {
	pub async fn insert_one(self, db: &Db) -> Result<(), sqlx::Error> {
		sqlx::query("insert into image_files values ($1, $2, $3, $4)")
			.bind(self.image_id)
			.bind(self.width as i32)
			.bind(self.height as i32)
			.bind(self.extension)
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

	pub fn get_path(&self) -> PathBuf {
		let mut path = PathBuf::new();
		path.push(IMAGES_PATH);
		path.push(crate::uuid_to_string(&self.image_id));
		path.push(format!("{}x{}", self.width, self.height));
		path.set_extension(&self.extension);

		return path;
	}
}