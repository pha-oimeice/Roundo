//! TOML caller contract for native views. Expressions can only read declared data,
//! the view's model, repeat locals and the current action result; never the ECS World.

use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub title: String,
    pub model: BTreeMap<String, Value>,
    pub sources: BTreeMap<String, String>,
    /// A declared source whose `epoch` invalidates drafts and outstanding actions.
    pub session_source: Option<String>,
    #[serde(default)]
    pub on_open: Vec<Action>,
    /// Coalesced after a declared source changes; never interrupts an action.
    #[serde(default)]
    pub on_data: BTreeMap<String, Vec<Action>>,
    #[serde(default)]
    pub shortcuts: BTreeMap<String, Vec<Action>>,
    pub nodes: Vec<NodeDef>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Widget {
    Container,
    Column,
    Row,
    Scroll,
    Text,
    Button,
    TextInput,
    NumberInput,
    Checkbox,
    Select,
    Slider,
    Repeat,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeDef {
    pub id: String,
    pub kind: Widget,
    pub text: Option<Value>,
    pub value: Option<Value>,
    /// Model path written by a user edit. Absent means an explicitly controlled widget.
    pub bind: Option<String>,
    pub visible: Option<Value>,
    pub enabled: Option<Value>,
    pub min: Option<Value>,
    pub max: Option<Value>,
    pub step: Option<Value>,
    pub integer: Option<bool>,
    /// Select options are objects with explicit `value` and `label` members.
    pub options: Option<Value>,
    pub max_visible: Option<usize>,
    pub scrollbar_width: Option<f32>,
    pub items: Option<Value>,
    pub alias: Option<String>,
    pub key: Option<Value>,
    pub style: Option<Style>,
    #[serde(default)]
    pub on_activate: Vec<Action>,
    #[serde(default)]
    pub on_change: Vec<Action>,
    #[serde(default)]
    pub children: Vec<NodeDef>,
}

/// Visual overrides plus Bevy Node fields with their native names and enum values.
/// Node fields may use the same expressions as widget values. Lengths accept
/// logical-pixel numbers or Bevy strings such as "auto", "50%", "80vw", "2rem".
/// Rectangles accept a uniform length or explicit native left/right/top/bottom.
///
/// Grid uses native reflected values, for example:
/// `display = "Grid"`, `grid_column = { start = 1, span = 2 }`, and
/// `grid_template_columns = [{ repetition = { Count = 1 }, tracks = [
/// { min_sizing_function = "Auto", max_sizing_function = { Fraction = 1.0 } }
/// ] }]`. Bevy, not the resource adapter, implements sizing and placement.
#[derive(Clone, Debug, Deserialize)]
pub struct Style {
    pub background: Option<[u8; 4]>,
    pub color: Option<[u8; 4]>,
    pub font_size: Option<f32>,
    /// Unknown names are rejected against Node's actual reflected schema at load.
    #[serde(flatten)]
    pub node: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Action {
    Set {
        path: String,
        value: Value,
    },
    Command {
        name: String,
        arguments: Value,
        save: Option<String>,
    },
    /// Waits for the exact current-epoch native debug receipt, not command admission.
    Debug {
        operation: Value,
        save: Option<String>,
    },
    Require {
        condition: Value,
        message: String,
    },
    Wait {
        seconds: f32,
    },
}

impl Page {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.title.trim().is_empty() {
            return Err("page title is empty".into());
        }
        if let Some(source) = &self.session_source {
            if self.sources.get(source).map(String::as_str) != Some("client.debug") {
                return Err("session_source must alias client.debug".into());
            }
        }
        for name in self.sources.keys() {
            if !identifier(name) {
                return Err(format!("invalid source alias `{name}`"));
            }
        }
        validate_actions(&self.on_open)?;
        for (source, actions) in &self.on_data {
            if !self.sources.contains_key(source) {
                return Err(format!("on_data references undeclared source {source}"));
            }
            validate_actions(actions)?;
        }
        for actions in self.shortcuts.values() {
            validate_actions(actions)?;
        }
        validate_nodes(&self.nodes)?;
        let mut roots = ["model", "data", "event", "result", "error", "busy"]
            .into_iter()
            .map(str::to_owned)
            .collect::<std::collections::BTreeSet<_>>();
        roots.extend(self.model.keys().map(|name| format!("model.{name}")));
        roots.extend(self.sources.keys().map(|name| format!("data.{name}")));
        let model = serde_json::to_value(&self.model).map_err(|e| e.to_string())?;
        validate_expressions(&self.nodes, &roots, &model)?;
        validate_action_expressions(&self.on_open, &roots, &model)?;
        for actions in self.on_data.values() {
            validate_action_expressions(actions, &roots, &model)?;
        }
        for actions in self.shortcuts.values() {
            validate_action_expressions(actions, &roots, &model)?;
        }
        Ok(())
    }
}

pub(crate) fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn validate_nodes(nodes: &[NodeDef]) -> Result<(), String> {
    let mut ids = std::collections::BTreeSet::new();
    for node in nodes {
        if !identifier(&node.id) || !ids.insert(&node.id) {
            return Err(format!("invalid/duplicate sibling node ID `{}`", node.id));
        }
        match node.kind {
            Widget::Text | Widget::Button | Widget::Checkbox if node.text.is_none() => {
                return Err(format!("{} requires text", node.id));
            }
            Widget::TextInput
            | Widget::NumberInput
            | Widget::Checkbox
            | Widget::Select
            | Widget::Slider
                if node.value.is_none() =>
            {
                return Err(format!("{} requires value", node.id));
            }
            Widget::Repeat
                if node.items.is_none()
                    || node.key.is_none()
                    || !node.alias.as_deref().is_some_and(identifier) =>
            {
                return Err(format!("{} requires items, alias and key", node.id));
            }
            _ => {}
        }
        if !node.on_activate.is_empty() && node.kind != Widget::Button {
            return Err(format!("{}: on_activate requires a button", node.id));
        }
        if node.text.is_some()
            && !matches!(node.kind, Widget::Text | Widget::Button | Widget::Checkbox)
        {
            return Err(format!("{}: text requires text/button/checkbox", node.id));
        }
        let editable = matches!(
            node.kind,
            Widget::TextInput
                | Widget::NumberInput
                | Widget::Checkbox
                | Widget::Select
                | Widget::Slider
        );
        if (node.bind.is_some() || node.value.is_some() || !node.on_change.is_empty()) && !editable
        {
            return Err(format!(
                "{}: value/bind/on_change require an editable widget",
                node.id
            ));
        }
        if (node.items.is_some() || node.alias.is_some() || node.key.is_some())
            && node.kind != Widget::Repeat
        {
            return Err(format!("{}: items/alias/key require a repeat", node.id));
        }
        if (node.options.is_some() || node.max_visible.is_some()) && node.kind != Widget::Select {
            return Err(format!("{}: options/max_visible require a select", node.id));
        }
        if node.integer.is_some() && node.kind != Widget::NumberInput {
            return Err(format!("{}: integer requires number-input", node.id));
        }
        if (node.min.is_some() || node.max.is_some() || node.step.is_some())
            && !matches!(node.kind, Widget::NumberInput | Widget::Slider)
        {
            return Err(format!(
                "{}: numeric limits require a numeric widget",
                node.id
            ));
        }
        if node.scrollbar_width.is_some() && node.kind != Widget::Scroll {
            return Err(format!("{}: scrollbar_width requires scroll", node.id));
        }
        if node.kind == Widget::Scroll
            && node
                .scrollbar_width
                .is_none_or(|v| !v.is_finite() || v <= 0.0)
        {
            return Err(format!("{} requires positive scrollbar_width", node.id));
        }
        if node.kind == Widget::Select
            && (node.options.is_none() || node.max_visible.is_none_or(|v| v == 0))
        {
            return Err(format!(
                "{} requires options and positive max_visible",
                node.id
            ));
        }
        if node.kind == Widget::Slider && (node.min.is_none() || node.max.is_none()) {
            return Err(format!("{} requires min/max", node.id));
        }
        if node.kind == Widget::NumberInput && node.integer.is_none() {
            return Err(format!("{} requires explicit integer mode", node.id));
        }
        if let Some(bind) = &node.bind {
            validate_model_path(bind)?;
        }
        if !matches!(
            node.kind,
            Widget::Container | Widget::Column | Widget::Row | Widget::Scroll | Widget::Repeat
        ) && !node.children.is_empty()
        {
            return Err(format!(
                "{}: leaf widget cannot have schema children",
                node.id
            ));
        }
        if let Some(style) = &node.style {
            super::layout::validate(&style.node)
                .map_err(|error| format!("{}: {error}", node.id))?;
            if style
                .font_size
                .is_some_and(|size| !size.is_finite() || size < 0.0)
            {
                return Err(format!("{}: font size must be finite/nonnegative", node.id));
            }
        }
        validate_actions(&node.on_activate)?;
        validate_actions(&node.on_change)?;
        validate_nodes(&node.children)?;
    }
    Ok(())
}

fn validate_expressions(
    nodes: &[NodeDef],
    roots: &std::collections::BTreeSet<String>,
    model: &Value,
) -> Result<(), String> {
    for node in nodes {
        for expr in [
            &node.text,
            &node.value,
            &node.visible,
            &node.enabled,
            &node.min,
            &node.max,
            &node.step,
            &node.options,
            &node.items,
        ]
        .into_iter()
        .flatten()
        {
            super::expression::validate(expr, roots)?;
        }
        if let Some(style) = &node.style {
            for value in style.node.values() {
                super::expression::validate(value, roots)?;
            }
        }
        if let Some(bind) = &node.bind {
            validate_write(bind, model)?;
        }
        validate_action_expressions(&node.on_activate, roots, model)?;
        validate_action_expressions(&node.on_change, roots, model)?;
        let mut roots = roots.clone();
        if node.kind == Widget::Repeat {
            let alias = node.alias.as_ref().unwrap();
            if !roots.insert(alias.clone()) {
                return Err(format!("repeat alias shadows expression root {alias}"));
            }
            roots.insert("index".into());
            super::expression::validate(node.key.as_ref().unwrap(), &roots)?;
        }
        validate_expressions(&node.children, &roots, model)?;
    }
    Ok(())
}
fn validate_write(path: &str, model: &Value) -> Result<(), String> {
    let mut model = model.clone();
    super::expression::set_model(&mut model, path, Value::Null)
}
fn validate_action_expressions(
    actions: &[Action],
    roots: &std::collections::BTreeSet<String>,
    model: &Value,
) -> Result<(), String> {
    for action in actions {
        match action {
            Action::Set { path, value } => {
                validate_write(path, model)?;
                super::expression::validate(value, roots)?;
            }
            Action::Command {
                arguments, save, ..
            }
            | Action::Debug {
                operation: arguments,
                save,
            } => {
                if let Some(path) = save {
                    validate_write(path, model)?;
                }
                super::expression::validate(arguments, roots)?;
            }
            Action::Require { condition, .. } => super::expression::validate(condition, roots)?,
            Action::Wait { .. } => {}
        }
    }
    Ok(())
}

pub(crate) fn validate_model_path(path: &str) -> Result<(), String> {
    if !path.starts_with("model.") || !path[6..].split('.').all(identifier) {
        return Err(format!("invalid writable model path `{path}`"));
    }
    Ok(())
}
fn validate_actions(actions: &[Action]) -> Result<(), String> {
    for action in actions {
        match action {
            Action::Set { path, .. } => validate_model_path(path)?,
            Action::Command {
                save: Some(path), ..
            }
            | Action::Debug {
                save: Some(path), ..
            } => validate_model_path(path)?,
            Action::Wait { seconds } if !seconds.is_finite() || *seconds < 0.0 => {
                return Err("wait duration must be finite/nonnegative".into());
            }
            _ => {}
        }
    }
    Ok(())
}
