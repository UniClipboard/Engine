use std::io::Cursor;

use anyhow::{Context, Result};
use async_trait::async_trait;
use image::{imageops::FilterType, ColorType, GenericImageView, ImageReader, Limits};
use tokio::sync::Semaphore;
use tokio::task::spawn_blocking;

use uc_core::clipboard::MimeType;
use uc_core::ports::clipboard::{GeneratedThumbnail, ThumbnailGeneratorPort};

static GENERATION_GATE: Semaphore = Semaphore::const_new(1);

pub struct InfraThumbnailGenerator {
    max_edge: u32,
}

impl InfraThumbnailGenerator {
    pub fn new(max_edge: u32) -> Result<Self> {
        if max_edge == 0 {
            anyhow::bail!("max_edge must be greater than 0, got: {}", max_edge);
        }
        Ok(Self { max_edge })
    }
}

#[async_trait]
impl ThumbnailGeneratorPort for InfraThumbnailGenerator {
    async fn generate_thumbnail(&self, image_bytes: &[u8]) -> Result<GeneratedThumbnail> {
        let permit = GENERATION_GATE
            .acquire()
            .await
            .context("thumbnail generator closed")?;
        if image_bytes.len() > 32 * 1024 * 1024 {
            anyhow::bail!("thumbnail source exceeds byte limit");
        }
        let bytes = image_bytes.to_vec();
        let max_edge = self.max_edge;
        // 阻塞任务持有许可，调用方取消也不能释放正在运行的解码容量。
        let result = spawn_blocking(move || {
            let _permit = permit;
            let mut reader = ImageReader::new(Cursor::new(bytes))
                .with_guessed_format()
                .context("identify thumbnail source")?;
            let mut limits = Limits::default();
            limits.max_alloc = Some(64 * 1024 * 1024);
            limits.max_image_width = Some(8192);
            limits.max_image_height = Some(8192);
            reader.limits(limits);
            let decoded = reader
                .decode()
                .context("decode image bytes for thumbnail")?;
            let (width, height) = decoded.dimensions();
            Self { max_edge }.generate_from_decoded(decoded, width, height)
        })
        .await
        .context("thumbnail blocking task failed")?;
        result
    }
}

impl InfraThumbnailGenerator {
    fn generate_from_decoded(
        &self,
        decoded: image::DynamicImage,
        original_width: u32,
        original_height: u32,
    ) -> Result<GeneratedThumbnail> {
        let (target_width, target_height) =
            calculate_target_size(original_width, original_height, self.max_edge);

        let resized = if target_width == original_width && target_height == original_height {
            decoded
        } else {
            image::DynamicImage::ImageRgba8(image::imageops::resize(
                &decoded,
                target_width,
                target_height,
                FilterType::Triangle,
            ))
        };

        let rgba = resized.to_rgba8();
        let (thumbnail_width, thumbnail_height) = rgba.dimensions();
        let mut thumbnail_bytes = Vec::new();
        let encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut thumbnail_bytes);
        encoder
            .encode(
                rgba.as_raw(),
                thumbnail_width,
                thumbnail_height,
                ColorType::Rgba8.into(),
            )
            .context("encode thumbnail to webp")?;

        Ok(GeneratedThumbnail {
            thumbnail_bytes,
            thumbnail_mime_type: MimeType("image/webp".to_string()),
        })
    }
}

fn calculate_target_size(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    if width <= max_edge && height <= max_edge {
        return (width, height);
    }

    if width >= height {
        let scaled_height = ((height as f64) * (max_edge as f64) / (width as f64)).round() as u32;
        (max_edge, scaled_height.max(1))
    } else {
        let scaled_width = ((width as f64) * (max_edge as f64) / (height as f64)).round() as u32;
        (scaled_width.max(1), max_edge)
    }
}
