use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::Engine as _;
use rmcp::model::RawContent;

use crate::client::McpClient;
use crate::connection::ConnectionInfo;
use crate::output;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    client: &McpClient,
    conn: &ConnectionInfo,
    ids: &[String],
    offset: Option<usize>,
    limit: Option<usize>,
    image_text: Option<String>,
    annotations_offset: Option<usize>,
    image: Option<String>,
    json_mode: bool,
) -> Result<()> {
    if let Some(image) = image {
        let [id] = ids else {
            return output::print_error("--image takes exactly one document ID", json_mode);
        };
        return read_image(client, conn, id, &image, json_mode).await;
    }

    // The `read` tool takes one document, so several IDs mean several calls.
    // Sequentially: the desktop is a single local process and the point here is
    // pipeability, not throughput.
    let multi = ids.len() > 1;
    let mut failures: Vec<String> = Vec::new();
    let mut succeeded = 0usize;

    for id in ids.iter() {
        let mut args = serde_json::json!({ "doc_id": id });
        if let Some(offset) = offset {
            args["offset"] = serde_json::json!(offset);
        }
        if let Some(limit) = limit {
            args["limit"] = serde_json::json!(limit);
        }
        if let Some(ref detail) = image_text {
            args["image_text"] = serde_json::json!(detail);
        }
        // Omitted means the desktop's default (0), and the desktop only
        // appends the annotations block off the first page when the field is
        // given explicitly — so an absent flag must stay absent, not become 0.
        if let Some(annotations_offset) = annotations_offset {
            args["annotations_offset"] = serde_json::json!(annotations_offset);
        }
        if json_mode {
            args["output_format"] = serde_json::json!("json");
        }

        match client.call_tool("read", args, conn).await {
            Ok(content) => {
                // JSON mode stays one object per line: a single ID produces
                // exactly what it always did, several produce JSON Lines —
                // streamable, and readable by `jq` without a wrapper array.
                //
                // Markdown needs no separator of its own; each response already
                // ends with the `---` of its metadata footer, and every document
                // starts with a heading.
                output::print_result(&content, json_mode);
                succeeded += 1;
            }
            Err(e) => {
                // One unreadable document among many shouldn't discard the rest
                // — that is the whole reason for passing a batch.
                if !multi {
                    return output::print_tool_error(&e, json_mode);
                }
                failures.push(format!("{}: {}", id, e));
            }
        }
    }

    if !failures.is_empty() {
        let summary = failures.join("\n  ");
        if succeeded == 0 {
            return output::print_error(
                &format!(
                    "Could not read any of the {} documents:\n  {}",
                    ids.len(),
                    summary
                ),
                json_mode,
            );
        }
        // Partial success: the readable documents are already on stdout, so the
        // failures go to stderr where they don't corrupt a downstream parser.
        eprintln!(
            "Warning: {} of {} documents could not be read:\n  {}",
            failures.len(),
            ids.len(),
            summary
        );
    }

    Ok(())
}

/// `read` with `image`: one image from inside the document. The text is printed
/// as usual; the image, which a terminal cannot show, is saved to a file and its
/// path printed (`image_file` in JSON). When no image came back, the text says
/// why and is printed alone.
async fn read_image(
    client: &McpClient,
    conn: &ConnectionInfo,
    id: &str,
    image: &str,
    json_mode: bool,
) -> Result<()> {
    let mut args = serde_json::json!({ "doc_id": id, "image": image });
    if json_mode {
        args["output_format"] = serde_json::json!("json");
    }
    let content = match client.call_tool_content("read", args, conn).await {
        Ok(content) => content,
        Err(e) => return output::print_tool_error(&e, json_mode),
    };
    let text = content
        .iter()
        .filter_map(|block| match &block.raw {
            RawContent::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let image_block = content.iter().find_map(|block| match &block.raw {
        RawContent::Image(image) => Some(image),
        _ => None,
    });
    let saved = match image_block {
        Some(block) => match save_image(&images_dir(), id, image, &block.data, &block.mime_type) {
            Ok(path) => Some(path),
            Err(e) => {
                return output::print_error(&format!("Could not save the image: {e:#}"), json_mode)
            }
        },
        None => None,
    };

    if json_mode {
        let mut data: serde_json::Value =
            serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({ "content": text }));
        if let (Some(obj), Some(path)) = (data.as_object_mut(), &saved) {
            obj.insert(
                "image_file".to_string(),
                serde_json::json!(path.to_string_lossy()),
            );
        }
        output::print_result(&data.to_string(), true);
    } else {
        output::print_result(&text, false);
        if let Some(path) = &saved {
            println!("Image saved to: {}", path.display());
        }
    }
    Ok(())
}

/// Where saved images go: `<user cache dir>/linkly-ai/images`, so they never land
/// in a temp directory other users can write to; the temp directory only when the
/// platform has no cache directory.
fn images_dir() -> PathBuf {
    dirs::cache_dir()
        .map(|dir| dir.join("linkly-ai"))
        .unwrap_or_else(std::env::temp_dir)
        .join("images")
}

/// Write an image block's base64 `data` to `<dir>/<doc id>-<image id>.<ext>`,
/// replacing a file already there, and return the path. Characters other than
/// ASCII letters and digits become `_` in the name. Written to a temp file in
/// the same directory and renamed over the target, so a reader never sees half
/// an image.
fn save_image(
    dir: &Path,
    doc_id: &str,
    image: &str,
    data: &str,
    mime_type: &str,
) -> Result<PathBuf> {
    use std::io::Write;

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .context("the image data is not valid base64")?;
    let ext = match mime_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "img",
    };
    let safe = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect()
    };
    std::fs::create_dir_all(dir)
        .with_context(|| format!("Failed to create directory: {}", dir.display()))?;
    let path = dir.join(format!("{}-{}.{ext}", safe(doc_id), safe(image)));
    let mut tmp = tempfile::Builder::new()
        .prefix(".image-")
        .suffix(".tmp")
        .tempfile_in(dir)
        .with_context(|| format!("Failed to create a temp file in {}", dir.display()))?;
    tmp.write_all(&bytes)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    tmp.persist(&path)
        .map_err(|e| e.error)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_is_saved_under_its_document_and_id() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("images");
        let data = base64::engine::general_purpose::STANDARD.encode(b"jpeg bytes");

        let path = save_image(&dir, "local://42", "p3-1", &data, "image/jpeg").unwrap();

        assert_eq!(path, dir.join("local___42-p3_1.jpg"));
        assert_eq!(std::fs::read(&path).unwrap(), b"jpeg bytes");

        // The same image again replaces the file, and leaves no temp file behind.
        let again = base64::engine::general_purpose::STANDARD.encode(b"newer");
        assert_eq!(
            save_image(&dir, "local://42", "p3-1", &again, "image/jpeg").unwrap(),
            path
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"newer");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }

    #[test]
    fn the_extension_follows_the_mime_type() {
        let tmp = tempfile::tempdir().unwrap();
        let data = base64::engine::general_purpose::STANDARD.encode(b"x");
        let png = save_image(tmp.path(), "local://1", "p1-1", &data, "image/png").unwrap();
        assert_eq!(png.extension().unwrap(), "png");
        let other = save_image(tmp.path(), "local://1", "p1-2", &data, "image/heic").unwrap();
        assert_eq!(other.extension().unwrap(), "img");
    }

    #[test]
    fn data_that_is_not_base64_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(save_image(tmp.path(), "local://1", "p1-1", "not base64!", "image/png").is_err());
    }
}
