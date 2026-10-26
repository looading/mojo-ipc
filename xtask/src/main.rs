use anyhow::{Context, Result, bail};
use quote::{format_ident, quote};
use std::{collections::BTreeSet, env, fs, path::PathBuf};

fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    match env::args().nth(1).as_deref() {
        Some("gen-sys") => {
            let check = match env::args().nth(2).as_deref() {
                None => false,
                Some("--check") => true,
                _ => bail!("usage: cargo xtask gen-sys [--check]"),
            };
            generate_sys(&root, check)
        }
        Some("install-native-overlay") => {
            let checkout = env::args()
                .nth(2)
                .context("usage: cargo run -p xtask -- install-native-overlay CHROMIUM_SRC")?;
            let status = std::process::Command::new("python3")
                .arg(root.join("native/scripts/build_native.py"))
                .arg("--chromium-src")
                .arg(checkout)
                .status()?;
            if !status.success() {
                bail!("native overlay installation failed");
            }
            Ok(())
        }
        _ => bail!("usage: cargo run -p xtask -- <gen-sys|install-native-overlay CHROMIUM_SRC>"),
    }
}

fn generate_bindings(root: &std::path::Path, target: &str) -> Result<String> {
    let include = root.join("crates/mojo-sys/include");
    let header = include.join("mojo/public/c/system/thunks.h");
    let bindings = bindgen::Builder::default()
        .header(header.to_string_lossy())
        .header(
            root.join("native/include/mojo_rs_native.h")
                .to_string_lossy(),
        )
        .clang_arg(format!("-I{}", include.display()))
        .clang_arg(format!("--target={target}"))
        // Public Mojo headers expose constants as inline constexpr in C++;
        // their cast-valued C macros are not all extracted by bindgen.
        .clang_arg("-x")
        .clang_arg("c++")
        .clang_arg("-std=c++17")
        .allowlist_type("Mojo.*")
        .allowlist_var("MOJO_.*")
        .blocklist_function("Mojo.*")
        .generate_comments(false)
        .derive_debug(true)
        .generate()
        .context("generate official Mojo C types")?
        .to_string();
    Ok(bindings)
}

