//! The node handle MIR lowering reads a checked expression through: HIR's
//! tree for it and its checked node, whose op names its operands and whose
//! attributes are its facts ([`super::facts`]).

use mojito_checked::checked::{CheckedExpr, CheckedNodeId, CheckedOp, CheckedTables};
use mojito_hir::hir::HirExpr;

/// One checked expression as MIR lowers it: HIR's tree for it (its operands'
/// trees and the storage it denotes) and its checked node. Operand `i` is
/// `hir.children[i]`, in [`CheckedOp::operands`] order.
#[derive(Clone, Copy)]
pub(super) struct Node<'a> {
    pub(super) hir: &'a HirExpr,
    pub(super) checked: &'a CheckedExpr,
    tables: &'a CheckedTables,
}

impl<'a> Node<'a> {
    /// The handle of HIR expression `hir`, whose node `tables` holds.
    pub(super) fn new(hir: &'a HirExpr, tables: &'a CheckedTables) -> Self {
        let checked = tables
            .expression(hir.node)
            .expect("a HIR expression names a node of its tables");
        debug_assert_eq!(
            hir.children
                .iter()
                .map(|child| child.node)
                .collect::<Vec<_>>(),
            checked.op.operands(),
            "a HIR expression's children are its node's operands"
        );
        Self {
            hir,
            checked,
            tables,
        }
    }

    pub(super) const fn id(self) -> CheckedNodeId {
        self.checked.id
    }

    /// The HIR expression's address, which keys per-occurrence lowering
    /// state for as long as the function's CFG lives.
    pub(super) fn address(self) -> usize {
        std::ptr::from_ref::<HirExpr>(self.hir) as usize
    }

    pub(super) const fn op(self) -> &'a CheckedOp {
        &self.checked.op
    }

    /// The operands' handles, in [`CheckedOp::operands`] order.
    pub(super) fn operands(self) -> impl Iterator<Item = Node<'a>> {
        self.hir
            .children
            .iter()
            .map(move |child| Self::new(child, self.tables))
    }

    pub(super) fn operand(self, index: usize) -> Option<Self> {
        self.hir
            .children
            .get(index)
            .map(|child| Self::new(child, self.tables))
    }

    /// The object of a member read, subscript, method call, or slice.
    pub(super) fn object(self) -> Option<Self> {
        match self.op() {
            CheckedOp::Member { object, .. }
            | CheckedOp::Index { object, .. }
            | CheckedOp::MethodCall { object, .. }
            | CheckedOp::Slice { object, .. }
            | CheckedOp::MultiIndex { object, .. } => self.operand_of(*object),
            _ => None,
        }
    }

    /// The index of a subscript.
    pub(super) fn index(self) -> Option<Self> {
        match self.op() {
            CheckedOp::Index { index, .. } => self.operand_of(*index),
            _ => None,
        }
    }

    /// The positional arguments of a call, invocation, or method call.
    pub(super) fn args(self) -> Vec<Self> {
        match self.op() {
            CheckedOp::Call { args, .. }
            | CheckedOp::Invoke { args, .. }
            | CheckedOp::MethodCall { args, .. } => args
                .iter()
                .filter_map(|argument| self.operand_of(*argument))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The keyword arguments of a call, invocation, or method call, with
    /// their names.
    pub(super) fn kwargs(self) -> Vec<(&'a str, Self)> {
        match self.op() {
            CheckedOp::Call { kwargs, .. }
            | CheckedOp::Invoke { kwargs, .. }
            | CheckedOp::MethodCall { kwargs, .. } => kwargs
                .iter()
                .filter_map(|kwarg| {
                    self.operand_of(kwarg.value)
                        .map(|value| (kwarg.name.as_str(), value))
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The handle of operand node `id` of this node.
    fn operand_of(self, id: CheckedNodeId) -> Option<Self> {
        self.operands().find(|operand| operand.id() == id)
    }
}
