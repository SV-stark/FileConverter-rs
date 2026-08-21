use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub release_notes: String,
    pub download_url: String,
    pub is_newer: bool,
}

pub fn check_for_updates(current_version: &str) -> Option<UpdateInfo> {
    let url = "https://api.github.com/repos/SV-stark/FileConverter-rs/releases/latest";
    let config = ureq::config::Config::builder()
        .timeout_global(Some(std::time::Duration::from_secs(3)))
        .build();
    let agent: ureq::Agent = config.into();
    let resp = agent
        .get(url)
        .header("User-Agent", "FileConverter-rs")
        .call()
        .ok()?;

    let json: serde_json::Value = serde_json::from_reader(resp.into_body().into_reader()).ok()?;
    let tag = json.get("tag_name")?.as_str()?;
    let body = json
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let html_url = json
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://github.com/SV-stark/FileConverter-rs/releases")
        .to_string();

    let is_newer = is_version_newer(current_version, tag);

    Some(UpdateInfo {
        current_version: current_version.to_string(),
        latest_version: tag.to_string(),
        release_notes: body,
        download_url: html_url,
        is_newer,
    })
}

pub fn is_version_newer(current: &str, latest: &str) -> bool {
    let parse = |v: &str| -> Vec<u32> {
        v.trim_start_matches('v')
            .split('.')
            .filter_map(|s| s.parse::<u32>().ok())
            .collect()
    };
    let c = parse(current);
    let l = parse(latest);
    l > c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_comparison_logic() {
        assert!(is_version_newer("0.9.1", "0.9.2"));
        assert!(is_version_newer("v0.9.1", "v0.9.2"));
        assert!(is_version_newer("0.8.0", "0.9.0"));
        assert!(!is_version_newer("0.9.2", "0.9.2"));
        assert!(!is_version_newer("0.9.3", "0.9.2"));
    }
}
