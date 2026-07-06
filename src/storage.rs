use std::fs;
use std::path::Path;

use crate::domain::AdjutantConfig;
use crate::error::AdjutantConfigError;

pub fn load_from_file(path: &Path) -> Result<AdjutantConfig, AdjutantConfigError> {
    let contents = fs::read_to_string(path)?;
    let config = serde_json::from_str(&contents)?;
    Ok(config)
}

pub fn save_to_file(config: &AdjutantConfig, path: &Path) -> Result<(), AdjutantConfigError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let contents = serde_json::to_string_pretty(config)?;
    fs::write(path, contents)?;
    Ok(())
}
