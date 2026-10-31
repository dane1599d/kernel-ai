//! `kai:ui@0.1.0`'s types as `appabi` values and back: what an app passes to
//! `view` lifted into `uikit`'s types, and `uikit`'s events lowered for `on-ui`.
//! Lifting already checked every value against its WIT type; what is left to
//! refuse here is a value that does not fit `uikit` (a case out of range).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use appabi::Val;
use uikit::style::{Tone, TypeStyle};
use uikit::widget::{
    Axis, CodeRow, CodeSpan, CodeView, Insets, Layout, Size, TextField, UiEvent, Value, Widget,
};

fn bad(what: &str, v: Option<&Val>) -> String {
    format!("{what} expected, got {v:?}")
}

pub(crate) fn u16_of(v: Option<&Val>) -> Result<u16, String> {
    match v {
        Some(Val::U16(n)) => Ok(*n),
        other => Err(bad("u16", other)),
    }
}

pub(crate) fn u32_of(v: Option<&Val>) -> Result<u32, String> {
    match v {
        Some(Val::U32(n)) => Ok(*n),
        other => Err(bad("u32", other)),
    }
}

fn s32_of(v: Option<&Val>) -> Result<i32, String> {
    match v {
        Some(Val::S32(n)) => Ok(*n),
        other => Err(bad("s32", other)),
    }
}

pub(crate) fn bool_of(v: Option<&Val>) -> Result<bool, String> {
    match v {
        Some(Val::Bool(b)) => Ok(*b),
        other => Err(bad("bool", other)),
    }
}

pub(crate) fn str_of(v: Option<&Val>) -> Result<String, String> {
    match v {
        Some(Val::String(s)) => Ok(s.clone()),
        other => Err(bad("string", other)),
    }
}

fn case_of(v: Option<&Val>) -> Result<u32, String> {
    match v {
        Some(Val::Enum(n)) => Ok(*n),
        other => Err(bad("enum", other)),
    }
}

fn strings(v: Option<&Val>) -> Result<Vec<String>, String> {
    match v {
        Some(Val::List(items)) => items.iter().map(|i| str_of(Some(i))).collect(),
        other => Err(bad("list<string>", other)),
    }
}

fn option<T>(
    v: Option<&Val>,
    inner: fn(Option<&Val>) -> Result<T, String>,
) -> Result<Option<T>, String> {
    match v {
        Some(Val::Option(None)) => Ok(None),
        Some(Val::Option(Some(x))) => inner(Some(x)).map(Some),
        other => Err(bad("option", other)),
    }
}

fn fields(v: Option<&Val>) -> Result<&[Val], String> {
    match v {
        Some(Val::Record(f)) => Ok(f),
        other => Err(bad("record", other)),
    }
}

fn axis(v: Option<&Val>) -> Result<Axis, String> {
    match case_of(v)? {
        0 => Ok(Axis::Row),
        1 => Ok(Axis::Column),
        other => Err(format!("axis case {other}")),
    }
}

fn size(v: Option<&Val>) -> Result<Size, String> {
    match v {
        Some(Val::Variant(0, None)) => Ok(Size::Fit),
        Some(Val::Variant(1, p)) => u16_of(p.as_deref()).map(Size::Fill),
        Some(Val::Variant(2, p)) => u16_of(p.as_deref()).map(Size::Fixed),
        other => Err(bad("size", other)),
    }
}

/// A `layout` record.
pub(crate) fn layout(v: Option<&Val>) -> Result<Layout, String> {
    let f = fields(v)?;
    let pad = fields(f.get(2))?;
    Ok(Layout {
        width: size(f.first())?,
        height: size(f.get(1))?,
        padding: Insets {
            top: u16_of(pad.first())?,
            right: u16_of(pad.get(1))?,
            bottom: u16_of(pad.get(2))?,
            left: u16_of(pad.get(3))?,
        },
        gap: u16_of(f.get(3))?,
    })
}

/// `text-style`'s cases, in order.
const STYLES: [TypeStyle; 6] = [
    TypeStyle::Small,
    TypeStyle::Body,
    TypeStyle::Label,
    TypeStyle::Heading,
    TypeStyle::Display,
    TypeStyle::Mono,
];
/// `tone`'s cases, in order.
const TONES: [Tone; 6] = [
    Tone::Normal,
    Tone::Muted,
    Tone::Dim,
    Tone::Accent,
    Tone::Ok,
    Tone::Error,
];

fn picked<T: Copy>(table: &[T], v: Option<&Val>, what: &str) -> Result<T, String> {
    let case = case_of(v)?;
    table
        .get(case as usize)
        .copied()
        .ok_or_else(|| format!("{what} case {case}"))
}

fn text_field(f: &[Val]) -> Result<TextField, String> {
    Ok(TextField {
        label: str_of(f.first())?,
        value: str_of(f.get(1))?,
        placeholder: str_of(f.get(2))?,
        max_len: u32_of(f.get(3))?,
    })
}

fn rows(v: Option<&Val>) -> Result<Vec<Vec<String>>, String> {
    match v {
        Some(Val::List(rows)) => rows.iter().map(|r| strings(Some(r))).collect(),
        other => Err(bad("list<list<string>>", other)),
    }
}

