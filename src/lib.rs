#![feature(exhaustive_patterns, generator_trait, generators, never_type)]
// TODO: Figure out to hygienically have a loop between proc-macro and library
// crates
//! This crate must not be renamed or facaded because it's referred to by name
//! from some proc-macros.

use core::{
    mem,
    ops::{Generator, GeneratorState},
    pin::Pin,
    ptr::NonNull,
    task::{self, Poll},
};
use futures_io::AsyncRead;
use std::io::Result;

pub use async_io_macros_impl::async_read;

#[doc(hidden)]
/// Dummy trait for capturing additional lifetime bounds on `impl Trait`s
pub trait Captures<'a> {}
impl<'a, T: ?Sized> Captures<'a> for T {}

trait IsPoll {
    type Ready;

    fn into_poll(self) -> Poll<Self::Ready>;
}

impl<T> IsPoll for Poll<T> {
    type Ready = T;

    fn into_poll(self) -> Poll<<Self as IsPoll>::Ready> {
        self
    }
}

pin_project_lite::pin_project! {
    struct AsyncReadImpl<G> {
        #[pin]
        generator: G,
    }
}

impl<G> AsyncRead for AsyncReadImpl<G>
where
    G: Generator<AsyncReadContext, Yield = Poll<usize>, Return = Result<()>>,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        match self
            .project()
            .generator
            .resume(AsyncReadContext::new(cx, buf))
        {
            GeneratorState::Yielded(Poll::Ready(count)) => Poll::Ready(Ok(count)),
            GeneratorState::Yielded(Poll::Pending) => Poll::Pending,
            GeneratorState::Complete(Ok(())) => Poll::Ready(Ok(0)),
            GeneratorState::Complete(Err(err)) => Poll::Ready(Err(err)),
        }
    }
}

#[doc(hidden)]
pub struct UnsafeContextRef(NonNull<task::Context<'static>>);

impl UnsafeContextRef {
    /// Get a reference to the wrapped context
    /// # Safety
    /// TODO
    pub unsafe fn get_context(&mut self) -> &mut task::Context<'_> {
        unsafe fn reattach_context_lifetimes<'a>(
            context: NonNull<task::Context<'static>>,
        ) -> &'a mut task::Context<'a> {
            mem::transmute(context)
        }

        reattach_context_lifetimes(self.0)
    }
}

impl From<&mut task::Context<'_>> for UnsafeContextRef {
    fn from(cx: &mut task::Context<'_>) -> Self {
        fn eliminate_context_lifetimes(
            context: &mut task::Context<'_>,
        ) -> NonNull<task::Context<'static>> {
            unsafe { mem::transmute(context) }
        }

        UnsafeContextRef(eliminate_context_lifetimes(cx))
    }
}

unsafe impl Send for UnsafeContextRef {}

#[doc(hidden)]
pub struct UnsafeMutBufRef(NonNull<[u8]>);

impl UnsafeMutBufRef {
    /// Get a reference to the wrapped buffer
    /// # Safety
    /// TODO
    pub unsafe fn get_buffer(&mut self) -> &mut [u8] {
        self.0.as_mut()
    }
}

impl From<&mut [u8]> for UnsafeMutBufRef {
    fn from(buf: &mut [u8]) -> Self {
        UnsafeMutBufRef(unsafe { NonNull::new_unchecked(buf) })
    }
}

unsafe impl Send for UnsafeMutBufRef {}

#[doc(hidden)]
pub struct AsyncReadContext {
    context: UnsafeContextRef,
    buffer: UnsafeMutBufRef,
}

impl AsyncReadContext {
    pub fn new(context: &mut task::Context<'_>, buffer: &mut [u8]) -> Self {
        Self {
            context: context.into(),
            buffer: buffer.into(),
        }
    }

    /// # Safety
    /// TODO
    pub unsafe fn get_context(&mut self) -> &mut task::Context<'_> {
        self.context.get_context()
    }

    /// # Safety
    /// TODO
    pub unsafe fn get_buffer(&mut self) -> &mut [u8] {
        self.buffer.get_buffer()
    }
}

/// # Safety
/// TODO
#[doc(hidden)]
pub unsafe fn make_async_read<G>(generator: G) -> impl AsyncRead
where
    G: Generator<AsyncReadContext, Return = Result<()>, Yield = Poll<usize>>,
{
    AsyncReadImpl { generator }
}

fn _check_send() {
    fn assert_send<T: Send>(_: T) {}

    unsafe {
        assert_send(make_async_read(move |_: AsyncReadContext| {
            if false {
                yield Poll::Pending;
            }
            Ok(())
        }));
    }
}
