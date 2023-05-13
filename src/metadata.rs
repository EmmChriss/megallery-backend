use axum::extract::Query;
use axum::{Extension, Json};
use uuid::Uuid;

use crate::db::{Collection, DbExtension, Image};
use crate::err::Result;

#[derive(serde::Deserialize)]
pub struct ImageMetadataRequestParams {
	collection_id: Option<Uuid>,
}

pub async fn get_image_metadata(
	Extension(db): DbExtension,
	Query(params): Query<ImageMetadataRequestParams>,
) -> Result<Json<Vec<Image>>> {
	let images = match params.collection_id {
		Some(c_id) => Image::get_all_for_collection(&db, c_id).await?,
		None => Image::get_all(&db).await?,
	};

	Ok(Json(images))
}

pub async fn get_collection_metadata(Extension(db): DbExtension) -> Result<Json<Vec<Collection>>> {
	Ok(Json(Collection::get_all(&db).await?))
}
