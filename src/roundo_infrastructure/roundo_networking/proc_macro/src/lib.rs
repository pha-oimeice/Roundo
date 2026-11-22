//! DataTransferObject derive for structs and enums sent through `roundo_infrastructure::networking`.

use proc_macro::TokenStream;
use proc_macro_crate::{Error, FoundCrate, crate_name};
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::{
    DeriveInput, GenericParam, Lifetime, LifetimeParam, LitStr, parse_macro_input, parse_quote,
    visit_mut::{self, VisitMut},
};

/// Implements the `DataTransferObject` marker and Serde `Serialize` and `Deserialize`
/// for a struct or enum without
/// requiring a direct Serde dependency. Do not also derive those traits on the
/// same type.
///
/// Named, tuple, and unit structs and enum variants are supported. Type
/// parameters receive the corresponding Serde bounds; deserialization requires
/// its input to outlive
/// the struct's lifetime parameters. Fields use the standard Serde encoding,
/// without implicit defaults, skipped fields, or a separate network schema.
/// Owned results can be received by `roundo_infrastructure::networking::Connection` directly.
/// Unions are rejected. Enum variants use Serde's externally tagged encoding;
/// Postcard transmits their declaration-order index followed by their fields,
/// not their Rust discriminant values. Peers must agree on variant order.
#[proc_macro_derive(DataTransferObject)]
pub fn derive_data_transfer_object(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    if matches!(input.data, syn::Data::Union(_)) {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "DataTransferObject requires a struct or enum; unions are not supported",
        ));
    }

    let name = &input.ident;
    let networking = networking_path(name.span())?;
    let serde = quote!(#networking::__serde);
    let serde_path = LitStr::new(&serde.to_string(), name.span());
    let helper = format_ident!("__DataTransferObjectFor{}", name);
    let remote = LitStr::new(&name.to_string(), name.span());
    let (_, ty_generics, _) = input.generics.split_for_impl();
    let mut definition = input.clone();
    // A remote Serde derive accesses the original by reference and constructs it
    // directly on decode. The mirror is never allocated or copied at runtime.
    ReplaceSelf(parse_quote!(#name #ty_generics)).visit_derive_input_mut(&mut definition);
    definition.ident = helper.clone();
    definition.vis = syn::Visibility::Inherited;
    definition.attrs = vec![
        parse_quote!(#[derive(
            #serde::Serialize,
            #serde::Deserialize
        )]),
        parse_quote!(#[serde(
            crate = #serde_path,
            remote = #remote,
            rename = #remote
        )]),
    ];
    // Attributes belonging to unrelated derives must not be copied to the
    // private Serde mirror. DataTransferObject has no field or variant options.
    match &mut definition.data {
        syn::Data::Struct(data) => {
            for field in &mut data.fields {
                field.attrs.clear();
            }
        }
        syn::Data::Enum(data) => {
            for variant in &mut data.variants {
                variant.attrs.clear();
                // Serde identifies variants by declaration order. The mirror
                // needs neither Rust discriminants nor their repr attributes.
                variant.discriminant = None;
                for field in &mut variant.fields {
                    field.attrs.clear();
                }
            }
        }
        syn::Data::Union(_) => unreachable!("unions are rejected before generating the mirror"),
    }

    let serializer = fresh_ident(&input.generics, "__DataTransferObjectSerializer");
    let deserializer = fresh_ident(&input.generics, "__DataTransferObjectDeserializer");
    let de_name = fresh_ident(&input.generics, "__data_transfer_object_de");
    let de = Lifetime::new(&format!("'{de_name}"), Span::call_site());

    let mut serialize_generics = input.generics.clone();
    for parameter in serialize_generics.type_params_mut() {
        parameter.bounds.push(parse_quote!(#serde::Serialize));
    }
    let (ser_impl, _, ser_where) = serialize_generics.split_for_impl();

    let mut deserialize_generics = input.generics.clone();
    for parameter in deserialize_generics.type_params_mut() {
        parameter
            .bounds
            .push(parse_quote!(#serde::Deserialize<#de>));
    }
    let mut de_parameter = LifetimeParam::new(de.clone());
    for parameter in input.generics.lifetimes() {
        de_parameter.bounds.push(parameter.lifetime.clone());
    }
    deserialize_generics
        .params
        .insert(0, GenericParam::Lifetime(de_parameter));
    let (de_impl, _, de_where) = deserialize_generics.split_for_impl();

    Ok(quote! {
        const _: () = {
            #definition

            #[automatically_derived]
            impl #ser_impl #serde::Serialize
                for #name #ty_generics #ser_where
            {
                fn serialize<#serializer>(
                    &self,
                    serializer: #serializer,
                ) -> ::core::result::Result<#serializer::Ok, #serializer::Error>
                where
                    #serializer: #serde::Serializer,
                {
                    #helper::serialize(self, serializer)
                }
            }

            #[automatically_derived]
            impl #ser_impl #networking::DataTransferObject
                for #name #ty_generics #ser_where
            {}

            #[automatically_derived]
            impl #de_impl #serde::Deserialize<#de>
                for #name #ty_generics #de_where
            {
                fn deserialize<#deserializer>(
                    deserializer: #deserializer,
                ) -> ::core::result::Result<Self, #deserializer::Error>
                where
                    #deserializer: #serde::Deserializer<#de>,
                {
                    #helper::deserialize(deserializer)
                }
            }
        };
    })
}

fn networking_path(span: Span) -> syn::Result<syn::Path> {
    match crate_name("roundo_infrastructure") {
        Ok(FoundCrate::Itself) => Ok(parse_quote!(crate::networking)),
        Ok(FoundCrate::Name(name)) => {
            let name = syn::Ident::new_raw(&name, span);
            Ok(parse_quote!(::#name::networking))
        }
        // The networking crate derives DTOs itself and cannot depend on the façade.
        Err(Error::CrateNotFound { .. }) => match crate_name("roundo_networking") {
            Ok(FoundCrate::Itself) => Ok(parse_quote!(::roundo_networking)),
            Ok(FoundCrate::Name(name)) => {
                let name = syn::Ident::new_raw(&name, span);
                Ok(parse_quote!(::#name))
            }
            Err(error) => Err(syn::Error::new(span, error)),
        },
        Err(error) => Err(syn::Error::new(span, error)),
    }
}

fn fresh_ident(generics: &syn::Generics, base: &str) -> syn::Ident {
    let mut name = base.to_owned();
    while generics.params.iter().any(|parameter| match parameter {
        GenericParam::Type(parameter) => parameter.ident == name,
        GenericParam::Const(parameter) => parameter.ident == name,
        GenericParam::Lifetime(parameter) => parameter.lifetime.ident == name,
    }) {
        name.push('_');
    }
    syn::Ident::new(&name, Span::call_site())
}

// `Self` in original fields and bounds must still refer to the original type,
// not to the generated mirror that Serde inspects.
struct ReplaceSelf(syn::Path);

impl VisitMut for ReplaceSelf {
    fn visit_path_mut(&mut self, path: &mut syn::Path) {
        visit_mut::visit_path_mut(self, path);
        if path.leading_colon.is_none()
            && path
                .segments
                .first()
                .is_some_and(|segment| segment.ident == "Self")
        {
            let suffix = path.segments.iter().skip(1).cloned();
            let mut replacement = self.0.clone();
            replacement.segments.extend(suffix);
            *path = replacement;
        }
    }
}
