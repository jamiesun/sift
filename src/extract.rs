use std::path::Path;

use serde::Serialize;
use tree_sitter::{Node, Parser};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Lang {
    Rust,
    Python,
}

impl Lang {
    pub fn from_path(p: &Path) -> Option<Lang> {
        match p.extension().and_then(|e| e.to_str()) {
            Some("rs") => Some(Lang::Rust),
            Some("py") => Some(Lang::Python),
            _ => None,
        }
    }
    fn ts(self) -> tree_sitter::Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
        }
    }
}

/// 脱水后的扁平骨架：只留拓扑，丢注释与函数体。
#[derive(Debug, Default, Serialize)]
pub struct AstSummary {
    pub path: String,
    pub lang: Option<&'static str>,
    pub imports: Vec<String>,
    pub signatures: Vec<String>,
    pub calls: Vec<String>,
    /// 带行号的结构索引，供 P4 风险账本定位。
    pub locations: Vec<AstLocation>,
    /// 跳出当前目录树的引用，交大模型脑补。
    pub external: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AstLocation {
    pub kind: &'static str,
    pub line: usize,
    pub text: String,
}

/// 解析失败/坏节点返回 None 或残缺骨架，绝不 panic；解析完即 drop AST。
pub fn dehydrate(path: &Path, src: &[u8]) -> Option<AstSummary> {
    let lang = Lang::from_path(path)?;
    let mut parser = Parser::new();
    if parser.set_language(&lang.ts()).is_err() {
        return None;
    }
    let tree = parser.parse(src, None)?;
    let mut sum = AstSummary {
        path: path.display().to_string(),
        ..Default::default()
    };
    sum.lang = Some(match lang {
        Lang::Rust => "rust",
        Lang::Python => "python",
    });
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
}

fn walk(root: Node, src: &[u8], lang: Lang, sum: &mut AstSummary) {
    let mut cursor = root.walk();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match (lang, node.kind()) {
            (Lang::Rust, "use_declaration")
            | (Lang::Python, "import_statement")
            | (Lang::Python, "import_from_statement") => {
                if let Some(l) = first_line(node, src) {
                    if is_external(&l) {
                        sum.external.push(format!("[EXTERNAL_BLACKBOX] {l}"));
                    }
                    push_location(sum, "import", node, &l);
                    sum.imports.push(l);
                }
            }
            (
                Lang::Rust,
                "function_item" | "struct_item" | "enum_item" | "trait_item" | "impl_item",
            )
            | (Lang::Python, "function_definition" | "class_definition") => {
                if let Some(l) = first_line(node, src) {
                    push_location(sum, "signature", node, &l);
                    sum.signatures.push(l);
                }
            }
            (Lang::Rust, "call_expression") | (Lang::Python, "call") => {
                if let Some(f) = node.child(0).and_then(|c| first_line(c, src)) {
                    push_location(sum, "call", node, &f);
                    sum.calls.push(f);
                }
            }
            _ => {}
        }
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
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
    fn broken_input_no_panic() {
        let r = dehydrate(&PathBuf::from("a.rs"), b"fn ( { ] unterminated").unwrap_or_default();
        assert_eq!(r.lang, Some("rust"));
    }

    #[test]
    fn unknown_ext_is_none() {
        assert!(dehydrate(&PathBuf::from("a.txt"), b"hi").is_none());
    }
}
