use axum::{Extension, Json};

use crate::db::{DbExtension, Image};
use crate::err::Result;

pub async fn get_image_metadata(Extension(db): DbExtension) -> Result<Json<Vec<Image>>> {
	Ok(Json(Image::get_all(&db).await?))
}
