//! Resource access to Bevy's native Node fields, not a separate layout model.
//! Field names, enums, grid tracks and placement use Bevy's reflected schema.
//! Lengths additionally accept numbers (logical pixels) and Val::from_str syntax;
//! a scalar padding/margin/border uses UiRect::all. Layout solving stays in Bevy.

use bevy::{
    prelude::{Node, UiRect, Val},
    reflect::{
        PartialReflect, ReflectFromReflect, TypeRegistration, TypeRegistry,
        serde::{ReflectDeserializerProcessor, TypedReflectDeserializer},
        structs::Struct,
    },
};
use serde::{Deserialize, de::DeserializeSeed};
use serde_json::Value;
use std::{any::TypeId, collections::BTreeMap, sync::OnceLock};

fn registry() -> &'static TypeRegistry {
    static REGISTRY: OnceLock<TypeRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = TypeRegistry::default();
        registry.register::<Node>();
        registry
    })
}

/// Apply only explicitly supplied fields to the widget's original native Node.
/// Each field is replaced, not reflect-merged: shrinking track lists must remove
/// old tracks, and changing a partial rectangle must not retain the previous one.
/// Unknown fields, wrong types and non-finite lengths are errors.
pub fn resolve(base: &Node, fields: &Value) -> Result<Node, String> {
    let fields = fields
        .as_object()
        .ok_or("native layout must be an object")?;
    let mut node = base.clone();
    for (name, value) in fields {
        set_field(&mut node, name, value)?;
    }
    Ok(node)
}

/// Check native field names even for dynamic values; decode constant values at
/// resource load. Dynamic values are type-checked when evaluated by the view.
pub(crate) fn validate(fields: &BTreeMap<String, Value>) -> Result<(), String> {
    let mut node = Node::default();
    for (name, value) in fields {
        if node.field(name).is_none() {
            return Err(format!("unknown native Node field `{name}`"));
        }
        if constant(value) {
            set_field(&mut node, name, value)?;
        }
    }
    Ok(())
}

fn constant(value: &Value) -> bool {
    match value {
        Value::String(value) => !value.starts_with('$') && !value.contains("${"),
        Value::Array(values) => values.iter().all(constant),
        Value::Object(values) => !values.contains_key("op") && values.values().all(constant),
        _ => true,
    }
}

fn set_field(node: &mut Node, name: &str, value: &Value) -> Result<(), String> {
    let field = node
        .field_mut(name)
        .ok_or_else(|| format!("unknown native Node field `{name}`"))?;
    let info = field
        .get_represented_type_info()
        .ok_or_else(|| format!("native Node field `{name}` has no reflected type"))?;
    let registry = registry();
    let registration = registry
        .get(info.type_id())
        .ok_or_else(|| format!("native Node field `{name}` is not registered"))?;
    let reflected = TypedReflectDeserializer::with_processor(
        registration,
        registry,
        &mut NativeValues {
            scalar_rectangles: true,
        },
    )
    .deserialize(value)
    .map_err(|error| format!("native Node field `{name}`: {error}"))?;
    let concrete = registration
        .data::<ReflectFromReflect>()
        .and_then(|convert| convert.from_reflect(reflected.as_ref()))
        .ok_or_else(|| {
            format!(
                "native Node field `{name}` is not a complete {}",
                info.type_path()
            )
        })?;
    field
        .try_as_reflect_mut()
        .ok_or_else(|| format!("native Node field `{name}` is not writable"))?
        .set(concrete)
        .map_err(|_| format!("native Node field `{name}` has an incompatible type"))
}

struct NativeValues {
    scalar_rectangles: bool,
}
impl ReflectDeserializerProcessor for NativeValues {
    fn try_deserialize<'de, D>(
        &mut self,
        registration: &TypeRegistration,
        registry: &TypeRegistry,
        deserializer: D,
    ) -> Result<Result<Box<dyn PartialReflect>, D>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        if registration.type_id() == TypeId::of::<Val>() {
            let value = Value::deserialize(deserializer)?;
            return length(&value)
                .map(|value| Ok(Box::new(value) as Box<dyn PartialReflect>))
                .map_err(D::Error::custom);
        }
        if self.scalar_rectangles && registration.type_id() == TypeId::of::<UiRect>() {
            let value = Value::deserialize(deserializer)?;
            if value.is_number() || value.is_string() {
                return length(&value)
                    .map(|value| Ok(Box::new(UiRect::all(value)) as Box<dyn PartialReflect>))
                    .map_err(D::Error::custom);
            }
            // Delegate structured rectangles to Bevy, including unknown-field
            // checking. No second rectangle schema or per-edge defaults here.
            let reflected = TypedReflectDeserializer::with_processor(
                registration,
                registry,
                &mut NativeValues {
                    scalar_rectangles: false,
                },
            )
            .deserialize(&value)
            .map_err(D::Error::custom)?;
            return Ok(Ok(reflected));
        }
        if registration.type_id() == TypeId::of::<f32>() {
            let value = f32::deserialize(deserializer)?;
            if !value.is_finite() {
                return Err(D::Error::custom("layout numbers must be finite"));
            }
            return Ok(Ok(Box::new(value)));
        }
        Ok(Err(deserializer))
    }
}

fn length(value: &Value) -> Result<Val, String> {
    let length = match value {
        Value::Number(number) => {
            let value = number.as_f64().ok_or("invalid numeric length")? as f32;
            Val::Px(value)
        }
        Value::String(value) => value
            .parse::<Val>()
            .map_err(|error| format!("invalid length `{value}`: {error}"))?,
        _ => return Err("length must be a number or a Bevy length string".into()),
    };
    match length {
        Val::Auto => Ok(length),
        Val::Px(value)
        | Val::Percent(value)
        | Val::Vw(value)
        | Val::Vh(value)
        | Val::VMin(value)
        | Val::VMax(value)
        | Val::Em(value)
        | Val::Rem(value)
            if value.is_finite() =>
        {
            Ok(length)
        }
        _ => Err("length must be finite".into()),
    }
}
