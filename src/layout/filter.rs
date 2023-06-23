use serde::{Deserialize, Serialize};

use crate::db::Image;

#[derive(Debug, Serialize, Deserialize)]
pub struct Filter {
	has_metadata: Option<Vec<String>>,
	pub limit: Option<usize>,
}

impl Filter {
	pub fn filter(&self, m: &Image) -> bool {
		if let Some(ref has_metadata) = self.has_metadata {
			for hm in has_metadata.iter() {
				match hm.as_str() {
					"date_time" => {
						if m.metadata.date_time.is_none() {
							return false;
						}
					}
					"palette" => {
						if m.metadata.palette.is_none() {
							return false;
						}
					}
					_ => {}
				}
			}
		}

		true
	}
}
