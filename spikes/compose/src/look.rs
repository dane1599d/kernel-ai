//! ADR 0005 §8's colour tokens, `0x00RRGGBB`.

#[derive(Clone, Copy, Debug)]
pub struct Look {
    pub dark: bool,
    pub desk: u32,
    pub window: u32,
    pub raised: u32,
    pub line: u32,
    pub line_strong: u32,
    pub text: u32,
    pub muted: u32,
    pub dim: u32,
    /// The AI and nothing else (text; marks use `ai_mark`).
    pub ai: u32,
    pub ai_mark: u32,
    pub ask: u32,
    pub error: u32,
    pub ok: u32,
    pub accent: u32,
    pub focus: u32,
}

pub const DARK: Look = Look {
    dark: true,
    desk: 0x0D1017,
    window: 0x141821,
    raised: 0x1B202B,
    line: 0x232A38,
    line_strong: 0x5F6B80,
    text: 0xECE6DA,
    muted: 0x9AA3B4,
    dim: 0x7C8AA8,
    ai: 0xFF9F5A,
    ai_mark: 0xFF9F5A,
    ask: 0xF5C451,
    error: 0xFF6B6B,
    ok: 0x7FD49B,
    accent: 0x8CB8FF,
    focus: 0xECE6DA,
};

pub const LIGHT: Look = Look {
    dark: false,
    desk: 0xE9E4DA,
    window: 0xFAF8F4,
    raised: 0xFFFFFF,
    line: 0xD9D3C7,
    line_strong: 0x857E71,
    text: 0x1A1D24,
    muted: 0x4F5666,
    dim: 0x5B6271,
    ai: 0xA84A0C,
    ai_mark: 0xC4560C,
    ask: 0x7F5800,
    error: 0xB42318,
    ok: 0x26703F,
    accent: 0x1F5FBF,
    focus: 0x1A1D24,
};
