//! Building a member read's object once, as upstream's parser emits the
//! object of `a.x` once and resolves the member against the emitted
//! value: the compile-time builder asks for a member read's object from
//! several readers (a struct value's field, a type member's base, a
//! value's type), and without the memo each of them builds the object
//! again, so a chain `a.x.x…x` costs the readers' count raised to its
//! length. A probe forgets the objects built under it along with the facts
//! it undoes.

use super::{Checker, StorageStrictness};
use mojito_ast::ast::Expr;
use mojito_common::error::TypeError;
use mojito_common::token::SourceSpan;
use mojito_types::param_expr::ParamExpr;

/// The objects of the member reads being built, and the silent build of
/// each one built so far, by expression and the positions it was built in.
#[derive(Default)]
pub(super) struct MemberObjectMemo {
    open: Vec<MemberObjectKey>,
    built: Vec<(MemberObjectKey, Result<ParamExpr, TypeError>)>,
}

impl Checker {
    /// Run `build` with `object`, the object of the member read it builds,
    /// open: the first silent build of `object` in the same positions is
    /// kept until `build` returns, and every later one reads it.
    pub(super) fn with_member_object<R>(&self, object: &Expr, build: impl FnOnce() -> R) -> R {
        let key = self.member_object_key(object);
        let mark = {
            let mut memo = self.member_object_memo.borrow_mut();
            memo.open.push(key);
            memo.built.len()
        };
        let result = build();
        let mut memo = self.member_object_memo.borrow_mut();
        memo.open.pop();
        memo.built.truncate(mark);
        result
    }

    /// The kept silent build of `expr`, an open member read's object.
    pub(super) fn built_member_object(&self, expr: &Expr) -> Option<Result<ParamExpr, TypeError>> {
        let memo = self.member_object_memo.borrow();
        if memo.open.is_empty() {
            return None;
        }
        let key = self.member_object_key(expr);
        memo.built
            .iter()
            .rev()
            .find(|(built, _)| *built == key)
            .map(|(_, built)| built.clone())
    }

    /// Keep `built`, the silent build of `expr`, when `expr` is an open
    /// member read's object.
    pub(super) fn keep_member_object(&self, expr: &Expr, built: &Result<ParamExpr, TypeError>) {
        if self.member_object_memo.borrow().open.is_empty() {
            return;
        }
        let key = self.member_object_key(expr);
        let mut memo = self.member_object_memo.borrow_mut();
        if memo.open.contains(&key) {
            memo.built.push((key, built.clone()));
        }
    }

    /// The memo's position when a probe opens.
    pub(super) fn member_object_mark(&self) -> usize {
        self.member_object_memo.borrow().built.len()
    }

    /// Forget every object built since `mark`, whose facts the probe undid.
    pub(super) fn roll_back_member_objects(&self, mark: usize) {
        self.member_object_memo.borrow_mut().built.truncate(mark);
    }

    fn member_object_key(&self, expr: &Expr) -> MemberObjectKey {
        MemberObjectKey {
            span: expr.source_span(),
            comptime_positions: self.comptime_positions.get(),
            lifting_positions: self.lifting_positions.get(),
            probing_literal: self.probing_literal.get(),
            instance_inferences: self.instance_inferences.borrow().len(),
            strictness: self.application_strictness(),
        }
    }
}

/// An expression and the positions that change how it builds.
#[derive(Clone, PartialEq, Eq)]
struct MemberObjectKey {
    span: SourceSpan,
    comptime_positions: usize,
    lifting_positions: usize,
    probing_literal: bool,
    instance_inferences: usize,
    strictness: StorageStrictness,
}
