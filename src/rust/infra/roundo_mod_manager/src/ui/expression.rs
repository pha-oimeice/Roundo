//! Small data-only expressions. A null source propagates while its first snapshot
//! is pending; missing fields on an available object are errors, not fallback values.

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub struct Context<'a> {
    pub model: &'a Value,
    pub data: &'a Value,
    pub locals: &'a BTreeMap<String, Value>,
    pub event: &'a Value,
    pub result: &'a Value,
    pub error: &'a str,
    pub busy: bool,
}

impl Context<'_> {
    fn path(&self, path: &str) -> Result<Value, String> {
        let mut segments = path.split('.');
        let root = segments.next().unwrap_or_default();
        let mut value = match root {
            "model" => self.model.clone(),
            "data" => self.data.clone(),
            "event" => self.event.clone(),
            "result" => self.result.clone(),
            "error" => Value::String(self.error.into()),
            "busy" => Value::Bool(self.busy),
            name => self
                .locals
                .get(name)
                .cloned()
                .ok_or_else(|| format!("unknown expression root `{name}`"))?,
        };
        for segment in segments {
            if value.is_null() {
                return Ok(Value::Null);
            }
            value = if let Some(object) = value.as_object() {
                object
                    .get(segment)
                    .cloned()
                    .ok_or_else(|| format!("missing expression field `{segment}` in `{path}`"))?
            } else if let Some(array) = value.as_array() {
                let index = segment
                    .parse::<usize>()
                    .map_err(|_| format!("invalid array index `{segment}`"))?;
                array
                    .get(index)
                    .cloned()
                    .ok_or_else(|| format!("array index out of range in `{path}`"))?
            } else {
                return Err(format!("cannot traverse `{path}` through a scalar"));
            };
        }
        Ok(value)
    }

    pub fn eval(&self, expr: &Value) -> Result<Value, String> {
        match expr {
            Value::String(s) if s.starts_with('$') && !s.starts_with("${") => self.path(&s[1..]),
            Value::String(s) if s.contains("${") => {
                let mut out = String::new();
                let mut rest = s.as_str();
                while let Some(start) = rest.find("${") {
                    out.push_str(&rest[..start]);
                    let tail = &rest[start + 2..];
                    let end = tail.find('}').ok_or("unterminated UI interpolation")?;
                    out.push_str(&text(&self.path(&tail[..end])?));
                    rest = &tail[end + 1..];
                }
                out.push_str(rest);
                Ok(Value::String(out))
            }
            Value::Array(array) => array
                .iter()
                .map(|v| self.eval(v))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array),
            Value::Object(object) if object.contains_key("op") => self.operation(object),
            Value::Object(object) => object
                .iter()
                .map(|(k, v)| Ok((k.clone(), self.eval(v)?)))
                .collect::<Result<Map<_, _>, String>>()
                .map(Value::Object),
            value => Ok(value.clone()),
        }
    }

    fn operation(&self, object: &Map<String, Value>) -> Result<Value, String> {
        let op = object["op"].as_str().ok_or("expression op must be text")?;
        let expr = |key: &str| {
            object
                .get(key)
                .ok_or_else(|| format!("{op} requires {key}"))
        };
        let eval = |key: &str| self.eval(expr(key)?);
        match op {
            "null" => Ok(Value::Null),
            "literal" => Ok(expr("value")?.clone()),
            "eq" => Ok(json!(eval("left")? == eval("right")?)),
            "ne" => Ok(json!(eval("left")? != eval("right")?)),
            "not" => Ok(json!(!boolean(&eval("value")?)?)),
            "and" | "or" => {
                let args = expr("values")?
                    .as_array()
                    .ok_or("and/or values must be an array")?;
                for arg in args {
                    let b = boolean(&self.eval(arg)?)?;
                    if op == "and" && !b {
                        return Ok(json!(false));
                    }
                    if op == "or" && b {
                        return Ok(json!(true));
                    }
                }
                Ok(json!(op == "and"))
            }
            "if" => {
                if boolean(&eval("condition")?)? {
                    eval("then")
                } else {
                    eval("else")
                }
            }
            "coalesce" => {
                for value in expr("values")?
                    .as_array()
                    .ok_or("coalesce values must be array")?
                {
                    let value = self.eval(value)?;
                    if !value.is_null() {
                        return Ok(value);
                    }
                }
                Ok(Value::Null)
            }
            "json" => Ok(Value::String(
                serde_json::to_string_pretty(&eval("value")?).map_err(|e| e.to_string())?,
            )),
            "parse-json" => serde_json::from_str(
                eval("value")?
                    .as_str()
                    .ok_or("parse-json value must be text")?,
            )
            .map_err(|e| format!("invalid JSON: {e}")),
            "parse-number" => {
                let value = eval("value")?;
                let value = value
                    .as_str()
                    .ok_or("parse-number value must be text")?
                    .parse::<f64>()
                    .map_err(|_| "请输入有效数字".to_owned())?;
                if !value.is_finite() {
                    return Err("数字必须有限".into());
                }
                Ok(json!(value))
            }
            "length" => {
                let value = eval("value")?;
                Ok(json!(match &value {
                    Value::Null => 0,
                    Value::Array(a) => a.len(),
                    Value::Object(o) => o.len(),
                    Value::String(s) => s.chars().count(),
                    _ => return Err("length requires a collection".into()),
                }))
            }
            "contains" => {
                let value = eval("value")?;
                let item = eval("item")?;
                Ok(json!(match value {
                    Value::Null => false,
                    Value::Array(a) => a.contains(&item),
                    Value::String(s) =>
                        s.contains(item.as_str().ok_or("text contains requires text item")?),
                    _ => return Err("contains requires array or text".into()),
                }))
            }
            "get" => {
                let value = eval("value")?;
                let key = eval("key")?;
                if value.is_null() {
                    return Ok(Value::Null);
                }
                if let Some(object) = value.as_object() {
                    Ok(object
                        .get(key.as_str().ok_or("get key must be text")?)
                        .cloned()
                        .unwrap_or(Value::Null))
                } else {
                    Err("get requires object".into())
                }
            }
            "any" => {
                let items = eval("items")?;
                if items.is_null() {
                    return Ok(json!(false));
                }
                let items = items.as_array().ok_or("any requires an array")?;
                let alias = expr("alias")?.as_str().ok_or("any alias must be text")?;
                for item in items {
                    let mut locals = self.locals.clone();
                    locals.insert(alias.into(), item.clone());
                    let context = Context {
                        locals: &locals,
                        ..*self
                    };
                    if boolean(&context.eval(expr("condition")?)?)? {
                        return Ok(json!(true));
                    }
                }
                Ok(json!(false))
            }
            "options" => {
                let items = eval("items")?;
                let Some(items) = items.as_array() else {
                    return if items.is_null() {
                        Ok(json!([]))
                    } else {
                        Err("options requires array".into())
                    };
                };
                let alias = expr("alias")?
                    .as_str()
                    .ok_or("options alias must be text")?;
                let mut result = Vec::new();
                for item in items {
                    let mut locals = self.locals.clone();
                    locals.insert(alias.into(), item.clone());
                    let context = Context {
                        locals: &locals,
                        ..*self
                    };
                    if let Some(filter) = object.get("filter")
                        && !boolean(&context.eval(filter)?)?
                    {
                        continue;
                    }
                    result.push(json!({"value": context.eval(expr("value")?)?, "label": text(&context.eval(expr("label")?)?)}));
                }
                Ok(json!(result))
            }
            "append" => {
                let mut values = eval("values")?
                    .as_array()
                    .cloned()
                    .ok_or("append values must be array")?;
                values.push(eval("value")?);
                Ok(json!(values))
            }
            "last" | "init" => {
                let value = eval("value")?;
                let values = value.as_array().ok_or("last/init requires array")?;
                if values.is_empty() {
                    return Err("last/init requires a nonempty array".into());
                }
                if op == "last" {
                    Ok(values.last().unwrap().clone())
                } else {
                    Ok(json!(&values[..values.len() - 1]))
                }
            }
            "shortcuts" => {
                let snapshot = eval("bindings")?;
                if snapshot.is_null() {
                    return Ok(json!([]));
                }
                let function = eval("function")?;
                let bindings = snapshot["bindings"]
                    .as_array()
                    .ok_or("bindings snapshot has no bindings")?;
                let slots = snapshot["supported_slots"]
                    .as_array()
                    .ok_or("bindings snapshot has no supported_slots")?;
                let keys = snapshot["supported_keys"]
                    .as_array()
                    .ok_or("bindings snapshot has no supported_keys")?;
                Ok(json!(
                    bindings
                        .iter()
                        .filter(|b| slots.iter().any(|s| s["slot"] == b["slot"]
                            && s["functions"]
                                .as_array()
                                .is_some_and(|f| f.contains(&function))))
                        .map(|b| keys
                            .iter()
                            .find(|k| k["key"] == b["key"])
                            .map(|k| k["display_name"].clone())
                            .unwrap_or_else(|| b["key"].clone()))
                        .collect::<Vec<_>>()
                ))
            }
            _ => Err(format!("unknown UI expression operation `{op}`")),
        }
    }
}

