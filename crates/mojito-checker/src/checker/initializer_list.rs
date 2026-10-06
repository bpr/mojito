//! Initializer lists: `{}` and `{a, b}` at the type their context expects,
//! upstream's set-initializer literal emitted as the construction `T(a, b)`
//! of that type.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl Checker {
    /// Type an initializer list against the type its context expects: the
    /// construction `expected(entries...)`, spelled under an identity derived
    /// from the brace's and checked as any construction is, so the brace
    /// keeps the type its context sees while the construction keeps its own
    /// facts. The construction is recorded for the checked arena, which hands
    /// HIR the construction in the brace's place.
    pub(super) fn infer_initializer_list(
        &self,
        expression: &Expr,
        entries: &[Expr],
        expected: &Ty,
        record: bool,
    ) -> Result<Ty, TypeError> {
        let construction = self.initializer_list_construction(expression, entries, expected)?;
        let ty = self.infer_with_expected(&construction, expected, record)?;
        self.expression_types
            .borrow_mut()
            .insert(expression.source_span(), ty.clone());
        self.operation_adjustments.borrow_mut().insert(
            expression.source_span(),
            mojito_checked::checked::SemanticAdjustment::InitializerList {
                construction: mojito_checked::checked::InitializerListConstruction(Box::new(
                    construction,
                )),
            },
        );
        Ok(ty)
    }

    /// The construction `expected(entries...)` an initializer list stands
    /// for: a type parameter's own construction, a pack element's
    /// (`Self.Ts[i]()` / `Ts[i]()` over the element's pack and index), or the
    /// closed type's spelling applied to the entries. The entries keep their
    /// identities; the construction and the spelled arguments take ones
    /// derived from the brace.
    fn initializer_list_construction(
        &self,
        expression: &Expr,
        entries: &[Expr],
        expected: &Ty,
    ) -> Result<Expr, TypeError> {
        let args = entries.to_vec();
        let unspellable = || {
            TypeError::Unsupported(format!(
                "cannot emit initializer list for '{expected}': the type has no construction \
                 to spell"
            ))
        };
        let kind = match expected {
            Ty::Param { binder, .. } => ExprKind::Call {
                name: binder.name.to_string(),
                param_args: Vec::new(),
                args,
                kwargs: Vec::new(),
            },
            Ty::Dependent(dependent) if let Some((list, index)) = dependent.pack_element() => {
                let pack = list
                    .as_decl_ref()
                    .map(|reference| reference.name.trim_start_matches('*').to_string())
                    .ok_or_else(unspellable)?;
                let index =
                    pack_index_expression(index, expression.span).ok_or_else(unspellable)?;
                let own = self.self_decls.iter().any(|declaration| {
                    matches!(declaration, ParamDecl::Type { name, variadic: true, .. }
                        if name.trim_start_matches('*') == pack)
                });
                let callee = if own {
                    ExprKind::Member {
                        object: Box::new(Expr::new(
                            ExprKind::Identifier("Self".to_string()),
                            expression.span,
                        )),
                        field: pack,
                    }
                } else {
                    ExprKind::Identifier(pack)
                };
                ExprKind::Invoke {
                    callee: Box::new(Expr::new(callee, expression.span)),
                    param_args: vec![mojito_ast::ast::ParamArg::Value(index)],
                    args,
                    kwargs: Vec::new(),
                }
            }
            _ => {
                let name = match mojito_types::ct::source_type(expected, expression.span) {
                    Some(SourceType::Named(name, param_args)) => {
                        return Ok(self.derived_construction(
                            expression,
                            ExprKind::Call {
                                name,
                                param_args,
                                args,
                                kwargs: Vec::new(),
                            },
                        ));
                    }
                    Some(SourceType::Int) => "Int",
                    Some(SourceType::UInt) => "UInt",
                    Some(SourceType::Bool) => "Bool",
                    Some(SourceType::Float64) => "Float64",
                    _ => return Err(unspellable()),
                };
                ExprKind::Call {
                    name: name.to_string(),
                    param_args: Vec::new(),
                    args,
                    kwargs: Vec::new(),
                }
            }
        };
        Ok(self.derived_construction(expression, kind))
    }

    /// `kind` as the construction spelled for the initializer list
    /// `expression`: its location, with every synthesized node (the call and
    /// its spelled parameter arguments, never the entries) numbered from the
    /// brace's identity.
    #[allow(clippy::unused_self, reason = "an operation of the checker's pass")]
    fn derived_construction(&self, expression: &Expr, kind: ExprKind) -> Expr {
        let mut identities = mojito_ast::visit::DerivedIdentities {
            parent: expression.syntax_id,
            next: 1,
        };
        let mut construction = Expr {
            kind,
            span: expression.span,
            source: expression.source.clone(),
            syntax_id: mojito_common::token::SyntaxId::derived(expression.syntax_id, 0),
        };
        let synthesized: Vec<&mut Expr> = match &mut construction.kind {
            ExprKind::Call { param_args, .. } | ExprKind::Invoke { param_args, .. } => param_args
                .iter_mut()
                .filter_map(|argument| match argument {
                    mojito_ast::ast::ParamArg::Value(value) => Some(value),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        for value in synthesized {
            value.source.clone_from(&expression.source);
            mojito_ast::visit::walk_expr_mut(&mut identities, value);
        }
        construction
    }
}

/// The source spelling of a pack element's index that is still a parameter
/// expression: a binder in scope (`i`) or a closed integer.
fn pack_index_expression(
    index: &mojito_types::param_expr::ParamExpr,
    span: mojito_common::token::Span,
) -> Option<Expr> {
    if let Some(binder) = index.as_decl_ref() {
        return Some(Expr::new(
            ExprKind::Identifier(binder.name.to_string()),
            span,
        ));
    }
    index.as_constant()?.materialize(span)
}
