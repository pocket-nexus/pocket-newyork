//! The PS Vita's pack (`city.pack`, `cargo run --release -p ny-cook`) read through the kernel's `Source`: in a tab
//! the pack's file answered by ranges, or its pieces (`bun tools/web.ts dist`); on the build machine the file.
//! Its tables and the sections a frame needs from the start are read before the first frame (`city::CityGpu::load`);
//! a cell's record of `NEAR` and a window of `HMAP` as the eye comes near.

use std::collections::HashMap;

use ny_pack::Index;
use pocket_web_wgpu::source::Source;

#[derive(Clone)]
pub struct Pack {
    pub source: Source,
    index: std::rc::Rc<Index>,
}

impl Pack {
    /// The pack at `place`: its head and its table of sections.
    pub async fn open(place: &str) -> Result<Pack, String> {
        let source = Source::open(place).await?;
        let head = source.range(0, 16).await?;
        let count = Index::count(&head)?;
        let table = source.range(0, Index::head_bytes(count) as u64).await?;
        Ok(Pack { source, index: std::rc::Rc::new(Index::parse(&table)?) })
    }

    /// Offset and size of a section.
    pub fn range(&self, tag: u32) -> Result<(usize, usize), String> {
        self.index.range(tag)
    }

    pub fn has(&self, tag: u32) -> bool {
        self.index.range(tag).is_ok()
    }

    /// A section.
    pub async fn read(&self, tag: u32) -> Result<Vec<u8>, String> {
        let (at, size) = self.range(tag)?;
        self.source.range(at as u64, size as u64).await
    }

    /// `len` bytes of a section from `at`.
    pub async fn read_part(&self, tag: u32, at: usize, len: usize) -> Result<Vec<u8>, String> {
        let (offset, size) = self.range(tag)?;
        if at + len > size {
            return Err("read past the end of a pack section".into());
        }
        self.source.range((offset + at) as u64, len as u64).await
    }

    /// Several sections in one read of the bytes from the first to the end of the last (sections lie in the order
    /// the compiler wrote them): `tags` that the pack has, each as its own bytes.
    pub async fn read_run(&self, tags: &[u32]) -> Result<HashMap<u32, Vec<u8>>, String> {
        let ranges: Vec<(u32, usize, usize)> = tags.iter().filter_map(|&t| self.index.range(t).ok().map(|(at, size)| (t, at, size))).collect();
        let Some(from) = ranges.iter().map(|r| r.1).min() else { return Ok(HashMap::new()) };
        let to = ranges.iter().map(|r| r.1 + r.2).max().unwrap_or(from);
        let bytes = self.source.range(from as u64, (to - from) as u64).await?;
        Ok(ranges.into_iter().map(|(t, at, size)| (t, bytes[at - from..at - from + size].to_vec())).collect())
    }

    /// Every section's tag, in the pack's order.
    pub fn tags(&self) -> Vec<u32> {
        self.index.sections.iter().map(|s| s.0).collect()
    }
}
