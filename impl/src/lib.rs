extern crate proc_macro;

#[macro_use]
extern crate syn;

use proc_macro2::{Span, TokenStream};
use quote::{quote, ToTokens};

use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input,
    visit_mut::VisitMut,
    Block, Expr, ExprParen, Generics, Ident, Lifetime, LifetimeDef, Receiver, ReturnType, Stmt,
    TypeBareFn, TypeImplTrait, TypeParam, TypeParamBound, TypeReference,
};

struct AsyncRead {
    stmts: Vec<Stmt>,
}

impl AsyncRead {
    pub fn visit_mut(&mut self, v: &mut (impl VisitMut + ?Sized)) {
        for stmt in &mut self.stmts {
            v.visit_stmt_mut(stmt);
        }
    }
}

impl ToTokens for AsyncRead {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        for stmt in &self.stmts {
            stmt.to_tokens(tokens);
        }
    }
}

impl Parse for AsyncRead {
    fn parse(input: ParseStream) -> syn::parse::Result<Self> {
        let stmts = input.call(Block::parse_within)?;
        Ok(Self { stmts })
    }
}

#[proc_macro]
pub fn async_read(body: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let mut block = parse_macro_input!(body as AsyncRead);
    let context_arg = Ident::new("_async_io_macros_context_argument", Span::call_site());
    block.visit_mut(&mut ReplaceYield(&context_arg));
    block.visit_mut(&mut ExpandAwait(&context_arg));
    quote!({
        // Safety: We trust users not to come here, see that argument name we
        // generated above and use that in their code to break our other safety
        // guarantees. Our use of it in await! is safe because of reasons
        // probably described in the embrio-async safety notes.
        unsafe {
            ::async_io_macros::make_async_read(static |mut #context_arg: ::async_io_macros::AsyncReadContext| -> ::std::io::Result<()> {
                if false { #context_arg = yield ::std::task::Poll::Pending; }
                #block
            })
        }
    }).into()
}

struct ExpandAwait<'a>(&'a Ident);

impl syn::visit_mut::VisitMut for ExpandAwait<'_> {
    fn visit_expr_mut(&mut self, node: &mut syn::Expr) {
        syn::visit_mut::visit_expr_mut(self, node);

        let input = match node {
            syn::Expr::Await(syn::ExprAwait { base, .. }) => &*base,
            _ => return,
        };

        let context_arg = self.0;
        *node = syn::parse_quote!({
            let mut pinned = #input;
            loop {
                let polled = unsafe {
                    let pin = ::core::pin::Pin::new_unchecked(&mut pinned);
                    ::core::future::Future::poll(pin, #context_arg.get_context())
                };
                if let ::core::task::Poll::Ready(x) = polled {
                    break x;
                }
                #context_arg = yield ::core::task::Poll::Pending;
            }
        });
    }
}

struct ReplaceYield<'a>(&'a Ident);

impl syn::visit_mut::VisitMut for ReplaceYield<'_> {
    fn visit_expr_mut(&mut self, i: &mut Expr) {
        match i {
            Expr::Closure(_) => {
                // Don't descend into closures
            }
            Expr::Paren(ExprParen { expr, .. }) if matches!(expr.as_ref(), Expr::Yield(_)) => {
                // `yield ()` is sometimes parsed as `(yield ())`, remove
                // the unnecessary parentheses
                *i = expr.as_ref().clone();
                self.visit_expr_mut(i);
            }
            Expr::Yield(node) => {
                let context_arg = self.0;
                let closure = node.expr.as_ref().unwrap();
                *i = syn::parse_quote!({
                    fn yield_buffer(
                        mut context: ::async_io_macros::AsyncReadContext,
                        closure: impl ::std::ops::FnOnce(&mut [u8]) -> usize,
                    ) -> usize {
                        closure(unsafe { context.get_buffer() })
                    }
                    let count = yield_buffer(#context_arg, #closure);
                    #context_arg = yield ::core::task::Poll::Ready(count);
                });
            }
            i => {
                syn::visit_mut::visit_expr_mut(self, i);
            }
        }
    }
}

#[derive(Default)]
struct AsyncFnTransform {
    original_lifetimes: Vec<Lifetime>,
}

fn future_lifetime() -> Lifetime {
    Lifetime::new("'future", Span::call_site())
}

// Transforms a function of form
//
// ```
// #[embiro_async]
// async fn name(...) -> ReturnType {
//    ...
// }
// ```
//
// into
//
// ```
// fn name(...) -> impl Future<Output = ReturnType + ...> {
//    ...
// }
// ```
impl VisitMut for AsyncFnTransform {
    fn visit_type_reference_mut(&mut self, i: &mut TypeReference) {
        if i.lifetime.is_none() {
            i.lifetime = future_lifetime().into();
        }
        self.visit_type_mut(&mut *i.elem);
    }
    fn visit_receiver_mut(&mut self, i: &mut Receiver) {
        match i {
            Receiver {
                reference: Some((_, lifetime)),
                ..
            } if lifetime.is_none() => *lifetime = future_lifetime().into(),
            _ => (),
        }
    }
    fn visit_type_bare_fn_mut(&mut self, _i: &mut TypeBareFn) {}
    fn visit_type_impl_trait_mut(&mut self, i: &mut TypeImplTrait) {
        for bound in i.bounds.iter_mut() {
            self.visit_type_param_bound_mut(bound);
        }
        i.bounds.push(TypeParamBound::Lifetime(future_lifetime()));
    }
    fn visit_type_param_mut(&mut self, i: &mut TypeParam) {
        if i.colon_token.is_none() {
            i.colon_token = Some(Token![:](Span::call_site()));
        }
        for bound in i.bounds.iter_mut() {
            self.visit_type_param_bound_mut(bound);
        }
        i.bounds.push(future_lifetime().into())
    }
    fn visit_lifetime_mut(&mut self, i: &mut Lifetime) {
        if i.ident == "_" {
            *i = future_lifetime();
        }
    }
    fn visit_lifetime_def_mut(&mut self, i: &mut LifetimeDef) {
        if i.colon_token.is_none() {
            i.colon_token = Some(Token![:](Span::call_site()));
        }
        i.bounds.push(future_lifetime());
    }
    fn visit_generics_mut(&mut self, i: &mut Generics) {
        self.original_lifetimes = i.lifetimes().map(|lt| lt.lifetime.clone()).collect();
        for param in i.params.iter_mut() {
            self.visit_generic_param_mut(param);
        }
        i.params
            .insert(0, LifetimeDef::new(future_lifetime()).into());
    }
    fn visit_block_mut(&mut self, i: &mut Block) {
        *i = syn::parse_quote!({ async move #i });
    }
    fn visit_return_type_mut(&mut self, i: &mut ReturnType) {
        let lifetimes = &self.original_lifetimes;
        *i = syn::parse2(match i {
            ReturnType::Default => quote! {
                -> impl ::core::future::Future<Output = ()> #(+ ::async_io_macros::Captures<#lifetimes>)* + 'future
            },
            ReturnType::Type(_, ty) => quote! {
                -> impl ::core::future::Future<Output = #ty> #(+ ::async_io_macros::Captures<#lifetimes>)* + 'future
            },
        }).unwrap();
    }
}