/// A `widget` variant.
pub(crate) fn widget(v: Option<&Val>) -> Result<Widget, String> {
    let Some(Val::Variant(case, Some(payload))) = v else {
        return Err(bad("widget", v));
    };
    let f = fields(Some(payload))?;
    Ok(match case {
        0 => Widget::Stack {
            axis: axis(f.first())?,
        },
        1 => Widget::Label {
            text: str_of(f.first())?,
            style: picked(&STYLES, f.get(1), "text-style")?,
            tone: picked(&TONES, f.get(2), "tone")?,
        },
        2 => Widget::Button {
            text: str_of(f.first())?,
            action: option(f.get(1), str_of)?,
        },
        3 => Widget::TextLine(text_field(f)?),
        4 => Widget::TextArea(text_field(f)?),
        5 => Widget::ItemList {
            label: str_of(f.first())?,
            selected: option(f.get(1), u32_of)?,
        },
        6 => Widget::Table {
            label: str_of(f.first())?,
            columns: strings(f.get(1))?,
            rows: rows(f.get(2))?,
            selected: option(f.get(3), u32_of)?,
        },
        7 => Widget::Tabs {
            labels: strings(f.first())?,
            selected: u32_of(f.get(1))?,
        },
        8 => Widget::Toggle {
            label: str_of(f.first())?,
            on: bool_of(f.get(1))?,
        },
        9 => Widget::Slider {
            label: str_of(f.first())?,
            min: s32_of(f.get(1))?,
            max: s32_of(f.get(2))?,
            step: s32_of(f.get(3))?,
            value: s32_of(f.get(4))?,
        },
        10 => Widget::Menu {
            label: str_of(f.first())?,
            items: strings(f.get(1))?,
        },
        11 => Widget::Scroll {
            axis: axis(f.first())?,
        },
        12 => Widget::Split {
            axis: axis(f.first())?,
            first: u16_of(f.get(1))?,
        },
        13 => Widget::Image {
            label: str_of(f.first())?,
            source: str_of(f.get(1))?,
        },
        14 => Widget::Progress {
            label: str_of(f.first())?,
            done: option(f.get(1), u16_of)?,
        },
        other => return Err(format!("widget case {other}")),
    })
}

fn tone(v: Option<&Val>) -> Result<Tone, String> {
    picked(&TONES, v, "tone")
}

fn code_span(v: Option<&Val>) -> Result<CodeSpan, String> {
    let f = fields(v)?;
    Ok(CodeSpan {
        from: u16_of(f.first())?,
        to: u16_of(f.get(1))?,
        tone: tone(f.get(2))?,
        link: bool_of(f.get(3))?,
    })
}

fn code_row(v: Option<&Val>) -> Result<CodeRow, String> {
    let f = fields(v)?;
    let spans = match f.get(2) {
        Some(Val::List(items)) => items
            .iter()
            .map(|s| code_span(Some(s)))
            .collect::<Result<_, _>>()?,
        other => return Err(bad("list<span>", other)),
    };
    Ok(CodeRow {
        text: str_of(f.first())?,
        gutter: str_of(f.get(1))?,
        spans,
    })
}

/// A `kai:code/code` `code-view` record.
pub(crate) fn code_view(v: Option<&Val>) -> Result<CodeView, String> {
    let f = fields(v)?;
    let rows = match f.get(2) {
        Some(Val::List(items)) => items
            .iter()
            .map(|r| code_row(Some(r)))
            .collect::<Result<_, _>>()?,
        other => return Err(bad("list<row>", other)),
    };
    let cursor = |v: Option<&Val>| match v {
        Some(Val::Tuple(t)) => Ok((u32_of(t.first())?, u32_of(t.get(1))?)),
        other => Err(bad("tuple<u32, u32>", other)),
    };
    Ok(CodeView {
        label: str_of(f.first())?,
        gutter_width: u16_of(f.get(1))?,
        rows,
        cursor: option(f.get(3), cursor)?,
        selected: option(f.get(4), u32_of)?,
    })
}

/// An event from `window` as `kai:ui/input`'s `ui-event`; None for the code
/// view's, which go to `kai:code/code-input` ([`crate::CoreInstance::code_event`]).
pub(crate) fn event(window: u32, e: &UiEvent) -> Option<Val> {
    let some = |v: Val| Some(Box::new(v));
    Some(match e {
        UiEvent::Pressed(node) => Val::Variant(0, some(Val::U32(*node))),
        UiEvent::Changed(node, value) => {
            let value = match value {
                Value::Text(t) => Val::Variant(0, some(Val::String(t.clone()))),
                Value::On(b) => Val::Variant(1, some(Val::Bool(*b))),
                Value::Number(n) => Val::Variant(2, some(Val::S32(*n))),
                Value::Index(i) => Val::Variant(3, some(Val::U32(*i))),
            };
            Val::Variant(1, some(Val::Tuple(vec![Val::U32(*node), value])))
        }
        UiEvent::Submitted(node) => Val::Variant(2, some(Val::U32(*node))),
        UiEvent::CloseRequested => Val::Variant(3, some(Val::U32(window))),
        UiEvent::Key { .. } | UiEvent::Click { .. } | UiEvent::Scroll { .. } => return None,
    })
}
