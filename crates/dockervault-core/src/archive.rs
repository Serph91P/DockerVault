//! Streaming layout classification only; filesystem/link/type validation must precede extraction.

/// Keeps one candidate root instead of retaining an archive's member list.
pub struct LayoutScan {
    strip: bool,
    required: bool,
    root: Option<Vec<u8>>,
    consistent: bool,
    seen: bool,
}

pub struct RootMapping(Option<Vec<u8>>);

impl LayoutScan {
    /// None keeps container/stack roots; empty auto-detects legacy single-source roots.
    pub fn new(strip_root: Option<&[u8]>) -> Self {
        Self {
            strip: strip_root.is_some(),
            required: strip_root.is_some_and(|root| !root.is_empty()),
            root: strip_root
                .filter(|root| !root.is_empty())
                .map(<[u8]>::to_vec),
            consistent: true,
            seen: false,
        }
    }

    pub fn observe(&mut self, name: &[u8], directory: bool) -> Result<(), &'static str> {
        validate(name)?;
        self.seen = true;
        if !self.strip {
            return Ok(());
        }
        let name = directory_name(name, directory);
        let root = self.root.get_or_insert_with(|| {
            name.split(|&byte| byte == b'/')
                .next()
                .unwrap_or_default()
                .to_vec()
        });
        self.consistent &= matches_root(root, name, directory);
        Ok(())
    }

    pub fn finish(self) -> Result<RootMapping, &'static str> {
        if self.required && self.seen && !self.consistent {
            return Err("Archive members do not match the recorded layout");
        }
        Ok(RootMapping(if self.strip && self.seen && self.consistent {
            self.root
        } else {
            None
        }))
    }
}

impl RootMapping {
    pub fn member(&self, name: &[u8], directory: bool) -> Result<Vec<u8>, &'static str> {
        validate(name)?;
        let name = directory_name(name, directory);
        match &self.0 {
            None => Ok(name.to_vec()),
            Some(root) if name == root && directory => Ok(b".".to_vec()),
            Some(root) if matches_root(root, name, directory) => {
                Ok(name[root.len() + 1..].to_vec())
            }
            _ => Err("Archive member changed after layout validation"),
        }
    }

    pub fn hardlink(&self, target: &[u8]) -> Result<Vec<u8>, &'static str> {
        validate(target)?;
        match &self.0 {
            None => Ok(target.to_vec()),
            Some(root) if target.starts_with(root) && target.get(root.len()) == Some(&b'/') => {
                Ok(target[root.len() + 1..].to_vec())
            }
            _ => Err("Hardlink escapes the single-source archive root"),
        }
    }
}

fn validate(name: &[u8]) -> Result<(), &'static str> {
    if name.starts_with(b"/") || name.contains(&0) {
        Err("Invalid absolute or NUL archive path")
    } else {
        Ok(())
    }
}

fn directory_name(mut name: &[u8], directory: bool) -> &[u8] {
    if directory {
        while name.ends_with(b"/") {
            name = &name[..name.len() - 1];
        }
    }
    name
}

fn matches_root(root: &[u8], name: &[u8], directory: bool) -> bool {
    !matches!(root, b"" | b"." | b"..")
        && ((name == root && directory)
            || (name.starts_with(root) && name.get(root.len()) == Some(&b'/')))
}
