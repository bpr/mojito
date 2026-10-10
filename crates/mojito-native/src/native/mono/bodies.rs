//! The bodies an elaboration instantiates: the program's own, and an
//! imported package's, each decoded the first time the elaboration reaches
//! it.

use mojito_mir::mir::MirFunction;
use mojito_mir::mir::text::package::PackageBodies;
use mojito_symbol::symbol::CallableCandidate;
use std::collections::HashMap;

/// Every function body an elaboration may instantiate, by name.
pub(super) struct Bodies<'a> {
    resident: HashMap<&'a str, &'a MirFunction>,
    package: Option<&'a PackageBodies>,
}

impl<'a> Bodies<'a> {
    pub(super) fn new(
        resident: impl IntoIterator<Item = (&'a str, &'a MirFunction)>,
        package: Option<&'a PackageBodies>,
    ) -> Self {
        Self {
            resident: resident.into_iter().collect(),
            package,
        }
    }

    pub(super) fn get(&self, name: &str) -> Option<&'a MirFunction> {
        self.resident
            .get(name)
            .copied()
            .or_else(|| self.package?.get(name))
    }

    pub(super) fn contains_key(&self, name: &str) -> bool {
        self.resident.contains_key(name)
            || self.package.is_some_and(|package| package.contains(name))
    }

    /// Every function's name, the program's own first.
    pub(super) fn keys(&self) -> impl Iterator<Item = &'a str> + '_ {
        self.resident.keys().copied().chain(
            self.package
                .into_iter()
                .flat_map(|package| package.signatures().map(|(name, _)| name)),
        )
    }

    /// Every function as an overload-resolution candidate, by its name and
    /// runtime parameter count, without decoding a package body.
    pub(super) fn candidates(&self) -> impl Iterator<Item = CallableCandidate<'a>> + Clone + '_ {
        self.resident
            .iter()
            .map(|(name, function)| CallableCandidate {
                name,
                n_params: function.n_params,
            })
            .chain(self.package.into_iter().flat_map(|package| {
                package
                    .signatures()
                    .map(|(name, n_params)| CallableCandidate { name, n_params })
            }))
    }
}
