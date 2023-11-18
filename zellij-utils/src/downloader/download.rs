use serde::{Deserialize, Serialize};
use surf::Url;

#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct Download {
    pub url: String,
    pub file_name: String,
}

impl Download {
    pub fn from(url: &str) -> Self {
        match Url::parse(url) {
            Ok(u) => u
                .path_segments()
                .map_or_else(Download::default, |segments| {
                    let file_name = segments.last().unwrap_or("").to_string();

                    Download {
                        url: url.to_string(),
                        file_name,
                    }
                }),
            Err(_) => Download::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_download() {
        let download = Download::from("https://github.com/example/plugin.wasm");
        assert_eq!(download.url, "https://github.com/example/plugin.wasm");
        assert_eq!(download.file_name, "plugin.wasm");
    }

    #[test]
    fn test_empty_download() {
        let d1 = Download::from("https://example.com");
        assert_eq!(d1.url, "https://example.com");
        assert_eq!(d1.file_name, "");

        let d2 = Download::from("github.com");
        assert_eq!(d2.url, "");
        assert_eq!(d2.file_name, "");
    }
}
