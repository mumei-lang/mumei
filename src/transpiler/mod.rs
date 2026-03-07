pub mod golang;
pub mod rust;
pub mod typescript;

use crate::hir::HirAtom;
use crate::parser::{EnumDef, ImplDef, ImportDecl, StructDef, TraitDef};

#[derive(Copy, Clone)]
pub enum TargetLanguage {
    TypeScript,
    Rust,
    Go,
}

pub fn transpile(hir_atom: &HirAtom, lang: TargetLanguage) -> String {
    match lang {
        TargetLanguage::TypeScript => typescript::transpile_to_ts(hir_atom),
        TargetLanguage::Rust => rust::transpile_to_rust(hir_atom),
        TargetLanguage::Go => golang::transpile_to_go(hir_atom),
    }
}

/// Enum 定義を各言語の型定義に変換する
pub fn transpile_enum(enum_def: &EnumDef, lang: TargetLanguage) -> String {
    match lang {
        TargetLanguage::Rust => rust::transpile_enum_rust(enum_def),
        TargetLanguage::Go => golang::transpile_enum_go(enum_def),
        TargetLanguage::TypeScript => typescript::transpile_enum_ts(enum_def),
    }
}

/// Struct 定義を各言語の型定義に変換する
pub fn transpile_struct(struct_def: &StructDef, lang: TargetLanguage) -> String {
    match lang {
        TargetLanguage::Rust => rust::transpile_struct_rust(struct_def),
        TargetLanguage::Go => golang::transpile_struct_go(struct_def),
        TargetLanguage::TypeScript => typescript::transpile_struct_ts(struct_def),
    }
}

/// Trait 定義を各言語のインターフェース定義に変換する
pub fn transpile_trait(trait_def: &TraitDef, lang: TargetLanguage) -> String {
    match lang {
        TargetLanguage::Rust => rust::transpile_trait_rust(trait_def),
        TargetLanguage::Go => golang::transpile_trait_go(trait_def),
        TargetLanguage::TypeScript => typescript::transpile_trait_ts(trait_def),
    }
}

/// Impl 定義を各言語のトレイト実装に変換する
pub fn transpile_impl(impl_def: &ImplDef, lang: TargetLanguage) -> String {
    match lang {
        TargetLanguage::Rust => rust::transpile_impl_rust(impl_def),
        TargetLanguage::Go => golang::transpile_impl_go(impl_def),
        TargetLanguage::TypeScript => typescript::transpile_impl_ts(impl_def),
    }
}

/// import 宣言からバンドルファイルのヘッダー（mod/use, package/import, import/export）を生成する
pub fn transpile_module_header(
    imports: &[ImportDecl],
    module_name: &str,
    lang: TargetLanguage,
) -> String {
    match lang {
        TargetLanguage::Rust => rust::transpile_module_header_rust(imports, module_name),
        TargetLanguage::Go => golang::transpile_module_header_go(imports, module_name),
        TargetLanguage::TypeScript => typescript::transpile_module_header_ts(imports),
    }
}
