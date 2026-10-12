//! Google Drive: finding Drive links in mail, naming the folders Mermoz saves into,
//! and a thin REST client for the few Drive v3 calls the daemon makes.
//!
//! Drive is authorised separately from Gmail (its own consent and refresh token), so
//! declining or disconnecting Drive never touches the account's mail access.

use crate::error::{Error, Result};
use crate::oauth::TokenSource;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// Reads every file the user can open: previews of linked documents and the files
/// shared with a thread's participants. A restricted scope.
pub const SCOPE_READ: &str = "https://www.googleapis.com/auth/drive.readonly";
/// Files the user created with Mermoz or picked for it: saving attachments, and
/// sharing a file the user chose. Non-sensitive.
pub const SCOPE_FILE: &str = "https://www.googleapis.com/auth/drive.file";
/// What "Connect Google Drive" asks for.
pub const SCOPES: &[&str] = &[SCOPE_READ, SCOPE_FILE];

/// The folder every saved attachment goes under.
pub const ROOT_FOLDER: &str = "Mermoz";
/// The folder for attachments of a thread that is in no project.
pub const NO_PROJECT_FOLDER: &str = "No project";

pub const FOLDER_MIME: &str = "application/vnd.google-apps.folder";

pub fn default_base() -> String {
    std::env::var("MERMOZ_DRIVE_BASE")
        .unwrap_or_else(|_| "https://www.googleapis.com/drive/v3".into())
}

pub fn default_upload_base() -> String {
    std::env::var("MERMOZ_DRIVE_UPLOAD_BASE")
        .unwrap_or_else(|_| "https://www.googleapis.com/upload/drive/v3".into())
}

/// What a link points at, from its URL alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkKind {
    Document,
    Spreadsheet,
    Presentation,
    Form,
    Drawing,
    Folder,
    File,
}

/// A Drive file referenced by a link in a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveLink {
    pub file_id: String,
    pub kind: LinkKind,
    /// The link as written (decoded from wrappers), for opening in the browser.
    pub url: String,
}

/// Drive ids are long base64url-ish tokens; shorter runs are path words, not ids.
const MIN_ID_LEN: usize = 15;

fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn take_id(s: &str) -> Option<&str> {
    let end = s.find(|c: char| !is_id_char(c)).unwrap_or(s.len());
    let id = &s[..end];
    (id.len() >= MIN_ID_LEN).then_some(id)
}

/// Undoes the encodings a link picks up in mail: HTML entities in hrefs and the
/// percent-encoding of redirect wrappers (`google.com/url?q=…`, safe-links).
fn unwrap_encodings(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("%3A", ":")
        .replace("%3a", ":")
        .replace("%2F", "/")
        .replace("%2f", "/")
        .replace("%3F", "?")
        .replace("%3f", "?")
        .replace("%3D", "=")
        .replace("%3d", "=")
        .replace("%26", "&")
}

/// The id and kind in a URL path that starts right after the host.
fn parse_after_host(host: &str, rest: &str) -> Option<(String, LinkKind)> {
    // Account switchers ("/u/1/", anywhere in the path) and legacy domain paths
    // ("/a/example.com/", first) carry no file information; drop them.
    let segments: Vec<&str> = rest.trim_start_matches('/').split('/').collect();
    let mut kept: Vec<&str> = vec![];
    let mut i = 0;
    while i < segments.len() {
        let next_is_number = segments
            .get(i + 1)
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        if segments[i] == "u" && next_is_number || segments[i] == "a" && i == 0 {
            i += 2;
            continue;
        }
        kept.push(segments[i]);
        i += 1;
    }
    let joined = kept.join("/");
    let path = joined.as_str();
    if host == "docs.google.com" {
        let (kind, after) = path.split_once('/')?;
        let kind = match kind {
            "document" => LinkKind::Document,
            "spreadsheets" => LinkKind::Spreadsheet,
            "presentation" => LinkKind::Presentation,
            "forms" => LinkKind::Form,
            "drawings" => LinkKind::Drawing,
            "file" => LinkKind::File,
            _ => return None,
        };
        let after = after.strip_prefix("d/")?;
        // "/d/e/2PACX…" is a published copy, not a Drive file id.
        if after.starts_with("e/") {
            return None;
        }
        return take_id(after).map(|id| (id.to_string(), kind));
    }
    // drive.google.com
    if let Some(after) = path.strip_prefix("file/d/") {
        return take_id(after).map(|id| (id.to_string(), LinkKind::File));
    }
    if let Some(after) = path
        .strip_prefix("drive/folders/")
        .or_else(|| path.strip_prefix("folderview?id="))
    {
        return take_id(after).map(|id| (id.to_string(), LinkKind::Folder));
    }
    for prefix in ["open?", "uc?", "file?"] {
        if let Some(q) = path.strip_prefix(prefix) {
            let id = q.split('&').find_map(|kv| kv.strip_prefix("id="))?;
            return take_id(id).map(|id| (id.to_string(), LinkKind::File));
        }
    }
    None
}

