#![allow(clippy::result_large_err)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::new_without_default)]

pub mod ast;
pub mod emitter;
#[allow(dead_code)]
pub mod ffi;
pub mod hir;
pub mod inspect;
pub mod manifest;
pub mod mir;
pub mod mir_analysis;
pub mod parser;
pub mod proof_cert;
pub mod registry;
pub mod resolver;
pub mod verification;
