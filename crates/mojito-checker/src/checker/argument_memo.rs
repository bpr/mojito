//! Typing a call's arguments once while its overloads are ranked, as
//! upstream's parser emits each argument once and ranks the candidates
//! over the typed values: without it, every candidate of a construction
//! re-types the whole argument subtree, and a nest of numeric
//! constructions (`Int(Int(Int(1)))`) costs the candidates' count raised
//! to its depth.

use super::Checker;
use mojito_common::error::TypeError;
use mojito_common::token::SourceSpan;
use mojito_types::types::Ty;
use std::collections::HashMap;

/// The context-free types of the expressions typed while an overload set
/// is ranked, by expression and by the positions the check stood in.
#[derive(Default)]
pub(super) struct ArgumentMemo {
    /// How many rankings are open; the memo is empty at zero.
    depth: usize,
    entries: HashMap<MemoKey, Result<Ty, TypeError>>,
    /// The keys in the order they were typed, so a probe forgets the
    /// entries typed under it along with the facts it undoes.
    log: Vec<MemoKey>,
}

impl Checker {
    /// Run `rank` with the memo open: an expression typed twice within it,
    /// in the same positions, is typed once.
    pub(super) fn with_argument_memo<R>(&self, rank: impl FnOnce() -> R) -> R {
        self.argument_memo.borrow_mut().depth += 1;
        let result = rank();
        let mut memo = self.argument_memo.borrow_mut();
        memo.depth -= 1;
        if memo.depth == 0 {
            memo.entries.clear();
            memo.log.clear();
        }
        result
    }

    /// `expr`'s memoized type, or `infer` run and memoized while a ranking
    /// is open.
    pub(super) fn memoized_infer(
        &self,
        expr: &mojito_ast::ast::Expr,
        infer: impl FnOnce() -> Result<Ty, TypeError>,
    ) -> Result<Ty, TypeError> {
        if self.argument_memo.borrow().depth == 0 {
            return infer();
        }
        let key = self.memo_key(expr.source_span());
        if let Some(typed) = self.argument_memo.borrow().entries.get(&key) {
            return typed.clone();
        }
        let typed = infer();
        let mut memo = self.argument_memo.borrow_mut();
        if memo.depth > 0 && memo.entries.insert(key.clone(), typed.clone()).is_none() {
            memo.log.push(key);
        }
        typed
    }

    /// The memo's position when a probe opens.
    pub(super) fn argument_memo_mark(&self) -> usize {
        self.argument_memo.borrow().log.len()
    }

    /// Forget every entry typed since `mark`, whose facts the probe undid.
    pub(super) fn roll_back_argument_memo(&self, mark: usize) {
        let mut memo = self.argument_memo.borrow_mut();
        if mark < memo.log.len() {
            let forgotten: Vec<_> = memo.log.drain(mark..).collect();
            for key in forgotten {
                memo.entries.remove(&key);
            }
        }
    }

    fn memo_key(&self, span: SourceSpan) -> MemoKey {
        MemoKey {
            span,
            comptime_positions: self.comptime_positions.get(),
            lifting_positions: self.lifting_positions.get(),
            probing_literal: self.probing_literal.get(),
            instance_inferences: self.instance_inferences.borrow().len(),
        }
    }
}

/// An expression and the positions that change how it types.
#[derive(Clone, PartialEq, Eq, Hash)]
struct MemoKey {
    span: SourceSpan,
    comptime_positions: usize,
    lifting_positions: usize,
    probing_literal: bool,
    instance_inferences: usize,
}
