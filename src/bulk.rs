use std::sync::Arc;

use axum::body::StreamBody;
use axum::{response::IntoResponse, Extension, Json};
use futures::{FutureExt, StreamExt};
use tokio::io::AsyncReadExt;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::db::{DbExtension, ImageFile};
use crate::err::{Error, Result};
use crate::RESPONSE_MAX_SIZE;

#[derive(serde::Deserialize, Clone, Copy)]
pub struct BulkImageRequestEntry(Uuid, u32, u32);

pub async fn get_images_bulk(
	Extension(db): DbExtension,
	Json(req): Json<Vec<BulkImageRequestEntry>>,
) -> Result<impl IntoResponse> {
	// TODO: find a way to select a list of ids
	// for now, the list is manually filtered

	let len = req.len();
	let header_stream = async move {
		let mut buf = vec![];
		rmp::encode::write_array_len(&mut buf, len as u32)?;
		Ok::<_, Error>(buf)
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
				let image_file = match ImageFile::get_approximate_size(&db, r.0, r.1, r.2).await? {
					Some(s) => s,
					None => {
						log::warn!("could not find image file {} <= {}x{}", r.0, r.1, r.2);
						rmp::encode::write_nil(&mut buf)?;
						return Ok(buf);
					}
				};

				// load and resize image to the given bounds
				let path = image_file.get_path();

				let file = match tokio::fs::File::open(&path).await {
					Ok(f) => f,
					Err(e) => {
						log::warn!("could not open file {:?}: {}", &path, e);
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
		.buffered(32);

	let count = *counter.read().await;
	if count > RESPONSE_MAX_SIZE {
		return Err(Error::PayloadTooLarge(count));
	}

	let stream_body = StreamBody::new(header_stream.chain(stream));

	Ok(stream_body)
}