fn generate_dispatch(bindings: &str) -> Result<(String, usize)> {
    let syntax = syn::parse_file(bindings)?;
    let table = syntax
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Struct(s) if s.ident == "MojoSystemThunks2" => Some(s),
            _ => None,
        })
        .context("MojoSystemThunks2 missing from bindgen output")?;
    let mut functions = Vec::new();
    for field in &table.fields {
        let field_name = field.ident.as_ref().context("unnamed thunk field")?;
        if field_name == "size" {
            continue;
        }
        let syn::Type::Path(option) = &field.ty else {
            bail!("unexpected thunk field {field_name}");
        };
        let syn::PathArguments::AngleBracketed(args) =
            &option.path.segments.last().unwrap().arguments
        else {
            bail!("expected optional function");
        };
        let Some(syn::GenericArgument::Type(syn::Type::BareFn(function))) = args.args.first()
        else {
            bail!("expected function pointer");
        };
        let name = format_ident!("Mojo{}", field_name);
        let output = &function.output;
        let mut inputs = Vec::new();
        let mut arguments = Vec::new();
        for (i, input) in function.inputs.iter().enumerate() {
            let arg = input
                .name
                .as_ref()
                .map(|(id, _)| id.clone())
                .unwrap_or_else(|| format_ident!("arg{i}"));
            let ty = &input.ty;
            inputs.push(quote!(#arg: #ty));
            arguments.push(arg);
        }
        functions.push(quote! {
            #[allow(non_snake_case)]
            pub unsafe fn #name(#(#inputs),*) #output {
                let table = crate::system_thunks();
                let function = table.#field_name.expect(concat!("missing Mojo thunk: ", stringify!(#field_name)));
                unsafe { function(#(#arguments),*) }
            }
        });
    }
    let count = functions.len();
    let dispatch = syn::parse2::<syn::File>(quote!(#(#functions)*))?;
    Ok((prettyplease::unparse(&dispatch), count))
}

fn opaque_x86_padding(bindings: &str) -> Result<String> {
    let mut syntax = syn::parse_file(bindings)?;
    let padding_structs: BTreeSet<_> = syntax
        .items
        .iter()
        .filter_map(|item| {
            let syn::Item::Struct(item) = item else {
                return None;
            };
            item.fields
                .iter()
                .any(|field| {
                    let syn::Type::Path(path) = &field.ty else {
                        return false;
                    };
                    path.path
                        .segments
                        .last()
                        .is_some_and(|segment| segment.ident == "__BindgenBitfieldUnit")
                })
                .then(|| item.ident.to_string())
        })
        .collect();
    if padding_structs.is_empty() {
        bail!("Windows x86 C headers have no expected anonymous pointer padding");
    }
    let mut replaced_helper = false;
    let mut items = Vec::new();
    for item in syntax.items {
        match item {
            syn::Item::Struct(item) if item.ident == "__BindgenBitfieldUnit" => {
                if replaced_helper {
                    bail!("duplicate bindgen bitfield helper");
                }
                replaced_helper = true;
                items.push(syn::parse2(quote! {
                    pub type __BindgenBitfieldUnit<Storage> = crate::__CAnonymousPadding<Storage>;
                })?);
            }
            syn::Item::Impl(mut item) => {
                let syn::Type::Path(path) = &*item.self_ty else {
                    items.push(syn::Item::Impl(item));
                    continue;
                };
                let name = path
                    .path
                    .segments
                    .last()
                    .context("empty impl type")?
                    .ident
                    .to_string();
                if name == "__BindgenBitfieldUnit" {
                    continue;
                }
                if padding_structs.contains(&name) {
                    for method in &mut item.items {
                        let syn::ImplItem::Fn(method) = method else {
                            bail!("unexpected C padding implementation");
                        };
                        // 只有匿名、无数据 getter、无参数的 padding 构造器允许替换。
                        if !method.sig.ident.to_string().starts_with("new_bitfield_")
                            || !method.sig.inputs.is_empty()
                        {
                            bail!(
                                "named C bitfield API requires a separate binding policy: {name}"
                            );
                        }
                        method.block =
                            syn::parse2(quote! { { ::std::default::Default::default() } })?;
                    }
                }
                items.push(syn::Item::Impl(item));
            }
            item => items.push(item),
        }
    }
    if !replaced_helper {
        bail!("missing bindgen anonymous bitfield helper");
    }
    syntax.items = items;
    Ok(prettyplease::unparse(&syntax))
}

fn generate_sys(root: &std::path::Path, check: bool) -> Result<()> {
    // 绑定始终来自随 SDK 固定的官方 C 头，不跟随运行库的自定义 pin。
    // 显式选择 Clang target，避免在不同生成机器上静默改写指针布局。
    let bindings64 = generate_bindings(root, "x86_64-unknown-linux-gnu")?;
    let bindings32 = generate_bindings(root, "i686-pc-windows-msvc")?;
    let (dispatch, count) = generate_dispatch(&bindings64)?;
    if generate_dispatch(&bindings32)?.0 != dispatch {
        bail!("Windows x86 thunk signatures differ from the 64-bit C API");
    }
    let bindings32 = format!(
        "// C layout target: i686-pc-windows-msvc.\n\
         // Mojo headers: official Chromium bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd.\n\
         // bindgen 0.72.1 layout; anonymous padding uses opaque MaybeUninit storage.\n{}",
        opaque_x86_padding(&bindings32)?
    );
    let output = root.join("crates/mojo-sys/src/generated");
    let files = [
        ("types.rs", bindings64),
        ("types_windows_x86.rs", bindings32),
        ("dispatch.rs", dispatch),
    ];
    if check {
        for (name, expected) in &files {
            if fs::read_to_string(output.join(name)).unwrap_or_default() != *expected {
                bail!("generated C binding is stale: {name}; run cargo xtask gen-sys");
            }
        }
        println!("verified both C layouts and {count} invariant thunk signatures");
    } else {
        fs::create_dir_all(&output)?;
        for (name, contents) in files {
            fs::write(output.join(name), contents)?;
        }
        println!("generated both C layouts and {count} invariant thunk signatures");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_layouts_share_the_same_dispatch_signatures() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_owned();
        let output = root.join("crates/mojo-sys/src/generated");
        let current64 = fs::read_to_string(output.join("types.rs")).unwrap();
        let current32 = fs::read_to_string(output.join("types_windows_x86.rs")).unwrap();
        assert_eq!(
            generate_dispatch(&current64).unwrap(),
            generate_dispatch(&current32).unwrap()
        );
    }

    #[test]
    fn named_bitfields_are_rejected_instead_of_erasing_value_access() {
        let source = "pub struct __BindgenBitfieldUnit<T> { storage: T }\n\
                      pub struct C { pub _bitfield_1: __BindgenBitfieldUnit<[u8;4]> }\n\
                      impl C { pub fn flags(&self) -> u32 { 0 } }";
        assert!(
            opaque_x86_padding(source)
                .unwrap_err()
                .to_string()
                .contains("named C bitfield")
        );
    }

    #[test]
    fn anonymous_padding_has_no_raw_value_getters() {
        let source = "pub struct __BindgenBitfieldUnit<T> { storage: T }\n\
                      impl<T> __BindgenBitfieldUnit<T> { pub fn get(&self) {} }\n\
                      pub struct C { pub _bitfield_1: __BindgenBitfieldUnit<[u8;4]> }\n\
                      impl C { pub fn new_bitfield_1() -> __BindgenBitfieldUnit<[u8;4]> { loop {} } }";
        let output = opaque_x86_padding(source).unwrap();
        assert!(output.contains("crate::__CAnonymousPadding<Storage>"));
        assert!(!output.contains("fn get("));
        assert!(output.contains("Default::default()"));
    }
}
