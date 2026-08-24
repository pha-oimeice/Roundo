//! Proc macros for opt-in Unix command adapters.
use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, LitChar, LitStr, Type, parse_macro_input};

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

struct UnixField {
    positional: bool,
    long: Option<String>,
    short: Option<char>,
    default: Option<String>,
}

fn unix_field(field: &syn::Field) -> Result<UnixField, syn::Error> {
    let mut result = UnixField {
        positional: false,
        long: None,
        short: None,
        default: None,
    };
    for attribute in &field.attrs {
        if !attribute.path().is_ident("unix") {
            continue;
        }
        attribute.parse_nested_meta(|meta| {
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
    Ok(result)
}

/// Generates the Unix text adapter. It only projects tokens to JSON; execution
/// still goes through the source-neutral typed command registry.
#[proc_macro_derive(UnixCommand, attributes(unix, command))]
pub fn derive_unix_command(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;
    let path = input.attrs.iter().find_map(|attribute| {
        if !attribute.path().is_ident("unix") {
            return None;
        }
        let mut path = None;
        let _ = attribute.parse_nested_meta(|meta| {
            if meta.path.is_ident("path") {
                path = Some(meta.value()?.parse::<LitStr>()?);
            }
            Ok(())
        });
        path
    });
    let Some(path) = path else {
        return syn::Error::new_spanned(name, "UnixCommand requires #[unix(path = \"…\")]")
            .to_compile_error()
            .into();
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
    for (index, (field, _)) in positional.iter().enumerate() {
        if generic_inner(&field.ty, "Vec").is_some() && index + 1 != positional.len() {
            return syn::Error::new_spanned(
                &field.ty,
                "a Unix positional Vec<T> must be the final positional field",
            )
            .to_compile_error()
            .into();
        }
        if generic_inner(&field.ty, "Option").is_some()
            && positional.iter().skip(index + 1).any(|(later, _)| {
                generic_inner(&later.ty, "Option").is_none()
                    && generic_inner(&later.ty, "Vec").is_none()
            })
        {
            return syn::Error::new_spanned(
                &field.ty,
                "Unix positional Option<T> fields must form a trailing suffix",
            )
            .to_compile_error()
            .into();
        }
    }
    let initializers = fields.iter().zip(&specs).filter_map(|(field, spec)| {
        let ident = field.ident.as_ref().unwrap().to_string();
        let key = LitStr::new(&ident, field.ident.as_ref().unwrap().span());
        if is_bool(&field.ty) {
            Some(quote! { values.insert(#key.to_string(), ::serde_json::Value::Bool(false)); })
        } else if generic_inner(&field.ty, "Vec").is_some() {
            Some(quote! { values.insert(#key.to_string(), ::serde_json::json!([])); })
        } else if let Some(default) = &spec.default {
            let default = LitStr::new(default, field.ident.as_ref().unwrap().span());
            let ty = &field.ty;
            Some(quote! { values.insert(#key.to_string(), ::serde_json::to_value(#default.parse::<#ty>().map_err(|_| ::roundo_cli::UnixCommandParseError::new(concat!("invalid default for ", #key)))?).map_err(|error| ::roundo_cli::UnixCommandParseError::new(error.to_string()))?); })
        } else { None }
    });
    let option_arms = fields.iter().zip(&specs).filter(|(_, spec)| !spec.positional).flat_map(|(field, spec)| {
        let key = LitStr::new(&field.ident.as_ref().unwrap().to_string(), field.ident.as_ref().unwrap().span());
        let ty = &field.ty;
        let parse = if is_bool(ty) {
            quote! { values.insert(#key.to_string(), ::serde_json::Value::Bool(true)); }
        } else if let Some(inner) = generic_inner(ty, "Vec") {
            quote! { let value = arguments.get(index + 1).ok_or_else(|| ::roundo_cli::UnixCommandParseError::new(format!("missing value for {}", token)))?.parse::<#inner>().map_err(|_| ::roundo_cli::UnixCommandParseError::new(format!("invalid value for {}", token)))?; let entry = values.entry(#key.to_string()).or_insert_with(|| ::serde_json::json!([])); entry.as_array_mut().unwrap().push(::serde_json::to_value(value).map_err(|error| ::roundo_cli::UnixCommandParseError::new(error.to_string()))?); index += 1; }
        } else if let Some(inner) = generic_inner(ty, "Option") {
            quote! { let value = arguments.get(index + 1).ok_or_else(|| ::roundo_cli::UnixCommandParseError::new(format!("missing value for {}", token)))?.parse::<#inner>().map_err(|_| ::roundo_cli::UnixCommandParseError::new(format!("invalid value for {}", token)))?; values.insert(#key.to_string(), ::serde_json::to_value(value).map_err(|error| ::roundo_cli::UnixCommandParseError::new(error.to_string()))?); index += 1; }
        } else {
            quote! { let value = arguments.get(index + 1).ok_or_else(|| ::roundo_cli::UnixCommandParseError::new(format!("missing value for {}", token)))?.parse::<#ty>().map_err(|_| ::roundo_cli::UnixCommandParseError::new(format!("invalid value for {}", token)))?; values.insert(#key.to_string(), ::serde_json::to_value(value).map_err(|error| ::roundo_cli::UnixCommandParseError::new(error.to_string()))?); index += 1; }
        };
        let mut arms = Vec::new();
        if let Some(long) = &spec.long { let token = LitStr::new(&format!("--{long}"), field.ident.as_ref().unwrap().span()); arms.push(quote! { #token => { #parse } }); }
        if let Some(short) = spec.short { let token = LitStr::new(&format!("-{short}"), field.ident.as_ref().unwrap().span()); arms.push(quote! { #token => { #parse } }); }
        arms
    });
    let positional_assignments = positional.iter().enumerate().map(|(index, (field, _))| {
        let key = LitStr::new(&field.ident.as_ref().unwrap().to_string(), field.ident.as_ref().unwrap().span());
        let ty = &field.ty;
        if let Some(inner) = generic_inner(ty, "Vec") {
            quote! { let parsed = positional.iter().skip(#index).map(|value| value.parse::<#inner>().map_err(|_| ::roundo_cli::UnixCommandParseError::new(format!("invalid argument {}", #index + 1)))).collect::<Result<Vec<_>, _>>()?; values.insert(#key.to_string(), ::serde_json::to_value(parsed).map_err(|error| ::roundo_cli::UnixCommandParseError::new(error.to_string()))?); }
        } else if let Some(inner) = generic_inner(ty, "Option") {
            quote! { if let Some(value) = positional.get(#index) { values.insert(#key.to_string(), ::serde_json::to_value(value.parse::<#inner>().map_err(|_| ::roundo_cli::UnixCommandParseError::new(format!("invalid argument {}", #index + 1)))?).map_err(|error| ::roundo_cli::UnixCommandParseError::new(error.to_string()))?); } }
        } else {
            quote! { let value = positional.get(#index).ok_or_else(|| ::roundo_cli::UnixCommandParseError::new(format!("missing argument {}", #index + 1)))?.parse::<#ty>().map_err(|_| ::roundo_cli::UnixCommandParseError::new(format!("invalid argument {}", #index + 1)))?; values.insert(#key.to_string(), ::serde_json::to_value(value).map_err(|error| ::roundo_cli::UnixCommandParseError::new(error.to_string()))?); }
        }
    });
    let max_positionals = if positional
        .iter()
        .any(|(field, _)| generic_inner(&field.ty, "Vec").is_some())
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
            usage.push_str(&format!(" <{field_name}>"));
        } else if let Some(long) = &spec.long {
            if is_bool(&field.ty) {
                usage.push_str(&format!(" [--{long}]"));
            } else {
                usage.push_str(&format!(" [--{long} <{field_name}>]"));
            }
        } else if let Some(short) = spec.short {
            usage.push_str(&format!(" [-{short}]"));
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
