//! The closure trait each closure's storage struct prints as.
//!
//! A capture-list closure's or capturing lambda's value is a struct nested
//! in its declaration, as upstream's `ClosureEmitter::emitClosure` returns
//! the storage. Upstream's type printer reads that struct's declaration
//! and prints the closure trait it conforms to (`def(x: Int) -> Int`); a
//! `Ty` has no declaration to read, so the check declares each storage's
//! trait here, for the thread it runs on, while it runs
//! ([`ClosureTraits::install`]).

use crate::types::Ty;
use std::cell::RefCell;
use std::collections::HashMap;

/// The closure traits one check declares, installed on its thread until
/// dropped, when the table it replaced comes back.
#[must_use = "the table is uninstalled when the guard drops"]
pub struct ClosureTraits {
    replaced: Option<HashMap<String, Ty>>,
}

impl ClosureTraits {
    /// Install an empty table on this thread.
    pub fn install() -> Self {
        Self {
            replaced: TRAITS.with(|traits| traits.borrow_mut().replace(HashMap::new())),
        }
    }
}

impl Drop for ClosureTraits {
    fn drop(&mut self) {
        let replaced = self.replaced.take();
        TRAITS.with(|traits| *traits.borrow_mut() = replaced);
    }
}

/// Declare that the storage struct `storage` prints as `contract`, its
/// closure trait, in the installed table; without one nothing is kept.
pub fn declare_closure_trait(storage: &str, contract: &Ty) {
    TRAITS.with(|traits| {
        if let Some(traits) = traits.borrow_mut().as_mut() {
            traits.insert(storage.to_string(), contract.clone());
        }
    });
}

/// The closure trait the storage struct `storage` was declared to print as.
pub fn closure_trait(storage: &str) -> Option<Ty> {
    TRAITS.with(|traits| traits.borrow().as_ref()?.get(storage).cloned())
}

thread_local! {
    static TRAITS: RefCell<Option<HashMap<String, Ty>>> = const { RefCell::new(None) };
}
