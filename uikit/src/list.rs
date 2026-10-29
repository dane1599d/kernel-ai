//! The display list's types: what `uikit` hands to `gfx2d`, in tokens, type
//! styles and logical px, and its text form for the golden tests.

use crate::geom::Rect;
use crate::style::{Token, TypeStyle};
use crate::widget::NodeId;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// One drawing step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Fill {
        rect: Rect,
        token: Token,
        radius: i32,
    },
    Stroke {
        rect: Rect,
        token: Token,
        width: i32,
        radius: i32,
    },
    /// One line of text; `y` is the top of its line box.
    Text {
        x: i32,
        y: i32,
        style: TypeStyle,
        token: Token,
        text: String,
    },
    /// A Lucide icon by name, drawn at `rect`.
    Icon {
        rect: Rect,
        name: &'static str,
        token: Token,
    },
    Image {
        rect: Rect,
        source: String,
    },
    /// Narrows the clip until the matching `PopClip`.
    PushClip(Rect),
    PopClip,
}

/// One node's drawing, clipped to `clip`; `bounds` holds every op.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub node: NodeId,
    pub clip: Rect,
    pub bounds: Rect,
    pub ops: Vec<Op>,
}

/// The window's whole display list, in paint order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayList {
    pub groups: Vec<Group>,
}

impl fmt::Display for DisplayList {
    /// One line per op, the golden tests' form.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let r = |r: &Rect| alloc::format!("{},{} {}x{}", r.x, r.y, r.w, r.h);
        for g in &self.groups {
            writeln!(f, "node {} clip {}", g.node, r(&g.clip))?;
            for op in &g.ops {
                match op {
                    Op::Fill {
                        rect,
                        token,
                        radius,
                    } => writeln!(f, "  fill {} {} r{}", r(rect), token.name(), radius)?,
                    Op::Stroke {
                        rect,
                        token,
                        width,
                        radius,
                    } => writeln!(
                        f,
                        "  stroke {} {} w{} r{}",
                        r(rect),
                        token.name(),
                        width,
                        radius
                    )?,
                    Op::Text {
                        x,
                        y,
                        style,
                        token,
                        text,
                    } => writeln!(
                        f,
                        "  text {},{} {} {} {:?}",
                        x,
                        y,
                        style.name(),
                        token.name(),
                        text
                    )?,
                    Op::Icon { rect, name, token } => {
                        writeln!(f, "  icon {} {} {}", r(rect), name, token.name())?
                    }
                    Op::Image { rect, source } => writeln!(f, "  image {} {:?}", r(rect), source)?,
                    Op::PushClip(c) => writeln!(f, "  clip {}", r(c))?,
                    Op::PopClip => writeln!(f, "  unclip")?,
                }
            }
        }
        Ok(())
    }
}
