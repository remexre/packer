#![recursion_limit = "1024"]

extern crate proc_macro;

mod ignore;

use std::env;
use std::path::PathBuf;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Lit, Meta, MetaNameValue, NestedMeta};
use walkdir::WalkDir;

use ignore::IgnoreFilter;

fn generate_file_list(file_list: &Vec<PathBuf>) -> TokenStream2 {
    let values = file_list
        .iter()
        .map(|path| path.to_str().unwrap().to_string());

    quote! {
        fn list() -> Self::Item {
            const FILES: &[&str] = &[#(#values),*];
            FILES.into_iter().cloned()
        }

        fn get_str(file_path: &str) -> Option<&'static str> {
            Self::get(file_path).and_then(|s| ::std::str::from_utf8(s).ok())
        }
    }
}

#[cfg(debug_assertions)]
fn generate_assets(_file_list: &Vec<PathBuf>) -> TokenStream2 {
    quote! {
        fn get(file_path: &str) -> Option<&'static [u8]> {
            use std::collections::HashSet;
            use std::fs::read;
            use std::path::{PathBuf, Path};
            use std::sync::Mutex;

            packer::lazy_static! {
                static ref CACHE: Mutex<HashSet<&'static [u8]>> = Mutex::new(HashSet::new());
            }

            let path = PathBuf::from(file_path);
            let file = read(path).ok()?;

            let mut cache = CACHE.lock().unwrap();
            if !cache.contains(&file as &[_]) {
                cache.insert(Box::leak(file.clone().into_boxed_slice()));
            }
            Some(cache.get(&file as &[_]).unwrap())
        }
    }
}

#[cfg(not(debug_assertions))]
fn generate_assets(file_list: &Vec<PathBuf>) -> TokenStream2 {
    let values = file_list
        .iter()
        .map(|path| {
            // let base = folder_path.as_ref();
            let key = String::from(
                path.to_str()
                    .expect("Path does not have a string representation"),
            );
            let canonical_path =
                std::fs::canonicalize(&path).expect("Could not get canonical path");
            let canonical_path_str = canonical_path.to_str();
            quote! { #key => Some(include_bytes!(#canonical_path_str)) }
        })
        .collect::<Vec<_>>();

    quote! {
        fn get(file_path: &str) -> Option<&'static [u8]> {
            match file_path {
                #(#values,)*
                _ => None,
            }
        }
    }
}

fn impl_packer(ast: &syn::DeriveInput) -> TokenStream2 {
    match ast.data {
        Data::Enum(_) => panic!("#[derive(Packer)] must be used on structs."),
        _ => (),
    };

    let ident = &ast.ident;
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();

    let mut file_list = Vec::new();

    // look for #[folder = ""] attributes
    for attr in &ast.attrs {
        let meta = attr.parse_meta().expect("Failed to parse meta.");
        let (ident, meta_list) = match meta {
            Meta::List(list) => (list.ident, list.nested),
            Meta::NameValue(_) => {
                panic!("The API has changed. Please see the docs for the updated syntax.")
            }
            _ => panic!("rtfm"),
        };

        if ident == "packer" {
            let mut source_path = None;
            let mut ignore_filter = IgnoreFilter::default();

            for meta_item in meta_list {
                let meta = match meta_item {
                    NestedMeta::Meta(meta) => meta,
                    _ => panic!("rtfm"),
                };

                let (name, value) = match meta {
                    Meta::NameValue(MetaNameValue { ident, lit, .. }) => (ident, lit),
                    _ => panic!("rtfm"),
                };

                if name == "source" {
                    let path = match value {
                        Lit::Str(s) => PathBuf::from(s.value()),
                        _ => panic!("Attribute value must be a string."),
                    };

                    if let Some(_) = source_path {
                        panic!("Cannot put two sources in the same attribute. Please create a new attribute.");
                    }

                    if !path.exists() {
                        panic!(
                            "Directory '{}' does not exist. cwd: '{}'",
                            path.to_str().unwrap(),
                            env::current_dir().unwrap().to_str().unwrap()
                        );
                    };

                    source_path = Some(path);
                } else {
                    #[cfg(feature = "ignore")]
                    {
                        if name == "ignore" {
                            let pattern = match value {
                                Lit::Str(s) => s.value(),
                                _ => panic!("Attribute value must be a string."),
                            };

                            ignore_filter.add_pattern(pattern);
                        }
                    }
                }
            }

            let source_path = match source_path {
                Some(path) => path,
                None => panic!("No source path provided."),
            };
            if source_path.is_file() {
                // check with the filter anyway
                if !ignore_filter.should_ignore(&source_path) {
                    file_list.push(source_path);
                }
            } else if source_path.is_dir() {
                WalkDir::new(&source_path)
                    .into_iter()
                    .for_each(|dir_entry| {
                        let dir_entry = dir_entry.unwrap_or_else(|err| {
                            panic!("WalkDir error: {}", err);
                        });
                        let file_path = dir_entry.path();

                        if !file_path.is_file() {
                            // ignore directories
                            return;
                        }

                        if !file_path.exists() {
                            panic!("Path doesn't exist: {:?}", &file_path);
                        }

                        if ignore_filter.should_ignore(&file_path) {
                            return;
                        }

                        file_list.push(file_path.to_path_buf());
                    });
            }
        }
    }

    let generate_file_list_fn = generate_file_list(&file_list);
    let generate_assets_fn = generate_assets(&file_list);

    quote! {
        impl #impl_generics ::packer::Packer for #ident #ty_generics #where_clause {
            type Item = ::std::iter::Cloned<::std::slice::Iter<'static, &'static str>>;

            #generate_file_list_fn
            #generate_assets_fn
        }
    }
}

#[proc_macro_derive(Packer, attributes(packer))]
pub fn derive_input_object(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let gen = impl_packer(&ast);
    TokenStream::from(gen)
}
