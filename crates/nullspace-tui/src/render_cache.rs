use image::RgbaImage;
use std::sync::atomic::{AtomicU64, Ordering};

const RENDER_CACHE_VERSION: u32 = 1;
static TEMP_FILE_NONCE: AtomicU64 = AtomicU64::new(0);

pub fn key(latex: &str, px: u32) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    mix_bytes(&mut hash, b"nullspace-render-cache");
    mix_bytes(&mut hash, &RENDER_CACHE_VERSION.to_le_bytes());
    mix_bytes(&mut hash, &px.to_le_bytes());
    mix_bytes(&mut hash, &(latex.len() as u64).to_le_bytes());
    mix_bytes(&mut hash, latex.as_bytes());
    hash
}

fn mix_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn cache_path(latex: &str, px: u32) -> Option<std::path::PathBuf> {
    let dirs = directories::ProjectDirs::from("dev", "nullspace", "Nullspace")?;
    let cache_key = key(latex, px);
    Some(
        dirs.cache_dir()
            .join("renders")
            .join(format!("{:016x}.png", cache_key)),
    )
}

pub fn load(latex: &str, px: u32) -> Option<RgbaImage> {
    let path = cache_path(latex, px)?;
    image::open(&path).ok().map(|img| img.into_rgba8())
}

pub fn store(latex: &str, px: u32, image: &RgbaImage) {
    let Some(path) = cache_path(latex, px) else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let nonce = TEMP_FILE_NONCE.fetch_add(1, Ordering::Relaxed);
    let temp_path = path.with_file_name(format!("{filename}.tmp.{}.{}", std::process::id(), nonce));
    if image
        .save_with_format(&temp_path, image::ImageFormat::Png)
        .is_ok()
    {
        if std::fs::rename(&temp_path, path).is_err() {
            let _ = std::fs::remove_file(temp_path);
        }
    } else {
        let _ = std::fs::remove_file(temp_path);
    }
}
