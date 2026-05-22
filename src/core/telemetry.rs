//! This build records command usage only in the local SQLite database (`tracking`).
//! Outbound telemetry (network pings) is not implemented.

use super::constants::RTK_DATA_DIR;
use std::path::PathBuf;

/// Legacy salt file path; `telemetry forget` removes it when present.
pub fn salt_file_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("rtk")
        .join(".device_salt")
}

/// Legacy last-ping marker path; `telemetry forget` removes it when present.
pub fn telemetry_marker_path() -> PathBuf {
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(RTK_DATA_DIR);
    let _ = std::fs::create_dir_all(&data_dir);
    data_dir.join(".telemetry_last_ping")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_salt_file_path_is_under_rtk() {
        assert!(salt_file_path().to_string_lossy().contains("rtk"));
        assert!(salt_file_path().to_string_lossy().contains(".device_salt"));
    }

    #[test]
    fn test_marker_path_is_under_rtk_data_dir() {
        let path = telemetry_marker_path();
        assert!(path.to_string_lossy().contains("rtk"));
        assert!(path.to_string_lossy().contains(".telemetry_last_ping"));
    }
}
