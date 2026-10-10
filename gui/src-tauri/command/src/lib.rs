//! `#[fastctx_gui_command::command]`: an async Tauri command whose body lives
//! on the heap.
//!
//! Tauri builds each async command's future, inside its own response future,
//! on the thread that dispatches the call: on Windows the main thread, whose
//! stack is 1 MB. A command that awaits the World's client carries every
//! nested future inline, a few hundred kilobytes each, and moving two of them
//! through the dispatch frames overflowed that stack the moment the window
//! opened. Boxing the body leaves only the arguments and a pointer in the
//! command's own future; the body is allocated when a worker first polls it.

use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, parse_macro_input, parse_quote};

/// `#[tauri::command]`, with the async function's body boxed and awaited.
/// The arguments, their names and the return type are untouched, so Tauri
/// reads them as it always does.
#[proc_macro_attribute]
pub fn command(attributes: TokenStream, item: TokenStream) -> TokenStream {
    let mut function = parse_macro_input!(item as ItemFn);
    if function.sig.asyncness.is_none() {
        return syn::Error::new_spanned(&function.sig, "a boxed Tauri command must be async").into_compile_error().into();
    }
    let body = &function.block;
    function.block = parse_quote!({ ::std::boxed::Box::pin(async move #body).await });
    let attributes = proc_macro2::TokenStream::from(attributes);
    let attributes = if attributes.is_empty() { quote!() } else { quote!((#attributes)) };
    quote!(#[::tauri::command #attributes] #function).into()
}
