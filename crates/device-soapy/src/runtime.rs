use crate::soapy;

#[derive(Clone, Debug)]
pub struct RuntimeInfo {
    pub core_version: Option<String>,
    pub library: Option<String>,
    pub search_paths: Vec<String>,
    pub modules: Vec<String>,
    pub error: Option<String>,
}

#[must_use]
pub fn runtime_info() -> RuntimeInfo {
    match soapy::installed() {
        Ok((version, path)) => RuntimeInfo {
            core_version: Some(version),
            library: Some(path.display().to_string()),
            search_paths: soapy::module_search_paths(),
            modules: soapy::list_modules(),
            error: None,
        },
        Err(error) => RuntimeInfo {
            core_version: None,
            library: None,
            search_paths: Vec::new(),
            modules: Vec::new(),
            error: Some(error),
        },
    }
}
