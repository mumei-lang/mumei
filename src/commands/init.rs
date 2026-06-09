use std::fs;
use std::path::Path;

pub(crate) fn cmd_init(name: &str) {
    let project_dir = Path::new(name);
    if project_dir.exists() {
        eprintln!("❌ Error: Directory '{}' already exists", name);
        std::process::exit(1);
    }

    // ディレクトリ構造を作成
    fs::create_dir_all(project_dir.join("src")).unwrap_or_else(|e| {
        eprintln!("❌ Error: Failed to create directory: {}", e);
        std::process::exit(1);
    });
    let _ = fs::create_dir_all(project_dir.join("dist"));

    // mumei.toml
    let toml_content = format!(
        r#"[package]
name = "{}"
version = "0.1.0"
# authors = ["Your Name"]
# description = "A formally verified Mumei project"
# repository = "https://github.com/mumei-lang/your-project"
[dependencies]
# 依存パッケージをここに記述
# example = {{ path = "./libs/example" }}
# math = {{ git = "https://github.com/mumei-lang/math-mm", tag = "v1.0.0" }}
[build]
verify = true
max_unroll = 3
[proof]
cache = true
timeout_ms = 10000
cross_spec_verify = true
[effects]
# allowed = ["Log", "FileRead"]
# denied = ["Network"]
"#,
        name
    );
    fs::write(project_dir.join("mumei.toml"), toml_content).unwrap();

    // .gitignore
    let gitignore_content = r#"# Mumei build artifacts
dist/
*.ll

# Verification cache (regenerated automatically)
.mumei_build_cache
.mumei_cache

# OS files
.DS_Store
Thumbs.db

# Editor files
.vscode/settings.json
*.swp
*~
"#;
    fs::write(project_dir.join(".gitignore"), gitignore_content).unwrap();

    // src/main.mm — 充実したテンプレート（検証成功例 + 標準ライブラリ使用例）
    let main_content = format!(
        r#"// =============================================================
// {} — Mumei Project
// =============================================================
//
// このファイルは mumei init で生成されたサンプルプロジェクトです。
// 形式検証の基本的な使い方を示しています。
//
// 実行方法:
//   mumei build src/main.mm -o dist/output
//   mumei verify src/main.mm
//   mumei check src/main.mm

import "std/option" as option;

// --- 精緻型（Refinement Type） ---
// 型に述語制約を付与し、Z3 で自動検証します
type Nat = i64 where v >= 0;
type Pos = i64 where v > 0;

// --- 基本的な atom（関数） ---
// requires（事前条件）と ensures（事後条件）を Z3 が数学的に証明します
atom increment(n: Nat)
requires:
    n >= 0;
ensures:
    result >= 1;
body: {{
    n + 1
}};

// --- 複数パラメータ + 算術検証 ---
atom safe_add(a: Nat, b: Nat)
requires:
    a >= 0 && b >= 0;
ensures:
    result >= a && result >= b;
body: {{
    a + b
}};

// --- 条件分岐を含む検証 ---
atom clamp(value: i64, min_val: Nat, max_val: Pos)
requires:
    min_val >= 0 && max_val > 0 && min_val < max_val;
ensures:
    result >= min_val && result <= max_val;
body: {{
    if value < min_val then min_val
    else if value > max_val then max_val
    else value
}};

// --- スタック操作（契約による安全性保証） ---
atom stack_push(top: Nat, max_size: Pos)
requires:
    top >= 0 && max_size > 0 && top < max_size;
ensures:
    result >= 1 && result <= max_size;
body: {{
    top + 1
}};

atom stack_pop(top: Pos)
requires:
    top > 0;
ensures:
    result >= 0;
body: {{
    top - 1
}};
"#,
        name
    );
    fs::write(project_dir.join("src/main.mm"), main_content).unwrap();

    println!("🗡️  Created new Mumei project '{}'", name);
    println!();
    println!("  {}/", name);
    println!("  ├── mumei.toml");
    println!("  ├── .gitignore");
    println!("  ├── dist/");
    println!("  └── src/");
    println!("      └── main.mm");
    println!();
    println!("Get started:");
    println!("  cd {}", name);
    println!("  mumei build src/main.mm -o dist/output");
    println!("  mumei verify src/main.mm");
    println!("  mumei check src/main.mm");
    println!("  mumei inspect                           # inspect environment");
}

// =============================================================================
// mumei inspect — environment check
// =============================================================================
