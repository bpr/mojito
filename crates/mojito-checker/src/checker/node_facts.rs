//! The check's facts at an occurrence, held in the occurrence's one
//! [`NodeFacts`] record on the node the emitter allocated for it (or its
//! pending record until a node holds it), as upstream sets each decided
//! fact as an attribute on the occurrence's op.
//!
//! Every write goes through the emitter, which logs and journals it, so a
//! body site's records are carried and a probe's are undone as any other
//! fact store's entries are.

use super::Checker;
use mojito_checked::checked::NodeFacts;
use mojito_common::token::SourceSpan;
use std::cell::RefMut;

impl Checker {
    /// The record at `span`, created if absent: the write path.
    pub(super) fn record_at(&self, span: SourceSpan) -> RefMut<'_, NodeFacts> {
        RefMut::map(self.emitter.borrow_mut(), |emitter| emitter.record(span))
    }

    /// One fact recorded at `span`, copied out.
    pub(super) fn fact<T: Clone>(
        &self,
        span: &SourceSpan,
        field: impl FnOnce(&NodeFacts) -> Option<&T>,
    ) -> Option<T> {
        self.emitter.borrow().get(span).and_then(field).cloned()
    }

    /// Whether the record at `span` satisfies `holds`; no record holds
    /// nothing.
    pub(super) fn has_fact(
        &self,
        span: &SourceSpan,
        holds: impl FnOnce(&NodeFacts) -> bool,
    ) -> bool {
        self.emitter.borrow().get(span).is_some_and(holds)
    }

    /// Take one fact out of the record at `span`, as upstream removes an
    /// attribute; a span with no record is left without one.
    pub(super) fn clear_fact<T>(
        &self,
        span: &SourceSpan,
        take: impl FnOnce(&mut NodeFacts) -> Option<T>,
    ) -> Option<T> {
        self.emitter.borrow_mut().get_mut(span).and_then(take)
    }

    /// Clear one flag of the record at `span`, returning whether it was set.
    pub(super) fn clear_flag(
        &self,
        span: &SourceSpan,
        flag: impl FnOnce(&mut NodeFacts) -> &mut bool,
    ) -> bool {
        self.emitter
            .borrow_mut()
            .get_mut(span)
            .is_some_and(|facts| std::mem::take(flag(facts)))
    }
}
