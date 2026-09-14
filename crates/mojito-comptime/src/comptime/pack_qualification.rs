//! Upstream's qualification rule for a variadic struct's own pack.
//!
//! Inside the struct's members — fields, `comptime` members, and methods —
//! the pack is `Self.Ts`; the bare name belongs to the header (its parameter
//! list, conformance clauses, and trailing `where`), where `Self` is not
//! available. A member spelling it bare is rejected with upstream's text, and
//! the qualified spread `*Self.Ts` the parser keeps apart is then folded onto
//! the bare `*Ts` node every later fold expands. A method or nested scope
//! that binds the same name owns it, so its bare uses are its own.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::visit::{self, Visitor};

/// Check and normalize every variadic struct declaration of `program`.
///
/// Runs on the linked program each elaboration entry receives, ahead of the
/// synthesized members that copy field types; it is a linear walk over the
/// struct declarations, so repeating it per discovery round is cheap.
pub(super) fn qualify_struct_packs(program: &mut [Stmt]) -> Result<(), ComptimeError> {
    for statement in program.iter_mut() {
        let StmtKind::Struct {
            type_params,
            fields,
            associated,
            methods,
            ..
        } = &statement.kind
        else {
            continue;
        };
        let packs = type_params
            .iter()
            .filter_map(|parameter| parameter.name.strip_prefix('*'))
            .map(str::to_string)
            .collect::<Vec<_>>();
        for pack in &packs {
            let mut walk = BarePackUse { pack, found: false };
            for field in fields {
                visit::walk_field(&mut walk, field);
            }
            for member in associated {
                visit::walk_struct_comptime(&mut walk, member);
            }
            for method in methods {
                visit::walk_method(&mut walk, method);
            }
            if walk.found {
                return Err(ComptimeError::UnqualifiedStructParam(pack.clone()));
            }
        }
        let spreads: HashMap<String, Type> = packs
            .iter()
            .map(|pack| {
                let spread = format!("*{pack}");
                (spread.clone(), Type::Named(spread, Vec::new()))
            })
            .collect();
        if !spreads.is_empty() {
            substitute_type_bindings_in_block(std::slice::from_mut(statement), &spreads);
        }
    }
    Ok(())
}

/// Finds the bare struct pack in a member: the identifier, the type name
/// (indexed or not), or the bare spread.
struct BarePackUse<'a> {
    pack: &'a str,
    found: bool,
}

impl Visitor for BarePackUse<'_> {
    fn visit_expr(&mut self, expression: &Expr) {
        if matches!(&expression.kind, ExprKind::Identifier(name) if name == self.pack) {
            self.found = true;
        }
    }

    fn visit_type(&mut self, ty: &Type) {
        if matches!(ty, Type::Named(name, _) if name.trim_start_matches('*') == self.pack) {
            self.found = true;
        }
    }

    fn enter_scope(&mut self, names: &[&str]) -> bool {
        !names
            .iter()
            .any(|name| name.trim_start_matches('*') == self.pack)
    }
}
