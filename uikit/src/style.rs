//! The look's names and the widgets' sizes (ADR 0005 section 8).
//!
//! `Token` and `TypeStyle` are names only, as `look` defines them; colour values
//! never appear outside `look`. `Token` is the twelve an app's widgets may use:
//! `ai` and `ask` belong to `desktop` (ADR 0005 sections 4 and 7), so uikit cannot
//! name them. When `look` lands these map onto its tokens and nothing else changes.

/// A colour token, by job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Token {
    Desk,
    Window,
    Raised,
    Line,
    LineStrong,
    Text,
    Muted,
    Dim,
    Error,
    Ok,
    Accent,
    Focus,
}

impl Token {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Desk => "desk",
            Self::Window => "window",
            Self::Raised => "raised",
            Self::Line => "line",
            Self::LineStrong => "line_strong",
            Self::Text => "text",
            Self::Muted => "muted",
            Self::Dim => "dim",
            Self::Error => "error",
            Self::Ok => "ok",
            Self::Accent => "accent",
            Self::Focus => "focus",
        }
    }
}

/// A type style: family, size, line and weight live in `look`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypeStyle {
    Small,
    Body,
    Label,
    Heading,
    Display,
    Mono,
    MonoTitle,
}

impl TypeStyle {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Body => "body",
            Self::Label => "label",
            Self::Heading => "heading",
            Self::Display => "display",
            Self::Mono => "mono",
            Self::MonoTitle => "mono_title",
        }
    }

    /// Size in logical px, from ADR 0005 section 8's type table.
    pub const fn size(self) -> i32 {
        match self {
            Self::Small | Self::MonoTitle => 12,
            Self::Body | Self::Mono => 14,
            Self::Label => 13,
            Self::Heading => 18,
            Self::Display => 28,
        }
    }

    /// Line height in logical px, from the same table: what [`crate::layout::Measure`]
    /// returns unless the font says otherwise.
    pub const fn line(self) -> i32 {
        match self {
            Self::Small | Self::MonoTitle => 16,
            Self::Body | Self::Mono => 20,
            Self::Label => 18,
            Self::Heading => 24,
            Self::Display => 34,
        }
    }
}

/// The colours an app may choose for text (`kai:ui/types.tone`): no `ai`, no `ask`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Muted,
    Dim,
    Accent,
    Ok,
    Error,
}

impl Tone {
    pub const fn token(self) -> Token {
        match self {
            Self::Normal => Token::Text,
            Self::Muted => Token::Muted,
            Self::Dim => Token::Dim,
            Self::Accent => Token::Accent,
            Self::Ok => Token::Ok,
            Self::Error => Token::Error,
        }
    }
}

/// Widget sizes in logical px.
pub mod metric {
    /// Buttons, fields, toggles, sliders and menus.
    pub const CONTROL_H: i32 = 28;
    /// Space left and right of a button's or a field's text.
    pub const CONTROL_PAD: i32 = 10;
    /// Corner radius of controls and lists (the window's is 6 px too).
    pub const RADIUS: i32 = 6;
    /// The focus ring: 2 px wide, 1 px from the control.
    pub const FOCUS_GAP: i32 = 1;
    pub const FOCUS_W: i32 = 2;
    /// A field's label above it, and the space below the label.
    pub const LABEL_GAP: i32 = 4;
    /// Rows of lists and tables.
    pub const ROW_H: i32 = 24;
    /// Space left and right of a list item, inside the list's frame.
    pub const ITEM_PAD: i32 = 8;
    /// The tab bar.
    pub const TAB_H: i32 = 32;
    pub const TAB_PAD: i32 = 12;
    /// A text area's least height, in lines of body text.
    pub const AREA_LINES: i32 = 4;
    /// Space above and below a text area's lines.
    pub const AREA_PAD: i32 = 6;
    /// The toggle's track and knob.
    pub const TOGGLE_W: i32 = 32;
    pub const TOGGLE_H: i32 = 18;
    /// The slider's track and knob.
    pub const SLIDER_W: i32 = 160;
    pub const TRACK_H: i32 = 4;
    pub const KNOB: i32 = 14;
    /// Progress bars.
    pub const PROGRESS_W: i32 = 160;
    pub const PROGRESS_H: i32 = 6;
    /// Scroll bars and the wheel's step.
    pub const SCROLLBAR_W: i32 = 4;
    pub const WHEEL_STEP: i32 = 40;
    /// The divider of a split view.
    pub const DIVIDER: i32 = 1;
    /// Space between a toggle or a slider and its label.
    pub const INLINE_GAP: i32 = 8;
    /// Space inside a code view's box.
    pub const CODE_PAD: i32 = 6;
}
