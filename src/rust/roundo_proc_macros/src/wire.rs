//! Structural wire metadata only; no serialization, dispatch, or transport policy.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Data, DeriveInput, Fields, Path, Result, Variant};

fn variants(input: &DeriveInput) -> Result<&syn::punctuated::Punctuated<Variant, syn::Token![,]>> {
    match &input.data {
        Data::Enum(data) => Ok(&data.variants),
        _ => Err(syn::Error::new_spanned(
            input,
            "wire metadata requires an enum",
        )),
    }
}

fn pattern(variant: &Variant) -> TokenStream {
    let name = &variant.ident;
    match &variant.fields {
        Fields::Unit => quote!(Self::#name),
        Fields::Unnamed(_) => quote!(Self::#name(..)),
        Fields::Named(_) => quote!(Self::#name { .. }),
    }
}

fn reject_wire_attributes(attributes: &[Attribute]) -> Result<()> {
    for attribute in attributes {
        if attribute.path().is_ident("wire") {
            return Err(syn::Error::new_spanned(
                attribute,
                "unexpected #[wire(...)] attribute",
            ));
        }
    }
    Ok(())
}

pub(super) fn message(input: DeriveInput) -> Result<TokenStream> {
    let variants = variants(&input)?;
    let mut envelope: Option<Path> = None;
    let mut stream: Option<Path> = None;
    for attribute in &input.attrs {
        if !attribute.path().is_ident("wire") {
            continue;
        }
        attribute.parse_nested_meta(|meta| {
            let target = if meta.path.is_ident("envelope") {
                &mut envelope
            } else if meta.path.is_ident("stream") {
                &mut stream
            } else {
                return Err(meta.error("expected envelope or stream"));
            };
            if target.is_some() {
                return Err(meta.error("duplicate wire metadata key"));
            }
            *target = Some(meta.value()?.parse()?);
            Ok(())
        })?;
    }
    let envelope = envelope.ok_or_else(|| {
        syn::Error::new_spanned(
            &input.ident,
            "WireMessage requires #[wire(envelope = Enum::Variant)]",
        )
    })?;
    let stream = stream.ok_or_else(|| {
        syn::Error::new_spanned(
            &input.ident,
            "WireMessage requires #[wire(stream = StreamId::Variant)]",
        )
    })?;
    let mut envelope_type = envelope.clone();
    if envelope_type.segments.len() < 2
        || !matches!(
            envelope_type.segments.last().unwrap().arguments,
            syn::PathArguments::None
        )
    {
        return Err(syn::Error::new_spanned(
            &envelope,
            "expected an envelope enum variant path",
        ));
    }
    envelope_type.segments.pop();
    envelope_type.segments.pop_punct();
    let mut names = Vec::new();
    for variant in variants {
        reject_wire_attributes(&variant.attrs)?;
        for field in &variant.fields {
            reject_wire_attributes(&field.attrs)?;
        }
        let pattern = pattern(variant);
        let name = variant.ident.to_string();
        names.push(quote!(#pattern => #name));
    }
    let name = &input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics #name #type_generics #where_clause {
            /// Returns the application variant name for diagnostics, not a wire ID.
            pub const fn kind(&self) -> &'static str {
                match self { #(#names,)* }
            }
        }

        impl #impl_generics ::roundo_contracts::ApplicationMessage for #name #type_generics #where_clause {
            const STREAM: ::roundo_contracts::StreamId = #stream;
        }

        impl #impl_generics ::core::convert::From<#name #type_generics> for #envelope_type #where_clause {
            fn from(message: #name #type_generics) -> Self {
                #envelope(message)
            }
        }
    })
}

pub(super) fn envelope(input: DeriveInput) -> Result<TokenStream> {
    let variants = variants(&input)?;
    reject_wire_attributes(&input.attrs)?;
    let mut names = Vec::new();
    let mut kinds = Vec::new();
    let mut streams = Vec::new();
    for variant in variants {
        let mut application = None;
        for attribute in &variant.attrs {
            if !attribute.path().is_ident("wire") {
                continue;
            }
            attribute.parse_nested_meta(|meta| {
                let value = if meta.path.is_ident("application") {
                    true
                } else if meta.path.is_ident("negotiation") {
                    false
                } else {
                    return Err(meta.error("expected application or negotiation"));
                };
                if application.replace(value).is_some() {
                    return Err(meta
                        .error("each envelope variant requires exactly one wire classification"));
                }
                Ok(())
            })?;
        }
        for field in &variant.fields {
            reject_wire_attributes(&field.attrs)?;
        }
        let application = application.ok_or_else(|| {
            syn::Error::new_spanned(
                variant,
                "envelope variant requires #[wire(application)] or #[wire(negotiation)]",
            )
        })?;
        let pattern = pattern(variant);
        let label = variant.ident.to_string();
        names.push(quote!(#pattern => #label));
        if application {
            let Fields::Unnamed(fields) = &variant.fields else {
                return Err(syn::Error::new_spanned(
                    variant,
                    "application envelope variant requires one unnamed field",
                ));
            };
            if fields.unnamed.len() != 1 {
                return Err(syn::Error::new_spanned(
                    fields,
                    "application envelope variant requires one unnamed field",
                ));
            }
            let ty = &fields.unnamed[0].ty;
            let name = &variant.ident;
            kinds.push(quote!(Self::#name(message) => message.kind()));
            streams.push(quote!(Self::#name(_) => ::core::option::Option::Some(
                <#ty as ::roundo_contracts::ApplicationMessage>::STREAM
            )));
        } else {
            kinds.push(quote!(#pattern => #label));
            streams.push(quote!(#pattern => ::core::option::Option::None));
        }
    }
    let name = &input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics #name #type_generics #where_clause {
            /// Returns the outer variant name, including during negotiation errors.
            pub const fn envelope_kind(&self) -> &'static str {
                match self { #(#names,)* }
            }

            /// Returns the inner application name or the negotiation variant name.
            pub const fn kind(&self) -> &'static str {
                match self { #(#kinds,)* }
            }

            /// Returns the application stream; negotiation is not sendable through application queues.
            pub const fn stream(&self) -> ::core::option::Option<::roundo_contracts::StreamId> {
                match self { #(#streams,)* }
            }
        }
    })
}
