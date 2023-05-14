use axum::extract::Path;
use axum::{Extension, Json};
use uuid::Uuid;

use crate::db::{Collection, DbExtension, Image, NewCollection};
use crate::err::Result;

pub async fn get_image_metadata(
	Extension(db): DbExtension,
	Path(collection_id): Path<Uuid>,
) -> Result<Json<Vec<Image>>> {
	Ok(Json(
		Image::get_all_for_collection(&db, collection_id).await?,
	))
}

pub async fn get_collections(Extension(db): DbExtension) -> Result<Json<Vec<Collection>>> {
	Ok(Json(Collection::get_all(&db).await?))
}

#[derive(serde::Deserialize)]
pub struct CreateCollectionRequest {
	name: String,
}

pub async fn create_collection(
	Extension(db): DbExtension,
	Json(req): Json<CreateCollectionRequest>,
) -> Result<Json<Collection>> {
	Ok(Json(
		NewCollection { name: req.name }.insert_one(&db).await?,
	))
}
