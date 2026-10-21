// Copyright 2025 The Chromium Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This crate defines proc-macros for deriving the `MojomParse` trait (and
//! `PrimitiveEnum`, which also derives `MojomParse`).
//!
//! The behavior of the proc macros is conceptually very simple: we require
//! every field to implement `MojomParse` independently, and then just invoke
//! those implementations for each field.
//!
//! These macros are necessary because Rust does not support reflection; in
//! order to iterate over the fields of a struct, we need to use compile-time
//! syntax processing (that is, a proc-macro).
//!
//! When deriving `PrimitiveEnum`, we instead implement traits that allow easy
//! conversion between the enum and `i32`, then use those to implement
//! `MojomParse`.

use quote::quote;
use syn::{DeriveInput, parse_macro_input};

#[proc_macro_derive(MojomParse, attributes(mojom))]
pub fn derive_mojomparse(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;
    let attributes = match MojomAttributes::parse(&input.attrs) {
        Ok(attributes) => attributes,
        Err(error) => return error.into_compile_error().into(),
    };
    let imports = match attributes.imports(true) {
        Ok(imports) => imports,
        Err(error) => return error.into_compile_error().into(),
    };

    let derived_tokens = match input.data {
        syn::Data::Struct(syn::DataStruct { fields, .. }) => match fields {
            syn::Fields::Named(syn::FieldsNamed { named, .. }) => {
                derive_mojomparse_struct(name.clone(), named, imports)
            }
            _ => panic!("Mojom structs do not support unnamed fields"),
        },
        syn::Data::Enum(syn::DataEnum { variants, .. }) => {
            derive_mojomparse_union(name.clone(), variants, imports)
        }
        syn::Data::Union(_) => {
            panic!("Mojom does not support untagged unions. Use a Rust enum instead.")
        }
    };

    return proc_macro::TokenStream::from(derived_tokens);
}

/// Cargo adapter attributes. `runtime` points to the public SDK dependency,
/// including a renamed dependency such as `::ipc`.
#[derive(Default)]
struct MojomAttributes {
    parse_as: Option<String>,
    in_bindings_crate: bool,
    runtime: Option<syn::Path>,
    core_path: Option<syn::Path>,
    bindings_path: Option<syn::Path>,
}

fn dependency_path(package: &str) -> syn::Result<syn::Path> {
    match proc_macro_crate::crate_name(package) {
        Ok(proc_macro_crate::FoundCrate::Itself) => Ok(syn::parse_quote!(crate)),
        Ok(proc_macro_crate::FoundCrate::Name(name)) => {
            let identifier =
                syn::Ident::new(&name.replace('-', "_"), proc_macro2::Span::call_site());
            Ok(syn::parse_quote!(::#identifier))
        }
        Err(error) => Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            format!(
                "cannot locate {package}: {error}; add #[mojom(runtime = \"::your_sdk_alias\")]"
            ),
        )),
    }
}

