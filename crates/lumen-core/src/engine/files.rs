//! Filesystem helpers for captured images.

use std::path::{Path, PathBuf};

use anyhow::Context;

/// First `prefix_0001.ext`, `prefix_0002.ext`, ... that does not exist yet.
pub fn next_free_path(dir: &Path, prefix: &str, ext: &str) -> PathBuf {
    (1u32..)
        .map(|i| dir.join(format!("{prefix}_{i:04}.{ext}")))
        .find(|p| !p.exists())
        .expect("u32 range exhausted")
}

/// First `prefix_001`, `prefix_002`, ... directory name that does not exist yet.
pub fn next_free_dir(parent: &Path, prefix: &str) -> PathBuf {
    (1u32..).map(|i| parent.join(format!("{prefix}_{i:03}"))).find(|p| !p.exists()).expect("u32 range exhausted")
}

pub fn extension_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|e| !e.is_empty() && e.len() <= 5)
        .unwrap_or_else(|| "jpg".into())
}

pub fn save_bytes(dir: &Path, prefix: &str, original_name: &str, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = next_free_path(dir, prefix, &extension_of(original_name));
    std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbering_skips_existing_files() {
        let dir = std::env::temp_dir().join(format!("lumen-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = save_bytes(&dir, "shot", "DSC_1.JPG", b"a").unwrap();
        let b = save_bytes(&dir, "shot", "DSC_2.JPG", b"b").unwrap();
        assert_ne!(a, b);
        assert!(a.ends_with("shot_0001.jpg") && b.ends_with("shot_0002.jpg"));
        assert_eq!(extension_of("x"), "jpg");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
