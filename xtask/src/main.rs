use std::{env, fs, path::Path, process::Command};
use syn::{Attribute, Expr, ExprCall, ExprMethodCall, File, ItemFn, Macro, visit::Visit};

fn run(args: &[&str]) -> bool {
    Command::new("cargo")
        .args(args)
        .status()
        .is_ok_and(|s| s.success())
}

const FORBIDDEN_MACROS: &[&str] = &["todo", "unimplemented", "panic", "dbg", "unreachable"];
const FORBIDDEN_METHODS: &[&str] = &[
    "unwrap",
    "expect",
    "unwrap_err",
    "expect_err",
    "unwrap_unchecked",
];
const FORBIDDEN_ATTRIBUTES: &[&str] = &["allow", "expect", "ignore"];

#[derive(Default)]
struct SlopVisitor {
    errors: Vec<String>,
}
impl<'ast> Visit<'ast> for SlopVisitor {
    fn visit_macro(&mut self, value: &'ast Macro) {
        if let Some(name) = value.path.segments.last().map(|seg| seg.ident.to_string())
            && FORBIDDEN_MACROS.contains(&name.as_str())
        {
            self.errors.push(format!("forbidden macro `{name}!`"));
        }
        syn::visit::visit_macro(self, value);
    }
    fn visit_expr_method_call(&mut self, value: &'ast ExprMethodCall) {
        let method = value.method.to_string();
        if FORBIDDEN_METHODS.contains(&method.as_str()) {
            self.errors.push(format!("forbidden `{method}` call"));
        }
        syn::visit::visit_expr_method_call(self, value);
    }
    fn visit_expr_call(&mut self, value: &'ast ExprCall) {
        if let Expr::Path(expr_path) = &*value.func
            && expr_path.path.segments.len() >= 2
            && let Some(last) = expr_path
                .path
                .segments
                .last()
                .map(|seg| seg.ident.to_string())
            && FORBIDDEN_METHODS.contains(&last.as_str())
        {
            self.errors.push(format!("forbidden UFCS `{last}` call"));
        }
        syn::visit::visit_expr_call(self, value);
    }
    fn visit_attribute(&mut self, value: &'ast Attribute) {
        if let Some(name) = value
            .path()
            .segments
            .last()
            .map(|seg| seg.ident.to_string())
            && FORBIDDEN_ATTRIBUTES.contains(&name.as_str())
        {
            self.errors.push(format!("forbidden attribute `{name}`"));
        }
        syn::visit::visit_attribute(self, value);
    }
    fn visit_item_fn(&mut self, value: &'ast ItemFn) {
        let is_test = value.attrs.iter().any(|attr| {
            attr.path()
                .segments
                .last()
                .is_some_and(|seg| seg.ident == "test")
        });
        if is_test && value.block.stmts.is_empty() {
            self.errors
                .push(format!("empty `#[test]` function `{}`", value.sig.ident));
        }
        syn::visit::visit_item_fn(self, value);
    }
}
fn audit_file(path: &Path, errors: &mut Vec<String>) {
    let source = match fs::read_to_string(path) {
        Ok(value) => value,
        Err(error) => {
            errors.push(format!("{}: {error}", path.display()));
            return;
        }
    };
    match syn::parse_file(&source) {
        Ok(file) => inspect(path, &file, errors),
        Err(error) => errors.push(format!("{}: parse error: {error}", path.display())),
    }
}
fn inspect(path: &Path, file: &File, errors: &mut Vec<String>) {
    let mut visitor = SlopVisitor::default();
    visitor.visit_file(file);
    errors.extend(
        visitor
            .errors
            .into_iter()
            .map(|error| format!("{}: {error}", path.display())),
    );
}
fn audit_tree(path: &Path, errors: &mut Vec<String>) {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => {
            errors.push(format!("{}: {error}", path.display()));
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .is_some_and(|name| name == "target" || name == ".git")
        {
            continue;
        }
        if path.is_dir() {
            audit_tree(&path, errors);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            audit_file(&path, errors);
        }
    }
}
fn crate_tier(name: &str) -> Option<u8> {
    match name {
        "pcc-expr" | "pcc-id" | "pcc-math" => Some(0),
        "pcc-geo" | "pcc-graph" | "pcc-sop" | "pcc-terrain" | "pcc-vdb" => Some(1),
        "pcc-store" | "pcc-cmd" | "pcc-eval" | "pcc-io" | "pcc-vex" => Some(2),
        "pcc-mcp" => Some(3),
        "pcc-mcp-stdio" | "pcc-torture" => Some(4),
        _ => None,
    }
}

