use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Profile {
    pub name: String,
    pub manifest_path: PathBuf,
}

pub fn list_profiles(config_dir: &Path) -> Result<Vec<Profile>, color_eyre::Report> {
    let mut profiles = Vec::new();

    let provision_dir = config_dir.join("provision.d");
    if provision_dir.is_dir() {
        for entry in fs::read_dir(&provision_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "sh") {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();
                profiles.push(Profile {
                    manifest_path: resolve_manifest(config_dir, &name),
                    name,
                });
            }
        }
    }

    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(profiles)
}

pub fn resolve_manifest(config_dir: &Path, profile_name: &str) -> PathBuf {
    let manifests_dir = config_dir.join("manifests");
    let profile_specific = manifests_dir.join(format!("{}.yaml", profile_name));
    if profile_specific.is_file() {
        return profile_specific;
    }
    manifests_dir.join("base-sandbox.yaml")
}