impl MojomAttributes {
    fn parse(attrs: &[syn::Attribute]) -> syn::Result<Self> {
        let mut result = Self::default();
        for attr in attrs {
            if !attr.path().is_ident("mojom") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("in_bindings_crate") {
                    result.in_bindings_crate = true;
                    return Ok(());
                }
                let value: syn::LitStr = meta.value()?.parse()?;
                if meta.path.is_ident("parse_as") {
                    result.parse_as = Some(value.value());
                } else if meta.path.is_ident("runtime") {
                    result.runtime = Some(value.parse()?);
                } else if meta.path.is_ident("core_path") || meta.path.is_ident("codec_crate") {
                    result.core_path = Some(value.parse()?);
                } else if meta.path.is_ident("bindings_path")
                    || meta.path.is_ident("bindings_crate")
                {
                    result.bindings_path = Some(value.parse()?);
                } else {
                    return Err(meta.error("unknown mojom attribute"));
                }
                Ok(())
            })?;
        }
        Ok(result)
    }

    fn imports(&self, needs_registrar: bool) -> syn::Result<proc_macro2::TokenStream> {
        let core = if let Some(path) = &self.core_path {
            quote!(#path)
        } else if let Some(runtime) = &self.runtime {
            quote!(#runtime::__private::mojom_value_parser_core)
        } else {
            let path = dependency_path("mojom-codec-core")?;
            quote!(#path)
        };
        let registrar = if !needs_registrar {
            quote!()
        } else if self.in_bindings_crate {
            quote!(
                use crate::interface::Registrar as __MojomRegistrar;
            )
        } else {
            let bindings = if let Some(path) = &self.bindings_path {
                quote!(#path)
            } else if let Some(runtime) = &self.runtime {
                quote!(#runtime::__private::bindings)
            } else {
                let path = dependency_path("mojo-bindings")?;
                quote!(#path)
            };
            quote!(use #bindings::interface::Registrar as __MojomRegistrar;)
        };
        Ok(quote! {
            use #core as __mojom_core;
            use __mojom_core::__private::anyhow as __mojom_anyhow;
            #registrar
        })
    }
}

fn field_wire_type(attrs: &[syn::Attribute], default_ty: &syn::Type) -> syn::Type {
    match MojomAttributes::parse(attrs)
        .expect("invalid mojom field attributes")
        .parse_as
    {
        Some(parse_as) => syn::parse_str(&parse_as).expect("invalid parse_as type"),
        None => default_ty.clone(),
    }
}

fn derive_mojomparse_struct(
    name: syn::Ident,
    struct_fields: syn::punctuated::Punctuated<syn::Field, syn::Token![,]>,
    imports: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let num_fields = struct_fields.len();

    // As far as I know, quote can only iterate over vectors of things that can
    // be directly converted to tokens. Notably, this means they have to be
    // single values. So if we want to write something like #name = #val, in a
    // loop, we first have to combine each pair of names and values into a
    // single token stream, and then can we iterate over that in the quote.

    // The names of the fields in the struct.
    let field_idents: Vec<&syn::Ident> = struct_fields
        .iter()
        .map(|field| field.ident.as_ref().unwrap())
        .collect();

    let context_type = syn::Ident::new("__MojomContext", proc_macro2::Span::mixed_site());
    let field_names: Vec<String> = field_idents.iter().map(|name| name.to_string()).collect();

    // A bunch of entries for a MojomType::Struct
    let mojom_type_fields: Vec<proc_macro2::TokenStream> = struct_fields
        .iter()
        .map(|field| {
            let ty = &field.ty;
            let wire_ty = field_wire_type(&field.attrs, ty);
            quote! { <#ty as __mojom_core::MojomParse<#wire_ty, #context_type>>::mojom_type() }
        })
        .collect();

    // The names of the `context` and `value` parameters in the `MojomParse`
    // functions.
    //
    // If the mojom file has a struct with a field named "context" or "value",
    // those functions will create local variables named `context` and
    // `value` respectively. To distinguish that variable from the
    // parameters (and thus prevent accidental variable shadowing), we give
    // the parameters a `mixed_site` span and the variables a `call_site`
    // span. This tells the compiler to treat them as separate values that
    // don't shadow each other.
    let context_ident = syn::Ident::new("context", proc_macro2::Span::mixed_site());
    let value_ident = syn::Ident::new("value", proc_macro2::Span::mixed_site());

    // A bunch of entries for a MojomValue::Struct
    let to_mojom_value_fields: Vec<proc_macro2::TokenStream> = struct_fields
        .iter()
        .map(|field| {
            let ty = &field.ty;
            let wire_ty = field_wire_type(&field.attrs, ty);
            let name = &field.ident;
            quote! {
                <#ty as __mojom_core::MojomParse<#wire_ty, #context_type>>::into_mojom_value(#value_ident.#name, #context_ident)
            }
        })
        .collect();

    let to_wire_value_fields: Vec<proc_macro2::TokenStream> = struct_fields
        .iter()
        .map(|field| {
            let ty = &field.ty;
            let wire_ty = field_wire_type(&field.attrs, ty);
            let name = &field.ident;
            quote! {
                <#ty as __mojom_core::MojomParse<#wire_ty, #context_type>>::into_mojom_value_for_wire(#value_ident.#name, #context_ident)
            }
        })
        .collect();

    // The body of a struct value, converting each field from a MojomValue with
    // the same name as the field.
    let from_mojom_value_fields: Vec<proc_macro2::TokenStream> = struct_fields
        .iter()
        .map(|field| {
            let ty = &field.ty;
            let wire_ty = field_wire_type(&field.attrs, ty);
            let name = &field.ident;
            quote! {
                #name: <#ty as __mojom_core::MojomParse<#wire_ty, #context_type>>::try_from_mojom_value(#name, #context_ident)?
            }
        })
        .collect();

    // We wrap the `impl` blocks in an anonymous scope so that we can
    // import mojom_value_parser_core without polluting the caller's namespace.
    return quote! {
        const _: () = {
            #imports

            impl<#context_type: __MojomRegistrar> __mojom_core::MojomParse<Self, #context_type> for #name {
                fn mojom_type() -> __mojom_core::MojomType {
                    __mojom_core::MojomType::Struct {
                        field_names: vec![#(#field_names.to_string()),*],
                        fields: vec![#(#mojom_type_fields),*],
                    }
                }

                fn wire_type() -> &'static __mojom_core::MojomWireType {
                    // Derived structs have a concrete Self type. A per-type
                    // cell avoids the global fallback's lock and hash lookup
                    // on every message. As with the trait fallback, Context
                    // must not change a type's wire schema.
                    static WIRE_TYPE: ::std::sync::OnceLock<__mojom_core::MojomWireType> = ::std::sync::OnceLock::new();
                    WIRE_TYPE.get_or_init(|| __mojom_core::pack_mojom_type(
                        &<Self as __mojom_core::MojomParse<Self, #context_type>>::mojom_type()
                    ))
                }

                fn into_mojom_value(self, #context_ident: &#context_type) -> __mojom_core::MojomValue {
                    let #value_ident = self;
                    __mojom_core::MojomValue::Struct (
                        vec![#(#field_names.to_string()),*],
                        vec![#(#to_mojom_value_fields),*],
                    )
                }

                fn into_mojom_value_for_wire(self, #context_ident: &#context_type) -> __mojom_core::MojomValue {
                    let #value_ident = self;
                    __mojom_core::MojomValue::Struct (
                        Vec::new(),
                        vec![#(#to_wire_value_fields),*],
                    )
                }

                fn try_from_mojom_value(#value_ident: __mojom_core::MojomValue, #context_ident: &#context_type) -> __mojom_anyhow::Result<Self> {
                    let __mojom_core::MojomValue::Struct(field_names, fields) = #value_ident else {
                        __mojom_anyhow::bail!(
                            "Cannot construct a value of type {} from non-struct MojomValue {:?}",
                            std::any::type_name::<#name>(),
                            #value_ident
                        );
                    };

                    if fields.len() != #num_fields {
                        __mojom_anyhow::bail!(
                            "Wrong number of fields to construct a value of type {} from MojomValue {:?}",
                            std::any::type_name::<#name>(),
                            __mojom_core::MojomValue::Struct(field_names, fields)
                        )
                    };

                    // Try to extract all the field values at once
                    let fields: [__mojom_core::MojomValue; #num_fields] = fields.try_into().unwrap();
                    let [#(#field_idents),*] = fields;
                    return Ok(Self {
                        #(#from_mojom_value_fields),*
                    })
                }
            }
        };
    };
}

fn derive_mojomparse_union(
    name: syn::Ident,
    variants: syn::punctuated::Punctuated<syn::Variant, syn::Token![,]>,
    imports: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    // Extract/compute just the bits of the variants that we care about:
    // The name, type, wire type, and discriminant value
    let mut next_discriminant: i64 = 0;
    let variant_info : Vec<(syn::Ident, syn::Type, syn::Type, i32)>
        = variants.into_iter().map(|variant: syn::Variant| {
        let variant_name = variant.ident;
        let variant_attrs = variant.attrs;
        let mut fields = match variant.fields {
            syn::Fields::Unnamed(syn::FieldsUnnamed { unnamed, .. }) => unnamed,
            syn::Fields::Named(syn::FieldsNamed { named, .. }) => named,
            syn::Fields::Unit => syn::punctuated::Punctuated::new(), // We'll panic shortly
        };
        if fields.len() != 1 {
            let mut panic_msg = format!("Variant {variant_name} of enum {name} must have exactly one field to be serialized as a Mojom union.");
            if fields.len() == 0 {
                panic_msg.push_str("\nTo serialize as a Mojom enum, derive PrimitiveEnum instead, which will automatically provide MojomParse.")
            }
            panic!("{}", panic_msg)
        }
        let field_ty = fields.pop().unwrap().into_value().ty;
        let wire_ty = field_wire_type(&variant_attrs, &field_ty);
        let discriminant = compute_next_discriminant(&mut next_discriminant, variant.discriminant);
        (variant_name, field_ty, wire_ty, discriminant)
    }).collect();

    let context_type = syn::Ident::new("__MojomContext", proc_macro2::Span::mixed_site());
    let mojom_type_fields = variant_info
        .iter()
        .map(|(_, ty, wire_ty, discriminant)| quote! { (#discriminant, <#ty as __mojom_core::MojomParse<#wire_ty, #context_type>>::mojom_type()) });
    let context_ident = syn::Ident::new("context", proc_macro2::Span::mixed_site());
    let value_ident = syn::Ident::new("value", proc_macro2::Span::mixed_site());

    let to_mojom_value_branches = variant_info
        .iter()
        .map(|(variant_name, ty, wire_ty, discriminant)| quote! { #name::#variant_name(v) => (#discriminant, <#ty as __mojom_core::MojomParse<#wire_ty, #context_type>>::into_mojom_value(v, #context_ident)) });
    let to_wire_value_branches = variant_info
        .iter()
        .map(|(variant_name, ty, wire_ty, discriminant)| quote! { #name::#variant_name(v) => (#discriminant, <#ty as __mojom_core::MojomParse<#wire_ty, #context_type>>::into_mojom_value_for_wire(v, #context_ident)) });
    let from_mojom_value_branches = variant_info.iter().map(|(name, ty, wire_ty, discriminant)| {
        // boxed_value is defined by the surrounding scope
        quote! { #discriminant => Ok(Self::#name(<#ty as __mojom_core::MojomParse<#wire_ty, #context_type>>::try_from_mojom_value(*boxed_value, #context_ident)?)), }
    });

    return quote! {
        const _: () = {
            #imports

            impl<#context_type: __MojomRegistrar> __mojom_core::MojomParse<Self, #context_type> for #name {
                fn mojom_type() -> __mojom_core::MojomType {
                    let variants : std::collections::BTreeMap<i32, __mojom_core::MojomType> = [
                        #(#mojom_type_fields),*
                    ].into();
                    __mojom_core::MojomType::Union { variants }
                }

                fn into_mojom_value(self, #context_ident: &#context_type) -> __mojom_core::MojomValue {
                    let (discriminant, mojom_value) = match self {
                        #(#to_mojom_value_branches),*
                    };
                    __mojom_core::MojomValue::Union ( discriminant, Box::new(mojom_value) )
                }

                fn into_mojom_value_for_wire(self, #context_ident: &#context_type) -> __mojom_core::MojomValue {
                    let (discriminant, mojom_value) = match self {
                        #(#to_wire_value_branches),*
                    };
                    __mojom_core::MojomValue::Union ( discriminant, Box::new(mojom_value) )
                }

                fn try_from_mojom_value(#value_ident: __mojom_core::MojomValue, #context_ident: &#context_type) -> __mojom_anyhow::Result<Self> {
                    let __mojom_core::MojomValue::Union(discriminant, boxed_value) = #value_ident else {
                        __mojom_anyhow::bail!(
                            "Cannot construct a value of type {} from non-union MojomValue {:?}",
                            std::any::type_name::<#name>(),
                            #value_ident
                        );
                    };

                    match discriminant {
                        #(#from_mojom_value_branches)*
                        discriminant => __mojom_anyhow::bail!(
                                    "Invalid discriminant to construct a value of type {} from MojomValue {:?}",
                                    std::any::type_name::<#name>(),
                                    __mojom_core::MojomValue::Union(discriminant, boxed_value))
                    }
                }
            }
        };
    };
}

// Compute the discriminant for this field, as either the provided value or the
// value of `next_discriminant`. In either case, set `next_discriminant` to the
// computed value + 1
fn compute_next_discriminant(
    next_discriminant: &mut i64,
    discriminant_opt: Option<(syn::Token![=], syn::Expr)>,
) -> i32 {
    let discriminant = match discriminant_opt {
        Some((
            _,
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Int(n),
                ..
            }),
        )) => {
            let discriminant = n
                .base10_parse::<i64>()
                .expect("Enum/Union discriminants must be a 32-bit integer literal.");
            discriminant
        }
        Some((
            _,
            syn::Expr::Unary(syn::ExprUnary {
                op: syn::UnOp::Neg(_),
                expr,
                ..
            }),
        )) => {
            if let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Int(n),
                ..
            }) = *expr
            {
                let discriminant = n
                    .base10_parse::<i64>()
                    .expect("Enum/Union discriminants must be a 32-bit integer literal.");
                -discriminant
            } else {
                panic!("Enum/Union discriminants must be a 32-bit integer literal.");
            }
        }
        None => *next_discriminant,
        _ => panic!("Enum/Union discriminants must be a 32-bit integer literal."),
    };
    let discriminant =
        i32::try_from(discriminant).expect("Enum/Union discriminant exceeds the i32 wire range");
    *next_discriminant = i64::from(discriminant) + 1;
    discriminant
}

#[proc_macro_derive(PrimitiveEnum, attributes(mojom))]
pub fn derive_primitiveenum(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let attributes = match MojomAttributes::parse(&input.attrs) {
        Ok(attributes) => attributes,
        Err(error) => return error.into_compile_error().into(),
    };
    let imports = match attributes.imports(false) {
        Ok(imports) => imports,
        Err(error) => return error.into_compile_error().into(),
    };
    let name = input.ident;

    let variants = match input.data {
        syn::Data::Enum(syn::DataEnum { variants, .. }) => variants,
        _ => panic!("No structs or unions allowed!"),
    };

    let mut next_discriminant: i64 = 0;
    let mut default_variant: Option<syn::Ident> = None;
    let generate_branch = |variant: syn::Variant| {
        if !variant.fields.is_empty() {
            panic!("Mojom enums must not have any variants with fields!")
        }

        // TODO(crbug.com/496945860): See if any variants have a "default"
        // attribute
        default_variant = None; // Silence compiler until we do that

        let discriminant = compute_next_discriminant(&mut next_discriminant, variant.discriminant);
        let variant_name = variant.ident;

        quote! {
            #discriminant => Ok(#name::#variant_name)
        }
    };

    let mut branches = variants
        .into_iter()
        .map(generate_branch)
        .collect::<Vec<_>>();
    if let Some(default) = default_variant {
        branches.push(quote! { _ => Ok(#default)})
    } else {
        branches.push(quote! { _ => Err(__mojom_anyhow::anyhow!(
            "Invalid discriminant {value} for type {}",
            std::any::type_name::<#name>()
        ))})
    }

    let context_type = syn::Ident::new("__MojomContext", proc_macro2::Span::mixed_site());

    let quoted = quote! {
        const _ : () = {
            #imports

            impl From<#name> for i32 {
                fn from(value: #name) -> i32 { value as i32 }
            }

            impl TryFrom<i32> for #name {
                type Error = __mojom_anyhow::Error;

                fn try_from(value : i32) -> __mojom_anyhow::Result<Self> {
                    match value {
                        #(#branches),*
                    }
                }
            }

            impl __mojom_core::PrimitiveEnum for #name {}

            impl<#context_type> __mojom_core::MojomParse<Self, #context_type> for #name {
                fn mojom_type() -> __mojom_core::MojomType {
                    __mojom_core::MojomType::Enum {
                        is_valid: __mojom_core::Predicate::new::<#name>(&(<Self as __mojom_core::PrimitiveEnum>::is_valid as fn(i32) -> bool)),
                    }
                }

                fn into_mojom_value(self, _context: &#context_type) -> __mojom_core::MojomValue {
                    __mojom_core::MojomValue::Enum(self.into())
                }

                fn try_from_mojom_value(value: __mojom_core::MojomValue, _context: &#context_type) -> __mojom_anyhow::Result<Self> {
                    if let __mojom_core::MojomValue::Enum(v) = value {
                        Ok(Self::try_from(v)?)
                    } else {
                        __mojom_anyhow::bail!(
                            "Cannot construct a value of type {} from non-enum MojomValue {:?}",
                            std::any::type_name::<Self>(),
                            value
                        )
                    }
                }
            }
        };
    };

    return proc_macro::TokenStream::from(quoted);
}

#[cfg(test)]
mod cargo_adapter_tests {
    use super::*;

    #[test]
    fn runtime_override_uses_facade_alias_for_both_paths() {
        let input: DeriveInput = syn::parse_quote! {
            #[mojom(runtime = "::renamed_ipc")]
            struct Payload { value: i32 }
        };
        let imports = MojomAttributes::parse(&input.attrs)
            .unwrap()
            .imports(true)
            .unwrap()
            .to_string();
        assert!(imports.contains("renamed_ipc :: __private :: mojom_value_parser_core"));
        assert!(imports.contains("renamed_ipc :: __private :: bindings"));
        assert!(!imports.contains("chromium"));
    }

    #[test]
    fn internal_registrar_does_not_add_a_bindings_self_dependency() {
        let input: DeriveInput = syn::parse_quote! {
            #[mojom(in_bindings_crate, core_path = "::internal_codec")]
            struct Payload { value: i32 }
        };
        let imports = MojomAttributes::parse(&input.attrs)
            .unwrap()
            .imports(true)
            .unwrap()
            .to_string();
        assert!(imports.contains("crate :: interface :: Registrar"));
        assert!(imports.contains("internal_codec"));
    }

    #[test]
    fn enum_wire_range_includes_both_i32_endpoints() {
        let mut next = 0;
        let min: syn::Variant = syn::parse_quote!(Min = -2147483648);
        assert_eq!(
            compute_next_discriminant(&mut next, min.discriminant),
            i32::MIN
        );
        let max: syn::Variant = syn::parse_quote!(Max = 2147483647);
        assert_eq!(
            compute_next_discriminant(&mut next, max.discriminant),
            i32::MAX
        );
        assert_eq!(next, i64::from(i32::MAX) + 1);
    }
}