/// Structural validation runs for every branch at resource load, including views
/// not yet opened. Operators cannot hide misspelled fields behind a lazy branch.
pub(crate) fn validate(
    expr: &Value,
    roots: &std::collections::BTreeSet<String>,
) -> Result<(), String> {
    match expr {
        Value::String(s) if s.starts_with('$') && !s.starts_with("${") => {
            validate_path(&s[1..], roots)
        }
        Value::String(s) => {
            let mut rest = s.as_str();
            while let Some(start) = rest.find("${") {
                let tail = &rest[start + 2..];
                let end = tail.find('}').ok_or("unterminated UI interpolation")?;
                validate_path(&tail[..end], roots)?;
                rest = &tail[end + 1..];
            }
            Ok(())
        }
        Value::Array(a) => {
            for v in a {
                validate(v, roots)?;
            }
            Ok(())
        }
        Value::Object(o) if o.contains_key("op") => {
            let op = o["op"].as_str().ok_or("expression op must be text")?;
            let (required, optional): (&[&str], &[&str]) = match op {
                "null" => (&[], &[]),
                "literal" => (&["value"], &[]),
                "eq" | "ne" => (&["left", "right"], &[]),
                "not" | "json" | "parse-json" | "parse-number" | "length" | "last" | "init" => {
                    (&["value"], &[])
                }
                "and" | "or" | "coalesce" => (&["values"], &[]),
                "if" => (&["condition", "then", "else"], &[]),
                "contains" => (&["value", "item"], &[]),
                "get" => (&["value", "key"], &[]),
                "options" => (&["items", "alias", "value", "label"], &["filter"]),
                "any" => (&["items", "alias", "condition"], &[]),
                "append" => (&["values", "value"], &[]),
                "shortcuts" => (&["bindings", "function"], &[]),
                _ => return Err(format!("unknown UI expression operation `{op}`")),
            };
            if let Some(missing) = required.iter().find(|key| !o.contains_key(**key)) {
                return Err(format!("{op} requires {missing}"));
            }
            if let Some(unknown) = o.keys().find(|k| {
                k.as_str() != "op"
                    && !required.contains(&k.as_str())
                    && !optional.contains(&k.as_str())
            }) {
                return Err(format!("{op}: unknown field {unknown}"));
            }
            if op == "literal" {
                return Ok(());
            }
            for (key, value) in o {
                if key == "op" || key == "alias" {
                    continue;
                }
                let mut roots = roots.clone();
                if matches!(op, "options" | "any") && key != "items" {
                    let alias = o["alias"].as_str().ok_or("options alias must be text")?;
                    if !super::schema::identifier(alias) || !roots.insert(alias.into()) {
                        return Err(format!("invalid/shadowed options alias {alias}"));
                    }
                }
                validate(value, &roots)?;
            }
            Ok(())
        }
        Value::Object(o) => {
            for value in o.values() {
                validate(value, roots)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
fn validate_path(path: &str, roots: &std::collections::BTreeSet<String>) -> Result<(), String> {
    let mut parts = path.split('.').peekable();
    let root = parts.next().unwrap_or_default();
    if matches!(root, "data" | "model")
        && let Some(field) = parts.peek()
        && !roots.contains(&format!("{root}.{field}"))
    {
        return Err(format!("undeclared UI expression path `{path}`"));
    }
    if !roots.contains(root) || parts.any(|s| s.is_empty()) {
        return Err(format!("invalid or unknown UI expression path `{path}`"));
    }
    Ok(())
}

pub fn text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        value => value.to_string(),
    }
}
pub fn boolean(value: &Value) -> Result<bool, String> {
    match value {
        Value::Null => Ok(false),
        Value::Bool(b) => Ok(*b),
        _ => Err("expected boolean UI expression".into()),
    }
}
pub fn set_model(model: &mut Value, path: &str, value: Value) -> Result<(), String> {
    super::schema::validate_model_path(path)?;
    let mut segments = path[6..].split('.').peekable();
    let mut target = model;
    while let Some(segment) = segments.next() {
        let object = target
            .as_object_mut()
            .ok_or_else(|| format!("non-object model path `{path}`"))?;
        if segments.peek().is_none() {
            if !object.contains_key(segment) {
                return Err(format!("undeclared model field `{path}`"));
            }
            object.insert(segment.into(), value);
            return Ok(());
        }
        target = object
            .get_mut(segment)
            .ok_or_else(|| format!("undeclared model field `{path}`"))?;
    }
    Err(format!("invalid model path `{path}`"))
}
