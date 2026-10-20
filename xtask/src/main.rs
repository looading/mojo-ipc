use anyhow::{Context, Result, bail};
use quote::{format_ident, quote};
use std::{env, fs, path::PathBuf};

fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    match env::args().nth(1).as_deref() {
        Some("gen-sys") => generate_sys(&root),
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

fn generate_sys(root: &std::path::Path) -> Result<()> {
    let include = root.join("crates/mojo-sys/include");
    let header = include.join("mojo/public/c/system/thunks.h");
    let bindings = bindgen::Builder::default()
        .header(header.to_string_lossy())
        .header(
            root.join("native/include/mojo_rs_native.h")
                .to_string_lossy(),
        )
        .clang_arg(format!("-I{}", include.display()))
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
    let syntax = syn::parse_file(&bindings)?;
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
    let output = root.join("crates/mojo-sys/src/generated");
    fs::create_dir_all(&output)?;
    fs::write(output.join("types.rs"), bindings)?;
    let dispatch = syn::parse2::<syn::File>(quote!(#(#functions)*))?;
    fs::write(output.join("dispatch.rs"), prettyplease::unparse(&dispatch))?;
    println!(
        "generated {} official thunk dispatch functions",
        functions.len()
    );
    Ok(())
}