/// Every Drive file linked in `text` (plain text or HTML), first occurrence first.
pub fn find_links(text: &str) -> Vec<DriveLink> {
    let text = unwrap_encodings(text);
    let mut out: Vec<DriveLink> = vec![];
    for host in ["docs.google.com", "drive.google.com"] {
        let mut from = 0;
        while let Some(i) = text[from..].find(host) {
            let start = from + i;
            from = start + host.len();
            // The host must start the authority: "https://", "//" or a bare word boundary.
            let before = text[..start].chars().last();
            if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
                continue;
            }
            let rest = &text[from..];
            let end = rest
                .find(|c: char| c.is_whitespace() || "\"'<>()[]{}|\\^`".contains(c))
                .unwrap_or(rest.len());
            let mut rest = &rest[..end];
            // An "&" before any "?" belongs to a redirect wrapper around the link.
            if let Some(amp) = rest.find('&')
                && rest.find('?').is_none_or(|q| amp < q)
            {
                rest = &rest[..amp];
            }
            if !rest.starts_with('/') {
                continue;
            }
            let Some((file_id, kind)) = parse_after_host(host, rest) else {
                continue;
            };
            if out.iter().any(|l| l.file_id == file_id) {
                continue;
            }
            let url = format!(
                "https://{host}{}",
                rest.trim_end_matches(['.', ',', ';', ':', '!', '?'])
            );
            out.push(DriveLink { file_id, kind, url });
        }
    }
    // Keep the order the links appear in the text, whatever the host.
    out.sort_by_key(|l| text.find(&l.file_id).unwrap_or(usize::MAX));
    out
}

/// One folder name, as Drive will show it: no control characters, single spaces,
/// at most 100 characters.
fn clean_segment(s: &str) -> String {
    let collapsed: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    collapsed.chars().take(100).collect()
}

