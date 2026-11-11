//! Bounded, namespace-aware DAV metadata. No DTD/entity loading or silent empty-on-error.
use super::DavError;
use quick_xml::{NsReader, XmlVersion, events::Event, name::ResolveResult};
use reqwest::Url;

pub(super) const MAX_XML: usize = 16 * 1024 * 1024;
#[derive(Debug, serde::Serialize)]
pub struct StorageEntry {
    pub name: String,
    pub size: i64,
    pub is_dir: bool,
    pub modified: f64,
}
pub(super) struct Parsed {
    pub entries: Vec<StorageEntry>,
    pub quota: Option<u64>,
}
#[derive(Default)]
struct Draft {
    href: String,
    size: String,
    modified: String,
    quota: String,
    status: String,
    propstats: u32,
    has_prop: bool,
    is_dir: bool,
}
fn path_is(path: &[(bool, String)], expected: &[&str]) -> bool {
    path.len() == expected.len()
        && path
            .iter()
            .zip(expected)
            .all(|((dav, name), expected)| *dav && name == expected)
}
fn text(draft: &mut Option<Draft>, path: &[(bool, String)], value: &str) {
    let Some(draft) = draft else { return };
    if path_is(path, &["multistatus", "response", "href"]) {
        draft.href.push_str(value);
    }
    if path_is(path, &["multistatus", "response", "status"]) {
        draft.status.push_str(value);
    }
    if draft.propstats != 1 {
        return;
    }
    if path_is(
        path,
        &[
            "multistatus",
            "response",
            "propstat",
            "prop",
            "getcontentlength",
        ],
    ) {
        draft.size.push_str(value);
    }
    if path_is(
        path,
        &[
            "multistatus",
            "response",
            "propstat",
            "prop",
            "getlastmodified",
        ],
    ) {
        draft.modified.push_str(value);
    }
    if path_is(
        path,
        &[
            "multistatus",
            "response",
            "propstat",
            "prop",
            "quota-available-bytes",
        ],
    ) {
        draft.quota.push_str(value);
    }
}
pub(super) fn parse(
    document: &str,
    request: &Url,
    cancel: &dyn Fn() -> bool,
) -> Result<Parsed, DavError> {
    if document.len() > MAX_XML {
        return Err(DavError::Xml);
    }
    let document = document.strip_prefix('\u{feff}').unwrap_or(document);
    let mut reader = NsReader::from_str(document);
    reader.config_mut().expand_empty_elements = true;
    let mut path = Vec::new();
    let mut draft = None;
    let mut seen_root = false;
    let mut declaration_allowed = true;
    let mut parsed = Parsed {
        entries: Vec::new(),
        quota: None,
    };
    loop {
        if cancel() {
            return Err(DavError::Cancelled);
        }
        let (namespace, event) = reader.read_resolved_event().map_err(|_| DavError::Xml)?;
        let dav = match namespace {
            ResolveResult::Bound(namespace) => namespace.as_ref() == "DAV:",
            ResolveResult::Unbound => false,
            ResolveResult::Unknown(_) => return Err(DavError::Xml),
        };
        if matches!(&event, Event::Decl(_)) && !declaration_allowed {
            return Err(DavError::Xml);
        }
        declaration_allowed = false;
        match event {
            Event::Start(start) => {
                if path.len() >= 64 {
                    return Err(DavError::Xml);
                }
                let name = start.local_name().as_ref().to_owned();
                if path.is_empty() {
                    if seen_root || !dav || name != "multistatus" {
                        return Err(DavError::Xml);
                    }
                    seen_root = true;
                }
                path.push((dav, name));
                if path_is(&path, &["multistatus", "response"]) {
                    draft = Some(Draft::default());
                }
                if let Some(draft) = &mut draft {
                    if path_is(&path, &["multistatus", "response", "propstat"]) {
                        draft.propstats = draft.propstats.checked_add(1).ok_or(DavError::Xml)?;
                    }
                    if draft.propstats == 1
                        && path_is(&path, &["multistatus", "response", "propstat", "prop"])
                    {
                        draft.has_prop = true;
                    }
                    if draft.propstats == 1
                        && path_is(
                            &path,
                            &[
                                "multistatus",
                                "response",
                                "propstat",
                                "prop",
                                "resourcetype",
                                "collection",
                            ],
                        )
                    {
                        draft.is_dir = true;
                    }
                }
            }
            Event::End(_) => {
                if path_is(&path, &["multistatus", "response"]) {
                    let draft = draft.take().ok_or(DavError::Xml)?;
                    if !draft.status.trim().is_empty() {
                        let code = draft
                            .status
                            .split_whitespace()
                            .nth(1)
                            .and_then(|s| s.parse::<u16>().ok())
                            .ok_or(DavError::Xml)?;
                        if code >= 400 {
                            return Err(DavError::Http(code));
                        }
                    }
                    if parsed.quota.is_none() {
                        parsed.quota = draft.quota.trim().parse::<u64>().ok();
                    }
                    if draft.href.is_empty() {
                        return Err(DavError::Xml);
                    }
                    let href = request.join(&draft.href).map_err(|_| DavError::Xml)?;
                    let href = href.path().trim_end_matches('/');
                    if href != request.path().trim_end_matches('/') {
                        if !draft.has_prop {
                            return Err(DavError::Xml);
                        }
                        let name = href.rsplit('/').next().unwrap_or("");
                        if !name.is_empty() {
                            let modified =
                                chrono::DateTime::parse_from_rfc2822(draft.modified.trim())
                                    .map(|date| date.timestamp() as f64)
                                    .unwrap_or(0.0);
                            parsed.entries.push(StorageEntry {
                                name: percent_encoding::percent_decode_str(name)
                                    .decode_utf8_lossy()
                                    .into_owned(),
                                size: draft.size.trim().parse().unwrap_or(0),
                                is_dir: draft.is_dir,
                                modified,
                            });
                        }
                    }
                }
                path.pop().ok_or(DavError::Xml)?;
            }
            Event::Text(value) => {
                let value = value.xml_content(XmlVersion::Implicit1_0);
                if path.is_empty() && !value.trim().is_empty() {
                    return Err(DavError::Xml);
                }
                text(&mut draft, &path, &value);
            }
            Event::CData(value) => {
                if path.is_empty() {
                    return Err(DavError::Xml);
                }
                text(
                    &mut draft,
                    &path,
                    &value.xml_content(XmlVersion::Implicit1_0),
                );
            }
            Event::GeneralRef(value) => {
                if path.is_empty() {
                    return Err(DavError::Xml);
                }
                let resolved =
                    if let Some(character) = value.resolve_char_ref().map_err(|_| DavError::Xml)? {
                        character.to_string()
                    } else {
                        quick_xml::escape::resolve_predefined_entity(value.as_ref())
                            .ok_or(DavError::Xml)?
                            .to_owned()
                    };
                text(&mut draft, &path, &resolved);
            }
            Event::DocType(_) | Event::Empty(_) => return Err(DavError::Xml),
            Event::Eof => {
                if !seen_root || !path.is_empty() {
                    return Err(DavError::Xml);
                }
                return Ok(parsed);
            }
            _ => {}
        }
    }
}
