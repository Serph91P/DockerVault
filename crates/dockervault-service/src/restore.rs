//! Bounded metadata reader. This is not an extraction API or a validated destination plan.
mod destination;
mod index;
pub use index::IndexedArchive;
use std::io::{self, Read, Seek, SeekFrom};

const MAX_EXTENSION: u64 = 1024 * 1024;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemberKind {
    File,
    Directory,
    Symlink,
    Hardlink,
}
pub struct Member {
    pub name: Vec<u8>,
    pub link_name: Option<Vec<u8>>,
    pub kind: MemberKind,
    pub size: u64,
    pub data_offset: u64,
    pub mode: u32,
    pub mtime: f64,
}
impl Member {
    pub fn data_mode(&self) -> Option<u32> {
        match self.kind {
            MemberKind::File | MemberKind::Hardlink => {
                Some(dockervault_core::archive::data_file_mode(self.mode))
            }
            MemberKind::Directory | MemberKind::Symlink => None,
        }
    }
}
#[derive(Clone, Default)]
struct Overrides {
    path: Option<Vec<u8>>,
    link: Option<Vec<u8>>,
    size: Option<u64>,
    mtime: Option<f64>,
}
impl Overrides {
    fn overlay(&mut self, other: Self) {
        if other.path.is_some() {
            self.path = other.path;
        }
        if other.link.is_some() {
            self.link = other.link;
        }
        if other.size.is_some() {
            self.size = other.size;
        }
        if other.mtime.is_some() {
            self.mtime = other.mtime;
        }
    }
    fn first(&mut self, other: Self) {
        if self.path.is_none() {
            self.path = other.path;
        }
        if self.link.is_none() {
            self.link = other.link;
        }
        if self.size.is_none() {
            self.size = other.size;
        }
        if self.mtime.is_none() {
            self.mtime = other.mtime;
        }
    }
}
pub struct ArchiveReader<R> {
    source: R,
    length: u64,
    offset: u64,
    global: Overrides,
    done: bool,
}
impl<R: Read + Seek> ArchiveReader<R> {
    pub fn new(mut source: R) -> io::Result<Self> {
        let length = source.seek(SeekFrom::End(0))?;
        source.rewind()?;
        Ok(Self {
            source,
            length,
            offset: 0,
            global: Overrides::default(),
            done: false,
        })
    }
    fn member(&mut self) -> io::Result<Option<Member>> {
        let mut pending = Overrides::default();
        let mut extension = false;
        loop {
            if self.offset == self.length && !extension {
                return Ok(None);
            }
            self.source.seek(SeekFrom::Start(self.offset))?;
            let mut block = [0u8; 512];
            self.source.read_exact(&mut block)?;
            if block.iter().all(|&b| b == 0) {
                if extension {
                    return Err(invalid("Extension has no following member"));
                }
                return Ok(None);
            }
            let h = tar::Header::from_byte_slice(&block);
            let unsigned: u32 = block
                .iter()
                .enumerate()
                .map(|(i, &b)| {
                    if (148..156).contains(&i) {
                        32
                    } else {
                        u32::from(b)
                    }
                })
                .sum();
            let signed: i64 = block
                .iter()
                .enumerate()
                .map(|(i, &b)| {
                    if (148..156).contains(&i) {
                        32
                    } else {
                        i64::from(b as i8)
                    }
                })
                .sum();
            let checksum = h.cksum()?;
            if checksum != unsigned && i64::from(checksum) != signed {
                return Err(invalid("Invalid tar header checksum"));
            }
            let kind = h.entry_type();
            let raw_size = h.entry_size()?;
            let data_offset = self
                .offset
                .checked_add(512)
                .ok_or_else(|| invalid("Archive offset overflow"))?;
            let special = kind.is_pax_local_extensions()
                || kind.is_pax_global_extensions()
                || kind.is_gnu_longname()
                || kind.is_gnu_longlink();
            let size = if special {
                raw_size
            } else {
                pending.size.or(self.global.size).unwrap_or(raw_size)
            };
            let padded = size
                .checked_add(511)
                .map(|v| v / 512 * 512)
                .ok_or_else(|| invalid("Archive size overflow"))?;
            let next = data_offset
                .checked_add(padded)
                .ok_or_else(|| invalid("Archive offset overflow"))?;
            if data_offset
                .checked_add(size)
                .is_none_or(|end| end > self.length)
            {
                return Err(invalid("Truncated archive payload"));
            }
            self.offset = next;
            if special {
                if size > MAX_EXTENSION {
                    return Err(invalid("Archive metadata extension exceeds bounded limit"));
                }
                let mut data = vec![0; size as usize];
                self.source.read_exact(&mut data)?;
                if kind.is_pax_local_extensions() || kind.is_pax_global_extensions() {
                    let mut values = Overrides::default();
                    for field in tar::PaxExtensions::new(&data) {
                        let field = field?;
                        let value = field.value_bytes();
                        match field.key_bytes() {
                            b"path" => values.path = Some(trim_slashes(value).to_vec()),
                            b"linkpath" => values.link = Some(value.to_vec()),
                            b"size" => {
                                values.size = Some(
                                    std::str::from_utf8(value)
                                        .ok()
                                        .and_then(|v| v.parse().ok())
                                        .unwrap_or(0),
                                )
                            }
                            b"mtime" => {
                                values.mtime = Some(
                                    std::str::from_utf8(value)
                                        .ok()
                                        .and_then(|v| v.parse().ok())
                                        .unwrap_or(0.0),
                                )
                            }
                            _ => {}
                        }
                    }
                    if kind.is_pax_global_extensions() {
                        self.global.overlay(values);
                    } else {
                        let mut local = self.global.clone();
                        local.overlay(values);
                        pending.first(local);
                    }
                } else {
                    while data.last() == Some(&0) {
                        data.pop();
                    }
                    if kind.is_gnu_longname() {
                        pending.path.get_or_insert(data);
                    } else {
                        pending.link.get_or_insert(data);
                    }
                }
                extension = true;
                continue;
            }
            let member_kind = if kind.is_file() || kind.as_byte() == b'7' {
                MemberKind::File
            } else if kind.is_dir() {
                MemberKind::Directory
            } else if kind.is_symlink() {
                MemberKind::Symlink
            } else if kind.is_hard_link() {
                MemberKind::Hardlink
            } else {
                return Err(invalid("Unsupported archive member type"));
            };
            let mut name = pending
                .path
                .or_else(|| self.global.path.clone())
                .unwrap_or_else(|| h.path_bytes().into_owned());
            if member_kind == MemberKind::Directory {
                name.truncate(trim_slashes(&name).len());
            }
            let link_name = pending
                .link
                .or_else(|| self.global.link.clone())
                .or_else(|| h.link_name_bytes().map(|v| v.into_owned()));
            if name.starts_with(b"/")
                || name.contains(&0)
                || link_name.as_ref().is_some_and(|v| v.contains(&0))
            {
                return Err(invalid("Invalid archive path"));
            }
            let mtime = pending
                .mtime
                .or(self.global.mtime)
                .map_or_else(|| h.mtime().map(|v| v as f64), Ok)?;
            if !mtime.is_finite() {
                return Err(invalid("Invalid nonfinite archive timestamp"));
            }
            return Ok(Some(Member {
                name,
                link_name,
                kind: member_kind,
                size,
                data_offset,
                mode: h.mode()?,
                mtime,
            }));
        }
    }
}
impl<R: Read + Seek> Iterator for ArchiveReader<R> {
    type Item = io::Result<Member>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.member() {
            Ok(Some(member)) => Some(Ok(member)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(error) => {
                self.done = true;
                Some(Err(error))
            }
        }
    }
}
fn trim_slashes(mut value: &[u8]) -> &[u8] {
    while value.ends_with(b"/") {
        value = &value[..value.len() - 1];
    }
    value
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
