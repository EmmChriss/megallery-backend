use std::collections::HashMap;

use axum::extract::Path;
use axum::{Extension, Json};
use uuid::Uuid;

use crate::db::{Collection, DbExtension, Image, ImageMetadata, NewCollection};
use crate::err::Result;

#[derive(serde::Serialize)]
pub struct ImageMetadataResponse(Uuid, u32, u32);

pub async fn get_images(
	Extension(db): DbExtension,
	Path(collection_id): Path<Uuid>,
) -> Result<Json<Vec<ImageMetadataResponse>>> {
	Ok(Json(
		Image::get_all_for_collection(&db, collection_id)
			.await?
			.into_iter()
			.map(|meta| ImageMetadataResponse(meta.id, meta.width, meta.height))
			.collect(),
	))
}

pub async fn get_image_metadata(
	Extension(db): DbExtension,
	Path(collection_id): Path<Uuid>,
) -> Result<Json<HashMap<Uuid, ImageMetadata>>> {
	let images = Image::get_all_for_collection(&db, collection_id).await?;
	let mut res = HashMap::new();
	for image in images {
		res.insert(image.id, image.metadata.0);
	}

	Ok(Json(res))
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
