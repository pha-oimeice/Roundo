//! Derives for command parsing and structural wire-message metadata.
//!
//! Wire derives generate names, classification, and envelope wrapping only.
//! The command parser handles token classification and scalar `FromStr`
//! conversion only. Shell expansion, quoting, and escaping remain the terminal
//! tokenizer's responsibility.

use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, LitChar, LitStr, Type, parse_macro_input};

mod wire;

/// Generates diagnostic `kind()`, `ApplicationMessage::STREAM`, and envelope
/// conversion for an application enum owned by `roundo_contracts`.
/// Requires `#[wire(envelope = Envelope::Variant, stream = StreamId::Variant)]`.
/// Does not alter the enum, its Serde representation, or transport behavior.
#[proc_macro_derive(WireMessage, attributes(wire))]
pub fn derive_wire_message(input: TokenStream) -> TokenStream {
    wire::message(parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Generates `kind()`, `envelope_kind()`, and `stream()` for a contracts envelope.
/// Every variant must explicitly declare `#[wire(application)]` (one unnamed
/// payload with WireMessage metadata) or `#[wire(negotiation)]` (no application
/// stream). No dispatch, validation, serialization, or I/O code is generated.
#[proc_macro_derive(WireEnvelope, attributes(wire))]
pub fn derive_wire_envelope(input: TokenStream) -> TokenStream {
    wire::envelope(parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

// Recognizes the final path segment of simple Option<T>/Vec<T>-style field types.
fn generic_inner<'a>(ty: &'a Type, container: &str) -> Option<&'a Type> {
    let Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != container {
        return None;
    }
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    arguments.args.iter().find_map(|argument| match argument {
        syn::GenericArgument::Type(inner) => Some(inner),
        _ => None,
    })
}

fn is_bool(ty: &Type) -> bool {
    matches!(ty, Type::Path(path) if path.path.is_ident("bool"))
}

/// Field conversion kind, analyzed once and shared by all generated projections.
#[derive(Clone, Copy)]
enum UnixFieldKind<'a> {
    Scalar(&'a Type),
    Flag,
    Optional(&'a Type),
    Repeated(&'a Type),
}

/// Parsed command-line projection metadata for one named struct field.
struct UnixField<'a> {
    kind: UnixFieldKind<'a>,
    omittable: bool,
    positional: bool,
    long: Option<String>,
    short: Option<char>,
    default: Option<String>,
}

// Defaults unannotated fields to positional arguments for backward compatibility.
fn unix_field(field: &syn::Field) -> Result<UnixField<'_>, syn::Error> {
    let mut result = UnixField {
        kind: UnixFieldKind::Scalar(&field.ty),
        omittable: false,
        positional: false,
        long: None,
        short: None,
        default: None,
    };
    let mut seen = std::collections::BTreeSet::new();
    for attribute in &field.attrs {
        if !attribute.path().is_ident("unix") {
            continue;
        }
        attribute.parse_nested_meta(|meta| {
            if let Some(key) = meta.path.get_ident() {
                if !seen.insert(key.to_string()) {
                    return Err(meta.error("duplicate #[unix(...)] field key"));
                }
            }
            if meta.path.is_ident("positional") {
                result.positional = true;
                return Ok(());
            }
            if meta.path.is_ident("long") {
                if meta.input.peek(syn::Token![=]) {
                    result.long = Some(meta.value()?.parse::<LitStr>()?.value());
                } else {
                    result.long = Some(field.ident.as_ref().unwrap().to_string().replace('_', "-"));
                }
                return Ok(());
            }
            if meta.path.is_ident("short") {
                result.short = Some(meta.value()?.parse::<LitChar>()?.value());
                return Ok(());
            }
            if meta.path.is_ident("default") {
                result.default = Some(meta.value()?.parse::<LitStr>()?.value());
                return Ok(());
            }
            Err(meta.error("unsupported #[unix(...)] option"))
        })?;
    }
    // Retain the original positional-only derive as a compatible shorthand.
    if !result.positional && result.long.is_none() && result.short.is_none() {
        result.positional = true;
    }
    if result.positional && (result.long.is_some() || result.short.is_some()) {
        return Err(syn::Error::new_spanned(
            field,
            "a Unix field cannot be both positional and an option",
        ));
    }
    result.kind = if let Some(inner) = generic_inner(&field.ty, "Option") {
        UnixFieldKind::Optional(inner)
    } else if let Some(inner) = generic_inner(&field.ty, "Vec") {
        UnixFieldKind::Repeated(inner)
    } else if !result.positional && is_bool(&field.ty) {
        UnixFieldKind::Flag
    } else {
        UnixFieldKind::Scalar(&field.ty)
    };
    if result.default.is_some()
        && matches!(
            result.kind,
            UnixFieldKind::Optional(_) | UnixFieldKind::Repeated(_)
        )
    {
        return Err(syn::Error::new_spanned(
            &field.ty,
            "Unix default is not supported for Option<T> or Vec<T> fields",
        ));
    }
    result.omittable = result.default.is_some() || !matches!(result.kind, UnixFieldKind::Scalar(_));
    Ok(result)
}

fn unix_path(attributes: &[syn::Attribute], name: &syn::Ident) -> Result<LitStr, syn::Error> {
    let mut path = None;
    for attribute in attributes {
        if !attribute.path().is_ident("unix") {
            continue;
        }
        attribute.parse_nested_meta(|meta| {
            if !meta.path.is_ident("path") {
                return Err(meta.error("unsupported #[unix(...)] struct key"));
            }
            if path.is_some() {
                return Err(meta.error("duplicate Unix command path"));
            }
            path = Some(meta.value()?.parse::<LitStr>()?);
            Ok(())
        })?;
    }
    path.ok_or_else(|| syn::Error::new_spanned(name, "UnixCommand requires #[unix(path = \"…\")]"))
}

/// Derives `roundo_cli::UnixCommand` for a named-field command struct.
///
/// The struct requires `#[unix(path = "command path")]`. Fields are positional
/// by default and may instead declare `#[unix(long)]`,
/// `#[unix(long = "name")]`, `#[unix(short = 'x')]`, or both long and short
/// option spellings. `#[unix(default = "...")]` parses a scalar default through
/// `FromStr` at runtime.
///
/// Exact `bool` options are flags initialized from their default or `false`;
/// encountering either spelling sets them to `true`. Positional bools are scalars.
/// `Vec<T>` options append on repetition, while
/// repeated scalar options keep the last value. Options and positionals may be
/// interleaved, but this is intentionally not a shell parser.
///
/// Omissible positional fields cannot precede required scalars, and positional
/// `Vec<T>` must be last. Defaults are not supported on `Option<T>` or `Vec<T>`. Tuple/unit structs, enums, empty command paths,
/// positional/option conflicts, and unsupported `#[unix(...)]` keys produce
/// compile errors. Final construction passes through Serde JSON, so missing
/// required option fields and field-level Serde validation remain observable as
/// runtime parse errors.
///
/// The macro only projects tokens to JSON; command execution still goes through
/// the source-neutral typed registry.
#[proc_macro_derive(UnixCommand, attributes(unix, command))]
pub fn derive_unix_command(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;
    let path = match unix_path(&input.attrs, &name) {
        Ok(path) => path,
        Err(error) => return error.to_compile_error().into(),
    };
    let path_parts: Vec<_> = path
        .value()
        .split_whitespace()
        .map(|part| LitStr::new(part, path.span()))
        .collect();
    if path_parts.is_empty() {
        return syn::Error::new_spanned(path, "UnixCommand path cannot be empty")
            .to_compile_error()
            .into();
    }
    let fields = match input.data {
        syn::Data::Struct(data) => match data.fields {
            syn::Fields::Named(fields) => fields.named,
            _ => {
                return syn::Error::new_spanned(name, "UnixCommand requires named fields")
                    .to_compile_error()
                    .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(name, "UnixCommand requires a struct")
                .to_compile_error()
                .into();
        }
    };
    let specs: Result<Vec<_>, _> = fields.iter().map(unix_field).collect();
    let specs = match specs {
        Ok(specs) => specs,
        Err(error) => return error.to_compile_error().into(),
    };
    let positional: Vec<_> = fields
        .iter()
        .zip(&specs)
        .filter(|(_, spec)| spec.positional)
        .collect();
    for (index, (field, spec)) in positional.iter().enumerate() {
        if matches!(spec.kind, UnixFieldKind::Repeated(_)) && index + 1 != positional.len() {
            return syn::Error::new_spanned(
                &field.ty,
                "a Unix positional Vec<T> must be the final positional field",
            )
            .to_compile_error()
            .into();
        }
        if spec.omittable
            && positional
                .iter()
                .skip(index + 1)
                .any(|(_, later)| !later.omittable)
        {
            let message = if matches!(spec.kind, UnixFieldKind::Optional(_)) {
                "Unix positional Option<T> fields must form a trailing suffix"
            } else {
                "an omissible Unix positional field cannot precede a required scalar"
            };
            return syn::Error::new_spanned(&field.ty, message)
                .to_compile_error()
                .into();
        }
    }
    let initializers = fields.iter().zip(&specs).filter_map(|(field, spec)| {
        let ident = field.ident.as_ref().unwrap().to_string();
        let key = LitStr::new(&ident, field.ident.as_ref().unwrap().span());
        if let Some(default) = &spec.default {
            let default = LitStr::new(default, field.ident.as_ref().unwrap().span());
            let ty = &field.ty;
            Some(quote! { values.insert(#key.to_string(), ::serde_json::to_value(#default.parse::<#ty>().map_err(|_| ::roundo_cli::UnixCommandParseError::new(concat!("invalid default for ", #key)))?).map_err(|error| ::roundo_cli::UnixCommandParseError::new(error.to_string()))?); })
        } else {
            match spec.kind {
                UnixFieldKind::Flag => Some(quote! { values.insert(#key.to_string(), ::serde_json::Value::Bool(false)); }),
                UnixFieldKind::Repeated(_) => Some(quote! { values.insert(#key.to_string(), ::serde_json::json!([])); }),
                UnixFieldKind::Optional(_) => Some(quote! { values.insert(#key.to_string(), ::serde_json::Value::Null); }),
                UnixFieldKind::Scalar(_) => None,
            }
        }
    });
    let option_arms = fields.iter().zip(&specs).filter(|(_, spec)| !spec.positional).flat_map(|(field, spec)| {
        let key = LitStr::new(&field.ident.as_ref().unwrap().to_string(), field.ident.as_ref().unwrap().span());
        let parse = match spec.kind {
            UnixFieldKind::Flag => quote! { values.insert(#key.to_string(), ::serde_json::Value::Bool(true)); },
            UnixFieldKind::Repeated(inner) => {
                quote! { let value = arguments.get(index + 1).ok_or_else(|| ::roundo_cli::UnixCommandParseError::new(format!("missing value for {}", token)))?.parse::<#inner>().map_err(|_| ::roundo_cli::UnixCommandParseError::new(format!("invalid value for {}", token)))?; let entry = values.entry(#key.to_string()).or_insert_with(|| ::serde_json::json!([])); entry.as_array_mut().unwrap().push(::serde_json::to_value(value).map_err(|error| ::roundo_cli::UnixCommandParseError::new(error.to_string()))?); index += 1; }
            }
            UnixFieldKind::Optional(ty) | UnixFieldKind::Scalar(ty) => {
                quote! { let value = arguments.get(index + 1).ok_or_else(|| ::roundo_cli::UnixCommandParseError::new(format!("missing value for {}", token)))?.parse::<#ty>().map_err(|_| ::roundo_cli::UnixCommandParseError::new(format!("invalid value for {}", token)))?; values.insert(#key.to_string(), ::serde_json::to_value(value).map_err(|error| ::roundo_cli::UnixCommandParseError::new(error.to_string()))?); index += 1; }
            }
        };
        let mut arms = Vec::new();
        if let Some(long) = &spec.long { let token = LitStr::new(&format!("--{long}"), field.ident.as_ref().unwrap().span()); arms.push(quote! { #token => { #parse } }); }
        if let Some(short) = spec.short { let token = LitStr::new(&format!("-{short}"), field.ident.as_ref().unwrap().span()); arms.push(quote! { #token => { #parse } }); }
        arms
    });
    let positional_assignments = positional.iter().enumerate().map(|(index, (field, spec))| {
        let key = LitStr::new(&field.ident.as_ref().unwrap().to_string(), field.ident.as_ref().unwrap().span());
        match spec.kind {
            UnixFieldKind::Repeated(inner) => {
                quote! { let parsed = positional.iter().skip(#index).map(|value| value.parse::<#inner>().map_err(|_| ::roundo_cli::UnixCommandParseError::new(format!("invalid argument {}", #index + 1)))).collect::<Result<Vec<_>, _>>()?; values.insert(#key.to_string(), ::serde_json::to_value(parsed).map_err(|error| ::roundo_cli::UnixCommandParseError::new(error.to_string()))?); }
            }
            UnixFieldKind::Optional(ty) => {
                quote! { if let Some(value) = positional.get(#index) { values.insert(#key.to_string(), ::serde_json::to_value(value.parse::<#ty>().map_err(|_| ::roundo_cli::UnixCommandParseError::new(format!("invalid argument {}", #index + 1)))?).map_err(|error| ::roundo_cli::UnixCommandParseError::new(error.to_string()))?); } }
            }
            UnixFieldKind::Scalar(ty) if spec.omittable => {
                quote! { if let Some(value) = positional.get(#index) { values.insert(#key.to_string(), ::serde_json::to_value(value.parse::<#ty>().map_err(|_| ::roundo_cli::UnixCommandParseError::new(format!("invalid argument {}", #index + 1)))?).map_err(|error| ::roundo_cli::UnixCommandParseError::new(error.to_string()))?); } }
            }
            UnixFieldKind::Scalar(ty) => {
                quote! { let value = positional.get(#index).ok_or_else(|| ::roundo_cli::UnixCommandParseError::new(format!("missing argument {}", #index + 1)))?.parse::<#ty>().map_err(|_| ::roundo_cli::UnixCommandParseError::new(format!("invalid argument {}", #index + 1)))?; values.insert(#key.to_string(), ::serde_json::to_value(value).map_err(|error| ::roundo_cli::UnixCommandParseError::new(error.to_string()))?); }
            }
            UnixFieldKind::Flag => unreachable!("flags are options"),
        }
    });
    let max_positionals = if positional
        .iter()
        .any(|(_, spec)| matches!(spec.kind, UnixFieldKind::Repeated(_)))
    {
        None
    } else {
        Some(positional.len())
    };
    let positional_limit = max_positionals.map(|max| quote! { if positional.len() > #max { return Err(::roundo_cli::UnixCommandParseError::new(format!("expected at most {} positional arguments", #max))); } });
    let mut usage = path.value();
    for (field, spec) in fields.iter().zip(&specs) {
        let field_name = field.ident.as_ref().unwrap();
        if spec.positional {
            if matches!(spec.kind, UnixFieldKind::Repeated(_)) {
                usage.push_str(&format!(" [{field_name}...]"));
            } else if spec.omittable {
                usage.push_str(&format!(" [{field_name}]"));
            } else {
                usage.push_str(&format!(" <{field_name}>"));
            }
        } else if let Some(long) = &spec.long {
            if matches!(spec.kind, UnixFieldKind::Flag) {
                usage.push_str(&format!(" [--{long}]"));
            } else {
                usage.push_str(&format!(" [--{long} <{field_name}>]"));
            }
        } else if let Some(short) = spec.short {
            if matches!(spec.kind, UnixFieldKind::Flag) {
                usage.push_str(&format!(" [-{short}]"));
            } else {
                usage.push_str(&format!(" [-{short} <{field_name}>]"));
            }
        }
    }
    let usage = LitStr::new(&usage, path.span());
    quote! {
        impl ::roundo_cli::UnixCommand for #name {
            const UNIX_PATH: &'static [&'static str] = &[#(#path_parts),*];
            fn parse_unix(arguments: &[String]) -> Result<::serde_json::Value, ::roundo_cli::UnixCommandParseError> {
                let mut values = ::serde_json::Map::new();
                #(#initializers)*
                let mut positional = Vec::new();
                let mut index = 0usize;
                while index < arguments.len() {
                    let token = &arguments[index];
                    if token.starts_with('-') {
                        match token.as_str() { #(#option_arms)* _ => return Err(::roundo_cli::UnixCommandParseError::new(format!("unknown option {}", token))), }
                    } else { positional.push(token.clone()); }
                    index += 1;
                }
                #positional_limit
                #(#positional_assignments)*
                ::serde_json::from_value::<Self>(::serde_json::Value::Object(values)).map_err(|error| ::roundo_cli::UnixCommandParseError::new(error.to_string()))
                    .and_then(|value| ::serde_json::to_value(value).map_err(|error| ::roundo_cli::UnixCommandParseError::new(error.to_string())))
            }
            fn usage() -> String {
                #usage.to_string()
            }
        }
    }.into()
}
