use crate::clipboard::MimeType;
use anyhow::Result;

/// Generated thumbnail payload and metadata.
///
/// 生成的缩略图负载与元数据。
pub struct GeneratedThumbnail {
    /// Thumbnail bytes.
    ///
    /// 缩略图字节数据。
    pub thumbnail_bytes: Vec<u8>,
    /// MIME type for thumbnail bytes.
    ///
    /// 缩略图字节的 MIME 类型。
    pub thumbnail_mime_type: MimeType,
}

/// Generator port for creating thumbnails from image bytes.
///
/// 从图像字节生成缩略图的生成器端口。
#[async_trait::async_trait]
pub trait ThumbnailGeneratorPort: Send + Sync {
    /// Generate thumbnail from image bytes.
    ///
    /// 从图像字节生成缩略图。
    async fn generate_thumbnail(&self, image_bytes: &[u8]) -> Result<GeneratedThumbnail>;
}