fn validate_manifest_tier(crate_name: &str, manifest: &str, errors: &mut Vec<String>) {
    let Some(own_tier) = crate_tier(crate_name) else {
        errors.push(format!(
            "unregistered workspace crate `{crate_name}` in tier DAG"
        ));
        return;
    };
    for line in manifest.lines() {
        let trimmed = line.trim();
        if let Some((dep_key, _)) = trimmed.split_once('=') {
            let dep = dep_key.trim();
            if dep.starts_with("pcc-") {
                let Some(dep_tier) = crate_tier(dep) else {
                    errors.push(format!("{crate_name}: unknown internal dependency `{dep}`"));
                    continue;
                };
                if dep == crate_name || dep_tier > own_tier {
                    errors.push(format!(
                        "{crate_name} (tier {own_tier}) cannot depend on {dep} (tier {dep_tier})"
                    ));
                }
            }
        }
    }
}

fn audit_crate_tiers(crates_dir: &Path, errors: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(crates_dir) else {
        errors.push(format!("failed to read {}", crates_dir.display()));
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(crate_name) = dir.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let manifest_path = dir.join("Cargo.toml");
        match fs::read_to_string(&manifest_path) {
            Ok(manifest) => validate_manifest_tier(crate_name, &manifest, errors),
            Err(err) => errors.push(format!("{}: {err}", manifest_path.display())),
        }
    }
}

fn ast_audit() -> bool {
    let mut errors = Vec::new();
    audit_tree(Path::new("crates"), &mut errors);
    audit_tree(Path::new("xtask"), &mut errors);
    audit_crate_tiers(Path::new("crates"), &mut errors);
    for error in &errors {
        eprintln!("audit: {error}");
    }
    errors.is_empty()
}
fn main() {
    let task = env::args().nth(1).unwrap_or_else(|| "ci".to_owned());
    let audit = || {
        run(&["fmt", "--check"])
            && run(&[
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ])
            && ast_audit()
    };
    let ok = match task.as_str() {
        "audit" => audit(),
        "test" => run(&["test", "--workspace"]),
        "ci" => audit() && run(&["test", "--workspace"]),
        _ => false,
    };
    if !ok {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_errors(source: &str) -> Vec<String> {
        let mut errors = Vec::new();
        if let Ok(file) = syn::parse_file(source) {
            inspect(Path::new("snippet.rs"), &file, &mut errors);
        } else {
            errors.push("parse_error".to_owned());
        }
        errors
    }

    #[test]
    fn slop_visitor_rejects_qualified_macros_ufcs_expect_attr_and_hollow_tests() {
        assert!(!collect_errors("fn f() { std::panic!(\"boom\"); }").is_empty());
        assert!(!collect_errors("fn f() { core::todo!(); }").is_empty());
        assert!(!collect_errors("fn f() { unreachable!(); }").is_empty());
        assert!(!collect_errors("fn f(x: Option<i32>) -> i32 { x.unwrap_unchecked() }").is_empty());
        assert!(!collect_errors("fn f(x: Option<i32>) -> i32 { Option::unwrap(x) }").is_empty());
        assert!(!collect_errors("#[expect(unused_variables)] fn f() {}").is_empty());
        assert!(!collect_errors("#[allow(dead_code)] fn f() {}").is_empty());
        assert!(!collect_errors("#[test] fn hollow() {}").is_empty());
        assert!(collect_errors("#[test] fn meaningful() { assert_eq!(2 + 2, 4); }").is_empty());
    }

    #[test]
    fn crate_tier_dag_rejects_upward_dependencies() {
        let mut ok_errors = Vec::new();
        validate_manifest_tier(
            "pcc-geo",
            "[dependencies]\npcc-math = { path = \"../pcc-math\" }\n",
            &mut ok_errors,
        );
        assert!(ok_errors.is_empty());

        let mut bad_errors = Vec::new();
        validate_manifest_tier(
            "pcc-geo",
            "[dependencies]\npcc-mcp = { path = \"../pcc-mcp\" }\n",
            &mut bad_errors,
        );
        assert!(!bad_errors.is_empty());
    }
}
