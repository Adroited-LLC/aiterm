use quote::ToTokens;
use std::{
    fs,
    path::{Path, PathBuf},
};
use syn::{FnArg, Item, Pat, ReturnType, Type};

fn camel(name: &str) -> String {
    let mut parts = name.split('_');
    let mut value = parts.next().unwrap_or_default().to_owned();
    for part in parts {
        let mut chars = part.chars();
        if let Some(c) = chars.next() {
            value.extend(c.to_uppercase());
            value.push_str(chars.as_str());
        }
    }
    value
}

fn scan(file: &Path, module: &str, arms: &mut Vec<String>) {
    println!("cargo:rerun-if-changed={}", file.display());
    let source = fs::read_to_string(file).unwrap();
    let parsed = syn::parse_file(&source).unwrap();
    for item in parsed.items {
        match item {
            Item::Fn(f)
                if f.attrs
                    .iter()
                    .any(|a| a.to_token_stream().to_string().contains("tauri :: command")) =>
            {
                let name = f.sig.ident.to_string();
                let args: Vec<String> = f.sig.inputs.iter().map(|input| {
                    let FnArg::Typed(arg) = input else { panic!("command receiver") };
                    let Pat::Ident(pat) = &*arg.pat else { panic!("command argument pattern") };
                    let last = match &*arg.ty { Type::Path(p) => p.path.segments.last().unwrap().ident.to_string(), _ => String::new() };
                    match last.as_str() {
                        "State" => "app.state()".into(),
                        "AppHandle" => "app.clone()".into(),
                        _ => {
                            let key = camel(&pat.ident.to_string());
                            format!("serde_json::from_value(args.get({key:?}).cloned().unwrap_or(serde_json::Value::Null)).map_err(|e| serde_json::json!(format!(\"Invalid {key}: {{e}}\")))?")
                        }
                    }
                }).collect();
                let await_ = if f.sig.asyncness.is_some() {
                    ".await"
                } else {
                    ""
                };
                let result = matches!(&f.sig.output, ReturnType::Type(_, t) if matches!(&**t, Type::Path(p) if p.path.segments.last().unwrap().ident == "Result"));
                let unwrap = if result {
                    ".map_err(|e| serde_json::to_value(e).unwrap_or(serde_json::Value::Null))?"
                } else {
                    ""
                };
                arms.push(format!("{name:?} => serde_json::to_value(crate::{module}::{name}({}){await_}{unwrap}).map_err(|e| serde_json::json!(e.to_string())),", args.join(",")));
            }
            Item::Mod(m)
                if m.content.is_none()
                    && !m.attrs.iter().any(|a| {
                        a.path().is_ident("cfg") && a.to_token_stream().to_string().contains("test")
                    }) =>
            {
                let parent = if file.file_name().unwrap() == "mod.rs" {
                    file.parent().unwrap().to_owned()
                } else {
                    file.with_extension("")
                };
                let direct = parent.join(format!("{}.rs", m.ident));
                let nested = parent.join(m.ident.to_string()).join("mod.rs");
                let child = if direct.exists() { direct } else { nested };
                if child.exists() {
                    scan(&child, &format!("{module}::{}", m.ident), arms);
                }
            }
            _ => {}
        }
    }
}

fn main() {
    println!("cargo:rustc-check-cfg=cfg(aiterm_headless)");
    println!("cargo:rustc-cfg=aiterm_headless");
    println!("cargo:rerun-if-changed=src/core_modules.rs");
    let modules = syn::parse_file(&fs::read_to_string("src/core_modules.rs").unwrap()).unwrap();
    let mut arms = Vec::new();
    for item in modules.items {
        let Item::Mod(m) = item else { continue };
        for attr in &m.attrs {
            if !attr.path().is_ident("path") {
                continue;
            }
            let syn::Meta::NameValue(meta) = &attr.meta else {
                continue;
            };
            let syn::Expr::Lit(expr) = &meta.value else {
                continue;
            };
            let syn::Lit::Str(path) = &expr.lit else {
                continue;
            };
            scan(
                &PathBuf::from("src").join(path.value()),
                &m.ident.to_string(),
                &mut arms,
            );
        }
    }
    let code = format!("pub async fn dispatch(command: &str, args: serde_json::Value, app: crate::runtime::AppHandle) -> Result<serde_json::Value,serde_json::Value> {{ use crate::runtime::Manager; match command {{ {} _ => Err(serde_json::json!(format!(\"Unknown workspace command: {{command}}\"))), }} }}", arms.join("\n"));
    fs::write(
        PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("commands.rs"),
        code,
    )
    .unwrap();
}
