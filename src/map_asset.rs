//! Bounded download of an already observed public static map reference.
//!
//! This checks transport, MIME and file headers, not decoded image content or
//! seating geometry. It never interprets colors/pixels as stock or adjacency.
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use reqwest::{Client, StatusCode, header, redirect::Policy};
use sha2::{Digest, Sha256};

pub const MAX_MAP_ASSET_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub struct MapAsset {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub sha256: String,
}

/// Download one allowlisted static reference. No redirects, retries, cookies,
/// API keys, session import, or filesystem writes are performed.
pub async fn download_map(url: &str) -> Result<MapAsset> {
    let url = crate::seat_map::validated_static_map_url(url)?;
    let client = Client::builder()
        .user_agent("TicketWatch/0.1 (static venue map reference)")
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(10))
        .redirect(Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .context("Initialize static map HTTPS client")?;
    fetch_asset(&client, &url, MAX_MAP_ASSET_BYTES).await
}

// The public entry point validates the remote URL before reaching this helper.
// Local fixture tests pass their own client/URL; this helper is not exported.
async fn fetch_asset(client: &Client, url: &str, limit: usize) -> Result<MapAsset> {
    let mut response = client
        .get(url)
        .header(
            header::ACCEPT,
            "image/png, image/jpeg, image/gif, image/webp",
        )
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Static map request failed"))?;
    ensure!(
        response.status() == StatusCode::OK,
        "Static map returned HTTP {}; no image accepted",
        response.status().as_u16()
    );
    let content_types = response.headers().get_all(header::CONTENT_TYPE);
    ensure!(
        content_types.iter().count() == 1,
        "Ambiguous static map content type"
    );
    let content_type = content_types
        .iter()
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase())
        .context("Missing static map content type")?;
    ensure!(
        matches!(
            content_type.as_str(),
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        ),
        "Unsupported static map content type"
    );
    ensure!(
        response
            .content_length()
            .is_none_or(|size| size <= limit as u64),
        "Static map exceeds image size limit"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("Static map body could not be read completely"))?
    {
        ensure!(
            chunk.len() <= limit.saturating_sub(bytes.len()),
            "Static map exceeds image size limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    validate_signature(&content_type, &bytes)?;
    Ok(MapAsset {
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        bytes,
        content_type,
    })
}

fn validate_signature(content_type: &str, bytes: &[u8]) -> Result<()> {
    match content_type {
        "image/png" => {
            ensure!(
                bytes.len() >= 45
                    && bytes.starts_with(b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR")
                    && bytes.ends_with(b"\x00\x00\x00\x00IEND\xae\x42\x60\x82"),
                "Static map does not match its PNG content type"
            );
            let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
            let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
            ensure!(
                width > 0 && height > 0 && u64::from(width) * u64::from(height) <= 100_000_000,
                "Static map PNG dimensions are invalid or exceed the display limit"
            );
        }
        "image/jpeg" => {
            // Header/trailer recognition only. JPEG decoding and geometric
            // interpretation are intentionally outside this transport module.
            ensure!(
                bytes.len() >= 12
                    && bytes.starts_with(b"\xff\xd8\xff")
                    && bytes.ends_with(b"\xff\xd9"),
                "Static map does not match its JPEG content type"
            );
        }
        "image/gif" => {
            ensure!(
                bytes.len() >= 14
                    && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"))
                    && bytes.ends_with(b";"),
                "Static map does not match its GIF content type"
            );
            let width = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
            let height = u16::from_le_bytes(bytes[8..10].try_into().unwrap());
            ensure!(
                width > 0 && height > 0 && u64::from(width) * u64::from(height) <= 100_000_000,
                "Static map GIF dimensions are invalid or exceed the display limit"
            );
        }
        "image/webp" => {
            ensure!(
                bytes.len() >= 20
                    && bytes.starts_with(b"RIFF")
                    && &bytes[8..12] == b"WEBP"
                    && matches!(&bytes[12..16], b"VP8 " | b"VP8L" | b"VP8X")
                    && u64::from(u32::from_le_bytes(bytes[4..8].try_into().unwrap())) + 8
                        == bytes.len() as u64,
                "Static map does not match its WebP content type"
            );
        }
        _ => bail!("Unsupported static map content type"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::oneshot,
    };

    // Synthetic 1x1 PNG header/trailer: transport tests do not claim this is a
    // decoded image. Its payload is only used to exercise byte preservation.
    fn png() -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        bytes.extend_from_slice(b"\x00\x00\x00\x00IEND\xae\x42\x60\x82");
        bytes
    }

    fn client() -> Client {
        Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .unwrap()
    }

    async fn mock(status: &str, headers: &str, body: &[u8]) -> (String, oneshot::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut response =
            format!("HTTP/1.1 {status}\r\nConnection: close\r\n{headers}\r\n").into_bytes();
        response.extend_from_slice(body);
        let (sender, receiver) = oneshot::channel();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 4096];
            while !request.windows(4).any(|value| value == b"\r\n\r\n") {
                let read = stream.read(&mut chunk).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
            }
            let _ = sender.send(String::from_utf8(request).unwrap());
            let _ = stream.write_all(&response).await;
        });
        (format!("http://{address}/map.png"), receiver)
    }

    #[tokio::test]
    async fn public_downloader_rejects_untrusted_and_session_urls_before_network() {
        for url in [
            "http://127.0.0.1/map.png",
            "https://example.com/map.png",
            "https://media.ticketmaster.eu/map.png?token=private",
            "https://user:private@media.ticketmaster.eu/map.png",
        ] {
            assert!(download_map(url).await.is_err());
        }
    }

    #[tokio::test]
    async fn preserves_static_png_bytes_and_digest_without_authentication() {
        let body = png();
        let (url, request) = mock("200 OK", "Content-Type: image/png\r\n", &body).await;
        let result = fetch_asset(&client(), &url, 128).await.unwrap();
        assert_eq!(result.content_type, "image/png");
        assert_eq!(result.bytes, body);
        assert_eq!(result.sha256, format!("{:x}", Sha256::digest(&body)));
        let request = request.await.unwrap().to_ascii_lowercase();
        assert!(request.contains("accept: image/png, image/jpeg, image/gif, image/webp"));
        assert!(
            !request.contains("authorization:")
                && !request.contains("cookie:")
                && !request.contains("apikey")
        );
    }

    #[tokio::test]
    async fn jpeg_header_recognition_preserves_bytes_without_claiming_decoding() {
        let body = b"\xff\xd8\xff\xe0\x00\x08JFIF\x00\x00\xff\xd9";
        let (url, _) = mock("200 OK", "Content-Type: IMAGE/JPEG\r\n", body).await;
        let result = fetch_asset(&client(), &url, 128).await.unwrap();
        assert_eq!(result.content_type, "image/jpeg");
        assert_eq!(result.bytes, body);
    }

    #[tokio::test]
    async fn gif_and_webp_reference_headers_are_recognized_without_decoding() {
        let gif = b"GIF89a\x01\x00\x01\x00\x00\x00\x00;".to_vec();
        let webp = b"RIFF\x0c\x00\x00\x00WEBPVP8X\x00\x00\x00\x00".to_vec();
        for (mime, body) in [("image/gif", gif), ("image/webp", webp)] {
            let (url, _) = mock("200 OK", &format!("Content-Type: {mime}\r\n"), &body).await;
            let result = fetch_asset(&client(), &url, 128).await.unwrap();
            assert_eq!(result.content_type, mime);
            assert_eq!(result.bytes, body);
        }
        for (mime, body) in [
            ("image/gif", b"GIF89a".as_slice()),
            (
                "image/webp",
                b"RIFF\xff\xff\xff\xffWEBPVP8X\0\0\0\0".as_slice(),
            ),
        ] {
            let (url, _) = mock("200 OK", &format!("Content-Type: {mime}\r\n"), body).await;
            assert!(fetch_asset(&client(), &url, 128).await.is_err());
        }
    }

    #[tokio::test]
    async fn redirects_are_not_followed_and_error_bodies_are_not_images() {
        let body = png();
        let (destination, mut redirected_request) =
            mock("200 OK", "Content-Type: image/png\r\n", &body).await;
        let (url, _) = mock("302 Found", &format!("Location: {destination}\r\n"), b"").await;
        let error = fetch_asset(&client(), &url, 128)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("HTTP 302"), "{error}");
        assert!(matches!(
            redirected_request.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        for status in [
            "403 Forbidden",
            "429 Too Many Requests",
            "503 Service Unavailable",
        ] {
            let (url, _) = mock(
                status,
                "Content-Type: image/png\r\nContent-Length: 90000000\r\n",
                b"private-body",
            )
            .await;
            let error = fetch_asset(&client(), &url, 128)
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains(&status[..3]));
            assert!(!error.contains("private-body"));
        }
    }

    #[tokio::test]
    async fn invalid_or_ambiguous_mime_and_mismatched_signatures_are_rejected() {
        for headers in [
            "Content-Type: text/html\r\n",
            "Content-Type: image/png\r\nContent-Type: image/jpeg\r\n",
            "Content-Type: image/jpeg\r\n",
            "",
        ] {
            let (url, _) = mock("200 OK", headers, &png()).await;
            assert!(fetch_asset(&client(), &url, 128).await.is_err());
        }
        let (url, _) = mock(
            "200 OK",
            "Content-Type: image/png\r\n",
            b"<html>private challenge</html>",
        )
        .await;
        let error = fetch_asset(&client(), &url, 128)
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.contains("private challenge"));
    }

    #[tokio::test]
    async fn declared_and_streamed_body_limits_are_enforced() {
        let (url, _) = mock(
            "200 OK",
            "Content-Type: image/png\r\nContent-Length: 90000000\r\n",
            b"",
        )
        .await;
        assert!(
            fetch_asset(&client(), &url, 128)
                .await
                .unwrap_err()
                .to_string()
                .contains("size limit")
        );
        let mut body = png();
        body.extend_from_slice(&[0; 128]);
        let (url, _) = mock("200 OK", "Content-Type: image/png\r\n", &body).await;
        assert!(
            fetch_asset(&client(), &url, 128)
                .await
                .unwrap_err()
                .to_string()
                .contains("size limit")
        );
    }

    #[tokio::test]
    async fn truncated_transport_and_invalid_png_dimensions_are_rejected() {
        let (url, _) = mock(
            "200 OK",
            "Content-Type: image/png\r\nContent-Length: 100\r\n",
            &png(),
        )
        .await;
        assert!(
            fetch_asset(&client(), &url, 128)
                .await
                .unwrap_err()
                .to_string()
                .contains("completely")
        );
        for width in [0, u32::MAX] {
            let mut body = png();
            body[16..20].copy_from_slice(&width.to_be_bytes());
            let (url, _) = mock("200 OK", "Content-Type: image/png\r\n", &body).await;
            assert!(
                fetch_asset(&client(), &url, 128)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("dimensions")
            );
        }
    }
}