/// Folders an attachment is saved into, from the root down: "Mermoz", then the
/// thread's first project split on Gmail's "/" nesting ("Clients/Acme" →
/// Mermoz › Clients › Acme), or "No project".
pub fn folder_path(projects: &[String]) -> Vec<String> {
    let mut path = vec![ROOT_FOLDER.to_string()];
    let segments: Vec<String> = projects
        .iter()
        .map(|p| {
            p.split('/')
                .map(clean_segment)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .find(|segs| !segs.is_empty())
        .unwrap_or_default();
    if segments.is_empty() {
        path.push(NO_PROJECT_FOLDER.into());
    } else {
        path.extend(segments);
    }
    path
}

/// A value inside a Drive query string (`name = '…'`).
pub fn quote(v: &str) -> String {
    format!("'{}'", v.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// The query for files a person owns or can open (shared by them or with them).
pub fn people_query(emails: &[String]) -> Option<String> {
    let clauses: Vec<String> = emails
        .iter()
        .filter(|e| e.contains('@'))
        .flat_map(|e| {
            let q = quote(&e.to_lowercase());
            [
                format!("{q} in owners"),
                format!("{q} in writers"),
                format!("{q} in readers"),
            ]
        })
        .collect();
    if clauses.is_empty() {
        return None;
    }
    Some(format!(
        "({}) and trashed = false and mimeType != {}",
        clauses.join(" or "),
        quote(FOLDER_MIME)
    ))
}

/// The query behind the compose search box: names and contents.
pub fn search_query(text: &str) -> String {
    let t = quote(text.trim());
    format!(
        "(name contains {t} or fullText contains {t}) and trashed = false and mimeType != {}",
        quote(FOLDER_MIME)
    )
}

/// How a Google-native file is turned into text for the AI, if it can be.
pub fn export_mime(mime: &str) -> Option<&'static str> {
    match mime {
        "application/vnd.google-apps.document" => Some("text/markdown"),
        "application/vnd.google-apps.spreadsheet" => Some("text/csv"),
        "application/vnd.google-apps.presentation" => Some("text/plain"),
        _ => None,
    }
}

/// Plain-text files are downloaded as they are.
pub fn is_plain_text(mime: &str) -> bool {
    mime.starts_with("text/") || mime == "application/json"
}

/// A plain-words name for a file type, as the preview card shows it.
pub fn kind_label(mime: &str) -> &'static str {
    match mime {
        "application/vnd.google-apps.document" => "Google Doc",
        "application/vnd.google-apps.spreadsheet" => "Google Sheet",
        "application/vnd.google-apps.presentation" => "Google Slides",
        "application/vnd.google-apps.form" => "Google Form",
        "application/vnd.google-apps.drawing" => "Google Drawing",
        FOLDER_MIME => "Folder",
        "application/pdf" => "PDF",
        m if m.starts_with("image/") => "Image",
        m if m.starts_with("video/") => "Video",
        m if m.starts_with("audio/") => "Audio",
        m if m.contains("spreadsheet") || m.contains("excel") => "Spreadsheet",
        m if m.contains("presentation") || m.contains("powerpoint") => "Presentation",
        m if m.contains("word") || m.contains("opendocument.text") => "Document",
        _ => "File",
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Person {
    #[serde(default, rename = "displayName", alias = "name")]
    pub name: String,
    #[serde(default, rename = "emailAddress", alias = "email")]
    pub email: String,
}

/// File metadata as Drive returns it (the fields Mermoz asks for).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMeta {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mime_type: String,
    /// RFC 3339.
    #[serde(default)]
    pub modified_time: Option<String>,
    #[serde(default)]
    pub web_view_link: Option<String>,
    #[serde(default)]
    pub icon_link: Option<String>,
    #[serde(default)]
    pub thumbnail_link: Option<String>,
    #[serde(default)]
    pub owners: Vec<Person>,
    #[serde(default)]
    pub size: Option<String>,
}

pub const FILE_FIELDS: &str = "id,name,mimeType,modifiedTime,webViewLink,iconLink,thumbnailLink,owners(displayName,emailAddress),size";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileList {
    #[serde(default)]
    files: Vec<FileMeta>,
}

/// RFC 3339 → Unix ms (0 when absent or unreadable).
pub fn rfc3339_ms(s: Option<&str>) -> i64 {
    s.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp_millis())
        .unwrap_or(0)
}

pub struct Drive {
    http: reqwest::Client,
    base: String,
    upload_base: String,
    tokens: Arc<TokenSource>,
}

/// Exports past this size are cut: the AI never sees more than a fraction of it.
pub const MAX_EXPORT_BYTES: usize = 2 * 1024 * 1024;

impl Drive {
    pub fn new(tokens: Arc<TokenSource>) -> Self {
        Self::with_bases(default_base(), default_upload_base(), tokens)
    }

    pub fn with_bases(base: String, upload_base: String, tokens: Arc<TokenSource>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("http client");
        Self {
            http,
            base,
            upload_base,
            tokens,
        }
    }

    /// Sends an authorised request; a stale access token is refreshed once.
    async fn send(
        &self,
        build: impl Fn(&reqwest::Client, String) -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response> {
        for attempt in 0..2 {
            let token = self.tokens.access_token().await?;
            let resp = build(&self.http, token).send().await?;
            let status = resp.status().as_u16();
            if resp.status().is_success() {
                return Ok(resp);
            }
            if status == 401 && attempt == 0 {
                self.tokens.invalidate();
                continue;
            }
            return Err(Error::Upstream {
                status,
                body: resp.text().await.unwrap_or_default(),
            });
        }
        Err(Error::other("drive: retries exhausted"))
    }

    /// The signed-in Google account's address.
    pub async fn about_email(&self) -> Result<String> {
        let url = format!("{}/about", self.base);
        let v: serde_json::Value = self
            .send(|h, t| {
                h.get(&url)
                    .bearer_auth(t)
                    .query(&[("fields", "user(emailAddress)")])
            })
            .await?
            .json()
            .await?;
        v["user"]["emailAddress"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| Error::other("Drive did not say which account this is"))
    }

    pub async fn get(&self, id: &str) -> Result<FileMeta> {
        let url = format!("{}/files/{id}", self.base);
        Ok(self
            .send(|h, t| {
                h.get(&url)
                    .bearer_auth(t)
                    .query(&[("fields", FILE_FIELDS), ("supportsAllDrives", "true")])
            })
            .await?
            .json()
            .await?)
    }

    /// Files matching a Drive query, in `order` ("modifiedTime desc", …).
    pub async fn list(&self, q: Option<&str>, order: &str, limit: usize) -> Result<Vec<FileMeta>> {
        let url = format!("{}/files", self.base);
        let fields = format!("files({FILE_FIELDS})");
        let size = limit.clamp(1, 100).to_string();
        let l: FileList = self
            .send(|h, t| {
                let mut query: Vec<(&str, &str)> = vec![
                    ("fields", &fields),
                    ("orderBy", order),
                    ("pageSize", &size),
                    ("supportsAllDrives", "true"),
                    ("includeItemsFromAllDrives", "true"),
                ];
                if let Some(q) = q {
                    query.push(("q", q));
                }
                h.get(&url).bearer_auth(t).query(&query)
            })
            .await?
            .json()
            .await?;
        Ok(l.files)
    }

    /// The file's text: Google files exported (Docs as Markdown, Sheets as CSV of the
    /// first sheet, Slides as text), plain-text files as they are, others `None`.
    pub async fn text(&self, meta: &FileMeta) -> Result<Option<String>> {
        let id = &meta.id;
        let resp = if let Some(mime) = export_mime(&meta.mime_type) {
            let url = format!("{}/files/{id}/export", self.base);
            self.send(|h, t| h.get(&url).bearer_auth(t).query(&[("mimeType", mime)]))
                .await?
        } else if is_plain_text(&meta.mime_type) {
            let url = format!("{}/files/{id}", self.base);
            self.send(|h, t| {
                h.get(&url)
                    .bearer_auth(t)
                    .query(&[("alt", "media"), ("supportsAllDrives", "true")])
            })
            .await?
        } else {
            return Ok(None);
        };
        let mut bytes = resp.bytes().await?.to_vec();
        bytes.truncate(MAX_EXPORT_BYTES);
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// A small preview image, when Drive has one and serves it to this token.
    pub async fn thumbnail(&self, link: &str) -> Result<(Vec<u8>, String)> {
        let link = link.to_string();
        let resp = self.send(|h, t| h.get(&link).bearer_auth(t)).await?;
        let mime = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/png")
            .to_string();
        Ok((resp.bytes().await?.to_vec(), mime))
    }

    /// The folder named `name` under `parent` that Mermoz can see, if any.
    pub async fn find_folder(&self, name: &str, parent: &str) -> Result<Option<String>> {
        let q = format!(
            "name = {} and mimeType = {} and {} in parents and trashed = false",
            quote(name),
            quote(FOLDER_MIME),
            quote(parent)
        );
        Ok(self
            .list(Some(&q), "createdTime", 1)
            .await?
            .into_iter()
            .next()
            .map(|f| f.id))
    }

    pub async fn create_folder(&self, name: &str, parent: &str) -> Result<String> {
        let url = format!("{}/files", self.base);
        let body = json!({ "name": name, "mimeType": FOLDER_MIME, "parents": [parent] });
        let f: FileMeta = self
            .send(|h, t| {
                h.post(&url)
                    .bearer_auth(t)
                    .query(&[("fields", "id"), ("supportsAllDrives", "true")])
                    .json(&body)
            })
            .await?
            .json()
            .await?;
        Ok(f.id)
    }

    /// Uploads a file into `parent` with a resumable session (works at any size).
    pub async fn upload(
        &self,
        name: &str,
        mime: &str,
        bytes: Vec<u8>,
        parent: &str,
    ) -> Result<FileMeta> {
        let url = format!("{}/files", self.upload_base);
        let meta = json!({ "name": name, "parents": [parent] });
        let len = bytes.len().to_string();
        let start = self
            .send(|h, t| {
                h.post(&url)
                    .bearer_auth(t)
                    .query(&[
                        ("uploadType", "resumable"),
                        ("fields", FILE_FIELDS),
                        ("supportsAllDrives", "true"),
                    ])
                    .header("X-Upload-Content-Type", mime)
                    .header("X-Upload-Content-Length", &len)
                    .json(&meta)
            })
            .await?;
        let session = start
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Error::other("Drive did not open an upload session"))?
            .to_string();
        let resp = self
            .send(|h, t| {
                h.put(&session)
                    .bearer_auth(t)
                    .header(reqwest::header::CONTENT_TYPE, mime)
                    .body(bytes.clone())
            })
            .await?;
        Ok(resp.json().await?)
    }

    /// Gives one person access to a file. `role` is "reader" or "commenter".
    /// Drive sends no email of its own: the message carrying the link is the invitation.
    pub async fn share(&self, id: &str, email: &str, role: &str) -> Result<()> {
        let url = format!("{}/files/{id}/permissions", self.base);
        let body = json!({ "type": "user", "role": role, "emailAddress": email });
        self.send(|h, t| {
            h.post(&url)
                .bearer_auth(t)
                .query(&[
                    ("sendNotificationEmail", "false"),
                    ("supportsAllDrives", "true"),
                ])
                .json(&body)
        })
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    const ID: &str = "1AbCdEfGhIjKlMnOpQrStUvWxYz_0123456789-xy";

    #[rstest]
    #[case::doc_edit(format!("https://docs.google.com/document/d/{ID}/edit?usp=sharing"), LinkKind::Document)]
    #[case::doc_bare(format!("https://docs.google.com/document/d/{ID}"), LinkKind::Document)]
    #[case::doc_account_switcher(format!("https://docs.google.com/document/u/1/d/{ID}/edit"), LinkKind::Document)]
    #[case::doc_legacy_domain(format!("https://docs.google.com/a/acme.example/document/d/{ID}/edit"), LinkKind::Document)]
    #[case::sheet(format!("https://docs.google.com/spreadsheets/d/{ID}/edit#gid=0"), LinkKind::Spreadsheet)]
    #[case::slides(format!("https://docs.google.com/presentation/d/{ID}/edit#slide=id.p"), LinkKind::Presentation)]
    #[case::form(format!("https://docs.google.com/forms/d/{ID}/edit"), LinkKind::Form)]
    #[case::drawing(format!("https://docs.google.com/drawings/d/{ID}/edit"), LinkKind::Drawing)]
    #[case::drive_file(format!("https://drive.google.com/file/d/{ID}/view?usp=drive_link"), LinkKind::File)]
    #[case::drive_file_account(format!("https://drive.google.com/file/u/0/d/{ID}/view"), LinkKind::File)]
    #[case::drive_open(format!("https://drive.google.com/open?id={ID}"), LinkKind::File)]
    #[case::drive_open_html_amp(format!("https://drive.google.com/open?authuser=0&amp;id={ID}"), LinkKind::File)]
    #[case::drive_download(format!("https://drive.google.com/uc?export=download&id={ID}"), LinkKind::File)]
    #[case::folder(format!("https://drive.google.com/drive/folders/{ID}?usp=sharing"), LinkKind::Folder)]
    #[case::folder_account(format!("https://drive.google.com/drive/u/2/folders/{ID}"), LinkKind::Folder)]
    #[case::http(format!("http://docs.google.com/document/d/{ID}/edit"), LinkKind::Document)]
    #[case::no_scheme(format!("see docs.google.com/document/d/{ID}/edit"), LinkKind::Document)]
    #[case::google_redirect(format!("https://www.google.com/url?q=https%3A%2F%2Fdocs.google.com%2Fdocument%2Fd%2F{ID}%2Fedit&sa=D"), LinkKind::Document)]
    #[case::html_href(format!("<a href=\"https://docs.google.com/spreadsheets/d/{ID}/edit\">Budget</a>"), LinkKind::Spreadsheet)]
    #[case::angle_brackets(format!("Budget <https://docs.google.com/spreadsheets/d/{ID}/edit>"), LinkKind::Spreadsheet)]
    #[case::in_parentheses(format!("(https://docs.google.com/document/d/{ID})."), LinkKind::Document)]
    fn finds_the_file(#[case] text: String, #[case] kind: LinkKind) {
        assert_eq!(
            find_links(&text)
                .into_iter()
                .map(|l| (l.file_id, l.kind))
                .collect::<Vec<_>>(),
            vec![(ID.to_string(), kind)]
        );
    }

    #[rstest]
    #[case::published_copy(
        "https://docs.google.com/document/d/e/2PACX-1vQabcdefghijklmnopqrstuvwxyz/pub"
    )]
    #[case::published_form(
        "https://docs.google.com/forms/d/e/1FAIpQLSabcdefghijklmnopqrstuvwxyz/viewform"
    )]
    #[case::drive_home("https://drive.google.com/drive/my-drive")]
    #[case::short_id("https://docs.google.com/document/d/abc/edit")]
    #[case::lookalike_host(
        "https://docs.google.com.evil.example/document/d/1AbCdEfGhIjKlMnOpQrSt/edit"
    )]
    #[case::prefixed_host("https://notdocs.google.com/document/d/1AbCdEfGhIjKlMnOpQrSt/edit")]
    #[case::plain_mention("I put it on docs.google.com somewhere")]
    fn ignores(#[case] text: &str) {
        assert_eq!(find_links(text), vec![]);
    }

    #[rstest]
    fn the_same_file_is_listed_once() {
        let t = format!(
            "https://docs.google.com/document/d/{ID}/edit and again <a href=\"https://docs.google.com/document/d/{ID}/edit?usp=sharing\">"
        );
        assert_eq!(find_links(&t).len(), 1);
    }

    #[rstest]
    fn links_keep_their_order_in_the_text() {
        let a = "1aaaaaaaaaaaaaaaaaaaaaaaa";
        let b = "1bbbbbbbbbbbbbbbbbbbbbbbb";
        let t = format!(
            "https://drive.google.com/file/d/{b}/view then https://docs.google.com/document/d/{a}/edit"
        );
        assert_eq!(
            find_links(&t)
                .into_iter()
                .map(|l| l.file_id)
                .collect::<Vec<_>>(),
            vec![b.to_string(), a.to_string()]
        );
    }

    #[rstest]
    fn trailing_punctuation_is_not_part_of_the_url() {
        let t = format!("Here: https://docs.google.com/document/d/{ID}/edit.");
        assert_eq!(
            find_links(&t)[0].url,
            format!("https://docs.google.com/document/d/{ID}/edit")
        );
    }

    #[rstest]
    #[case::nested_label(vec!["Clients/Acme"], vec!["Mermoz", "Clients", "Acme"])]
    #[case::flat_label(vec!["Receipts"], vec!["Mermoz", "Receipts"])]
    #[case::no_project(vec![], vec!["Mermoz", "No project"])]
    #[case::first_project_wins(vec!["Finance", "Clients/Acme"], vec!["Mermoz", "Finance"])]
    #[case::blank_project_skipped(vec!["  ", "Finance"], vec!["Mermoz", "Finance"])]
    #[case::empty_segments_dropped(vec!["Clients//Acme/"], vec!["Mermoz", "Clients", "Acme"])]
    #[case::spaces_collapsed(vec!["  Board   meetings \t"], vec!["Mermoz", "Board meetings"])]
    #[case::control_chars(vec!["Tax\u{0007}2026"], vec!["Mermoz", "Tax 2026"])]
    #[case::apostrophe_kept(vec!["Léa's projects"], vec!["Mermoz", "Léa's projects"])]
    fn folders_follow_the_project(#[case] projects: Vec<&str>, #[case] want: Vec<&str>) {
        let p: Vec<String> = projects.into_iter().map(String::from).collect();
        assert_eq!(folder_path(&p), want);
    }

    #[rstest]
    fn long_folder_names_are_cut() {
        assert_eq!(folder_path(&["x".repeat(300)])[1].chars().count(), 100);
    }

    #[rstest]
    fn quotes_are_escaped_in_queries() {
        assert_eq!(quote("Léa's \\ notes"), "'Léa\\'s \\\\ notes'");
    }

    #[rstest]
    fn people_query_covers_owners_writers_and_readers() {
        assert_eq!(
            people_query(&["Marc@Acme.example".into()]).unwrap(),
            "('marc@acme.example' in owners or 'marc@acme.example' in writers or 'marc@acme.example' in readers) and trashed = false and mimeType != 'application/vnd.google-apps.folder'"
        );
    }

    #[rstest]
    fn people_query_needs_an_address() {
        assert_eq!(people_query(&["Marc Petit".into()]), None);
    }

    #[rstest]
    #[case::doc("application/vnd.google-apps.document", Some("text/markdown"))]
    #[case::sheet("application/vnd.google-apps.spreadsheet", Some("text/csv"))]
    #[case::slides("application/vnd.google-apps.presentation", Some("text/plain"))]
    #[case::pdf("application/pdf", None)]
    fn export_formats(#[case] mime: &str, #[case] want: Option<&str>) {
        assert_eq!(export_mime(mime), want);
    }

    #[rstest]
    #[case::doc("application/vnd.google-apps.document", "Google Doc")]
    #[case::pdf("application/pdf", "PDF")]
    #[case::xlsx(
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "Spreadsheet"
    )]
    #[case::unknown("application/octet-stream", "File")]
    fn kind_labels(#[case] mime: &str, #[case] want: &str) {
        assert_eq!(kind_label(mime), want);
    }

    #[rstest]
    fn rfc3339_dates_become_ms() {
        assert_eq!(
            rfc3339_ms(Some("2026-10-11T08:12:00.000Z")),
            1_791_706_320_000
        );
    }
}
