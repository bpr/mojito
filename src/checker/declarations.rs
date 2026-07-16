//! Declaration collection, signature construction, and body-checking support.

use super::*;

pub(super) fn definitely_returns(body: &[Stmt]) -> bool {
    body.iter().any(stmt_returns)
}

pub(super) fn stmt_returns(stmt: &Stmt) -> bool {
    match &stmt.kind {
        StmtKind::Return(_) => true,
        // A `raise` diverges (it never falls through to the end), so for
        // reachability it behaves like a `return`.
        StmtKind::Raise(_) => true,
        StmtKind::If { branches, orelse } => {
            orelse.as_ref().is_some_and(|e| definitely_returns(e))
                && branches.iter().all(|(_, b)| definitely_returns(b))
        }
        // A `try` definitely diverges when: a `finally` does (it overrides every
        // path); or the **normal-completion** path diverges (the body — or, if the
        // body may complete, the `else`) *and* the **exceptional** path does (every
        // `except` handler diverges; with no handler, an uncaught raise itself
        // exits, so only the normal path can fall through).
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => {
            if finalbody.as_ref().is_some_and(|fb| definitely_returns(fb)) {
                return true;
            }
            let normal = match orelse {
                Some(else_) => definitely_returns(body) || definitely_returns(else_),
                None => definitely_returns(body),
            };
            let exceptional = match except {
                Some((_, handler)) => definitely_returns(handler),
                None => true,
            };
            normal && exceptional
        }
        _ => false,
    }
}

/// Conservative definite-initialization check for a named `out` result. A
/// value-returning path supplies the result directly; a fallthrough or bare
/// return path must assign the named result first.
pub(super) fn definitely_initializes_named_result(body: &[Stmt], name: &str) -> bool {
    let mut initialized = false;
    for stmt in body {
        match &stmt.kind {
            StmtKind::Assign { name: target, .. } if target == name => {
                initialized = true;
            }
            StmtKind::Return(Some(_)) | StmtKind::Raise(_) => return true,
            StmtKind::Return(None) => return initialized,
            StmtKind::If { branches, orelse } => {
                let Some(orelse) = orelse else { continue };
                if branches
                    .iter()
                    .all(|(_, branch)| definitely_initializes_named_result(branch, name))
                    && definitely_initializes_named_result(orelse, name)
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    initialized
}

/// Whether every normally completing path initializes `self.field`. A raised
/// path does not produce a value and therefore need not initialize the field;
/// every explicit return and every fallthrough path does. Loops are treated as
/// possibly executing zero times.
pub(super) fn definitely_initializes_self_field(body: &[Stmt], field: &str) -> bool {
    let flow = init_field_flow(body, field, false);
    flow.valid && flow.normal.is_none_or(|initialized| initialized)
}

#[derive(Clone, Copy)]
struct InitFieldFlow {
    /// Initialization state on normal fallthrough, or `None` when no path falls
    /// through (all paths returned or raised).
    normal: Option<bool>,
    /// Every value-producing exit seen so far was initialized.
    valid: bool,
}

fn init_field_flow(body: &[Stmt], field: &str, mut initialized: bool) -> InitFieldFlow {
    let mut valid = true;
    for stmt in body {
        match &stmt.kind {
            StmtKind::SetPlace { place, .. }
                if matches!(
                    &place.kind,
                    ExprKind::Member { object, field: assigned }
                        if assigned == field
                            && matches!(&object.kind, ExprKind::Identifier(name) if name == "self")
                ) =>
            {
                initialized = true;
            }
            StmtKind::Return(_) => {
                return InitFieldFlow {
                    normal: None,
                    valid: valid && initialized,
                };
            }
            StmtKind::Raise(_) => {
                return InitFieldFlow {
                    normal: None,
                    valid,
                };
            }
            StmtKind::If { branches, orelse } => {
                let mut flows: Vec<_> = branches
                    .iter()
                    .map(|(_, branch)| init_field_flow(branch, field, initialized))
                    .collect();
                flows.push(match orelse {
                    Some(orelse) => init_field_flow(orelse, field, initialized),
                    None => InitFieldFlow {
                        normal: Some(initialized),
                        valid: true,
                    },
                });
                valid &= flows.iter().all(|flow| flow.valid);
                let mut normal_paths = flows.iter().filter_map(|flow| flow.normal);
                initialized = match normal_paths.next() {
                    Some(first) => normal_paths.fold(first, |all, state| all && state),
                    None => {
                        return InitFieldFlow {
                            normal: None,
                            valid,
                        };
                    }
                };
            }
            // A loop may execute zero times, so assignments in its body cannot
            // establish initialization after the loop. Returns inside it still
            // have to be safe.
            StmtKind::While { body, .. } | StmtKind::For { body, .. } => {
                valid &= init_field_flow(body, field, initialized).valid;
            }
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                let body_flow = init_field_flow(body, field, initialized);
                valid &= body_flow.valid;
                let normal_flow = body_flow.normal.map(|state| match orelse {
                    Some(orelse) => init_field_flow(orelse, field, state),
                    None => InitFieldFlow {
                        normal: Some(state),
                        valid: true,
                    },
                });
                let exceptional_flow = except
                    .as_ref()
                    .map(|(_, handler)| init_field_flow(handler, field, initialized));
                valid &= normal_flow.is_none_or(|flow| flow.valid)
                    && exceptional_flow.is_none_or(|flow| flow.valid);

                let mut exits: Vec<bool> = normal_flow
                    .and_then(|flow| flow.normal)
                    .into_iter()
                    .chain(exceptional_flow.and_then(|flow| flow.normal))
                    .collect();
                if except.is_none() && body_flow.normal.is_none() {
                    exits.clear();
                }
                if let Some(finalbody) = finalbody {
                    let starts = if exits.is_empty() {
                        vec![initialized]
                    } else {
                        exits
                    };
                    let final_flows: Vec<_> = starts
                        .into_iter()
                        .map(|state| init_field_flow(finalbody, field, state))
                        .collect();
                    valid &= final_flows.iter().all(|flow| flow.valid);
                    exits = final_flows.iter().filter_map(|flow| flow.normal).collect();
                }
                if exits.is_empty() {
                    return InitFieldFlow {
                        normal: None,
                        valid,
                    };
                }
                initialized = exits.into_iter().all(|state| state);
            }
            _ => {}
        }
    }
    InitFieldFlow {
        normal: Some(initialized),
        valid,
    }
}
