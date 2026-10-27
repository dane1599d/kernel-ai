//! Value changes, from the user or the agent, and the agent's action requests.
//! The agent never sends synthetic input (ADR 0005 section 6): it asks a node
//! for one of the actions its accessibility entry offers.

use crate::a11y::Action;
use crate::layout::Measure;
use crate::widget::{MAX_TEXT, NodeId, UiEvent, Value, Widget};
use crate::window::Window;
use alloc::vec::Vec;

/// Why an action request was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActError {
    /// No such node, or it is not shown.
    NoNode,
    /// The node does not take that action.
    NotAllowed,
    /// The value is the wrong kind or out of range.
    BadValue,
}

impl Window {
    /// Applies `value` to `id`, as the user or the agent changed it, and the
    /// event the app hears; none if nothing changed.
    pub(crate) fn change(&mut self, id: NodeId, value: Value) -> Result<Option<UiEvent>, ActError> {
        // A slider reports the value it took, not the one asked for.
        let value = match (self.tree.get(id).map(|n| &n.widget), value) {
            (Some(Widget::Slider { min, max, .. }), Value::Number(n)) => {
                Value::Number(n.clamp(*min, (*max).max(*min)))
            }
            (_, v) => v,
        };
        let kids = self.tree.get(id).map_or(0, |n| n.children.len());
        let w = self.tree.widget_mut(id).ok_or(ActError::NoNode)?;
        let changed = match (w, &value) {
            (Widget::TextLine(f) | Widget::TextArea(f), Value::Text(t)) => {
                if t.len() > f.max_len.min(MAX_TEXT) as usize {
                    return Err(ActError::BadValue);
                }
                let c = f.value != *t;
                f.value.clone_from(t);
                c
            }
            (Widget::Toggle { on, .. }, Value::On(v)) => core::mem::replace(on, *v) != *v,
            (
                Widget::Slider {
                    min,
                    max,
                    value: cur,
                    ..
                },
                Value::Number(n),
            ) => {
                let n = (*n).clamp(*min, (*max).max(*min));
                core::mem::replace(cur, n) != n
            }
            (Widget::Tabs { labels, selected }, Value::Index(i)) => {
                if *i as usize >= labels.len() {
                    return Err(ActError::BadValue);
                }
                core::mem::replace(selected, *i) != *i
            }
            (Widget::Table { rows, selected, .. }, Value::Index(i)) => {
                if *i as usize >= rows.len() {
                    return Err(ActError::BadValue);
                }
                selected.replace(*i) != Some(*i)
            }
            (Widget::Menu { items, .. }, Value::Index(i)) => {
                if *i as usize >= items.len() {
                    return Err(ActError::BadValue);
                }
                true
            }
            (Widget::ItemList { selected, .. }, Value::Index(i)) => {
                if *i as usize >= kids {
                    return Err(ActError::BadValue);
                }
                selected.replace(*i) != Some(*i)
            }
            _ => return Err(ActError::BadValue),
        };
        if !changed {
            return Ok(None);
        }
        if let Value::Text(t) = &value {
            self.st.caret.insert(id, t.len());
        }
        self.dirty = true;
        Ok(Some(UiEvent::Changed(id, value)))
    }

    /// The agent's (or a screen reader's) request for one of a node's actions.
    pub fn act(
        &mut self,
        id: NodeId,
        action: Action,
        value: Option<Value>,
        m: &dyn Measure,
    ) -> Result<Vec<UiEvent>, ActError> {
        self.relayout(m);
        let node = crate::a11y::build(&self.tree, &self.places, &self.st, &self.title)
            .into_iter()
            .find(|n| n.id == id)
            .ok_or(ActError::NoNode)?;
        if !node.allows(action) {
            return Err(ActError::NotAllowed);
        }
        let w = self
            .tree
            .get(id)
            .map(|n| n.widget.clone())
            .ok_or(ActError::NoNode)?;
        let mut out = Vec::new();
        match (action, &w, value) {
            (Action::Focus, _, None) => self.set_focus(Some(id)),
            (Action::Click, Widget::Button { .. }, None) => out.push(UiEvent::Pressed(id)),
            (Action::Click, Widget::Toggle { on, .. }, None) => {
                out.extend(self.change(id, Value::On(!on))?)
            }
            (Action::SetValue, _, Some(v)) => out.extend(self.change(id, v)?),
            (Action::Increment | Action::Decrement, Widget::Slider { step, value, .. }, None) => {
                let s = (*step).max(1);
                let v = if action == Action::Increment {
                    value.saturating_add(s)
                } else {
                    value.saturating_sub(s)
                };
                out.extend(self.change(id, Value::Number(v))?);
            }
            _ => return Err(ActError::BadValue),
        }
        Ok(out)
    }
}
