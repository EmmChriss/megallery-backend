use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error("failed with message: '{1}', code: {0}")]
	Custom(StatusCode, String),

	#[error("database error: {0}")]
	DbError(#[from] sqlx::Error),

	#[error("image error: {0}")]
	ImageError(#[from] image::ImageError),

	#[error("image resize error: {0}")]
	ImageResizeBufferError(#[from] fast_image_resize::ImageBufferError),

	#[error("io error: {0}")]
	IoError(#[from] std::io::Error),

	#[error("multipart error: {0}")]
	MultipartError(#[from] axum::extract::multipart::MultipartError),

	#[error("multipart extractor error: missing field name")]
	MultipartMissingName,

	#[error("multipart extractor error: missing field: {0}")]
	MultipartMissingField(String),

	#[error("MessagePack write error: {0}")]
	MessagePackError(#[from] rmp::encode::ValueWriteError),

	#[error("MessagePack serializer error: {0}")]
	MessagePackSerializerError(#[from] rmp_serde::encode::Error),

	#[error("payload too large {0}")]
	PayloadTooLarge(u64),

	#[error("task join error")]
	JoinError(#[from] tokio::task::JoinError),

	#[error("generic error")]
	GenericInternalError,
}

impl IntoResponse for Error {
	fn into_response(self) -> Response {
		use Error::*;

		match self {
			Custom(code, msg) => (code, msg).into_response(),
			_ => (StatusCode::INTERNAL_SERVER_ERROR, format!("{}", self)).into_response(),
		}
	}
}
