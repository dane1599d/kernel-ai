//! What draws a page for the harness. B-1's `webcore` will be one; until it
//! lands there is only [`Stub`].

use super::compare::Picture;
use std::path::Path;

/// A page to draw: its file under the WPT root, its url (an engine resolves
/// the page's relative links against it), and the viewport.
pub struct Page<'a> {
    pub file: &'a Path,
    pub url: &'a str,
    pub width: u32,
    pub height: u32,
}

/// Draws pages. Errors are the test's (`ERROR` in the results), not the
/// run's: a renderer that cannot draw one page goes on to the next.
pub trait Renderer {
    /// Its name, in the results.
    fn name(&self) -> &str;
    fn render(&mut self, page: &Page) -> Result<Picture, String>;
}

/// The renderers `--renderer` names.
pub fn by_name(name: &str) -> Option<Box<dyn Renderer>> {
    match name {
        "stub" => Some(Box::new(Stub)),
        _ => None,
    }
}

/// Stands in for an engine until B-1: each page is one flat colour taken from
/// its file's bytes. So a test and a reference that are the same file match,
/// and any two different files do not. That exercises the whole harness
/// (fetching, chains, fuzzy rules, the report), but its pass rate says
/// nothing about rendering.
pub struct Stub;

impl Renderer for Stub {
    fn name(&self) -> &str {
        "stub"
    }

    fn render(&mut self, page: &Page) -> Result<Picture, String> {
        let bytes = std::fs::read(page.file)
            .map_err(|e| format!("{} ({}): {e}", page.url, page.file.display()))?;
        // FNV-1a over the bytes, three of them the colour.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in &bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        let [r, g, b, ..] = h.to_le_bytes();
        Ok(Picture::solid(page.width, page.height, [r, g, b, 255]))
    }
}
