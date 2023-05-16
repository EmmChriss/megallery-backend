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

		sqlx::query("INSERT INTO collections VALUES ($1, $2)")
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
		sqlx::query_as("SELECT * FROM collections WHERE id = $1")
			.bind(id)
			.fetch_optional(db)
			.await
	}

	pub async fn get_all(db: &Db) -> sqlx::Result<Vec<Collection>> {
		sqlx::query_as("SELECT * FROM collections")
			.fetch_all(db)
			.await
	}

	pub async fn save(&self, db: &Db) -> sqlx::Result<()> {
		sqlx::query("UPDATE collections SET name = $1, finalized = $2 WHERE id = $1")
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
	#[sqlx(try_from = "i32")]
	pub width: u32,
	#[sqlx(try_from = "i32")]
	pub height: u32,
}

impl Image {
	pub async fn get_all_for_collection(db: &Db, collection_id: Uuid) -> sqlx::Result<Vec<Image>> {
		sqlx::query_as("SELECT * FROM images WHERE collection_id = $1")
			.bind(collection_id)
			.fetch_all(db)
			.await
	}

	pub async fn get_by_id(db: &Db, id: sqlx::types::Uuid) -> sqlx::Result<Option<Self>> {
		sqlx::query_as("SELECT * FROM images WHERE id = $1")
			.bind(id)
			.fetch_optional(db)
			.await
	}
}

pub struct NewImage {
	pub width: u32,
	pub height: u32,
	pub collection_id: sqlx::types::Uuid,
}

impl NewImage {
	pub async fn insert_one(self, db: &Db) -> Result<Image, Error> {
		let id = Uuid::new_v4();

		sqlx::query(
			"
			INSERT INTO images (id, width, height, collection_id)
			VALUES ($1, $2, $3, $4)
			",
		)
		.bind(id)
		.bind(self.width as i32)
		.bind(self.height as i32)
		.bind(self.collection_id)
		.execute(db)
		.await?;

		Ok(Image {
			id,
			width: self.width,
			height: self.height,
			collection_id: self.collection_id,
		})
	}
}

#[derive(sqlx::Type)]
#[repr(i32)]
pub enum ImageFileKind {
	Original = 1,
	Thumbnail = 2,
	Partial = 3,
}

#[derive(sqlx::FromRow)]
pub struct ImageFile {
	pub image_id: Uuid,
	#[sqlx(try_from = "i32")]
	pub width: u32,
	#[sqlx(try_from = "i32")]
	pub height: u32,
	pub extension: String,
	pub kind: ImageFileKind,
}

impl ImageFile {
	pub async fn insert_one(self, db: &Db) -> Result<(), sqlx::Error> {
		sqlx::query(
			"
			INSERT INTO image_files (image_id, width, height, extension, kind)
			VALUES ($1, $2, $3, $4, $5)
			",
		)
		.bind(self.image_id)
		.bind(self.width as i32)
		.bind(self.height as i32)
		.bind(self.extension)
		.bind(self.kind)
		.execute(db)
		.await
		.map(|_| ())
	}

	pub async fn get_by_id(
		db: &Db,
		id: Uuid,
		width: u32,
		height: u32,
		kind: ImageFileKind,
	) -> Result<Option<Self>, sqlx::Error> {
		sqlx::query_as(
			"
			SELECT * FROM image_files
			WHERE
				image_id = $1 AND
				width = $2 AND
				height = $3 AND
				kind = $4
			LIMIT 1
			",
		)
		.bind(id)
		.bind(width as i32)
		.bind(height as i32)
		.bind(kind)
		.fetch_optional(db)
		.await
	}

	pub async fn get_smallest(db: &Db, id: Uuid) -> sqlx::Result<Option<Self>> {
		sqlx::query_as(
			"
			SELECT * FROM image_files
			WHERE image_id = $1
			ORDER BY width ASC, height ASC
			LIMIT 1",
		)
		.bind(id)
		.fetch_optional(db)
		.await
	}

	pub fn get_path(&self) -> PathBuf {
		let mut path = PathBuf::new();
		path.push(IMAGES_PATH);
		path.push(crate::uuid_to_string(&self.image_id));

		match self.kind {
			ImageFileKind::Thumbnail => path.push(format!("{}x{}", self.width, self.height)),
			ImageFileKind::Original => path.push("original"),
			ImageFileKind::Partial => unimplemented!(),
		}
		path.set_extension(&self.extension);

		return path;
	}
}
