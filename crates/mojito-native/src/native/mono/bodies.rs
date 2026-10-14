//! The bodies an elaboration instantiates: the program's own, and an
//! imported package's, each lowered from its unit the first time the
//! elaboration reaches it, and both looked up by name or name prefix, as
//! upstream's symbol table finds a callee's overload set.

use mojito_mir::mir::MirFunction;
use mojito_mir::mir::text::package::PackageBodies;
use mojito_symbol::symbol::{CallableCandidate, CallableLookup, OV_SEP};
use std::collections::BTreeMap;
use std::ops::Bound;

/// Every function body an elaboration may instantiate, by name: the
/// program's own, each with its position in the program, shadowing the
/// package's.
pub(super) struct Bodies<'a> {
    resident: BTreeMap<&'a str, (usize, &'a MirFunction)>,
    package: Option<&'a PackageBodies>,
}

impl<'a> Bodies<'a> {
    pub(super) fn new(
        resident: impl IntoIterator<Item = (&'a str, &'a MirFunction)>,
        package: Option<&'a PackageBodies>,
    ) -> Self {
        Self {
            resident: resident
                .into_iter()
                .enumerate()
                .map(|(index, (name, function))| (name, (index, function)))
                .collect(),
            package,
        }
    }

    pub(super) fn get(&self, name: &str) -> Option<&'a MirFunction> {
        self.resident
            .get(name)
            .map(|(_, function)| *function)
            .or_else(|| self.package?.get(name))
    }

    /// What made the package body of `name` unusable when its importer
    /// finished it; empty for the program's own, whose findings its driver
    /// refuses before the elaboration.
    pub(super) fn findings(&self, name: &str) -> &'a [String] {
        match self.package {
            Some(package) if !self.resident.contains_key(name) => package.findings(name),
            _ => &[],
        }
    }

    /// Why each package declaration the package body of `name` reached
    /// when its importer lowered and finished it does not decode; empty for
    /// the program's own, whose view its driver refuses before the
    /// elaboration.
    pub(super) fn reached(&self, name: &str) -> &'a [String] {
        match self.package {
            Some(package) if !self.resident.contains_key(name) => package.reached(name),
            _ => &[],
        }
    }

    pub(super) fn contains_key(&self, name: &str) -> bool {
        self.resident.contains_key(name)
            || self.package.is_some_and(|package| package.contains(name))
    }

    /// The position of `name` in output order: the program's own functions
    /// in program order, then the package's in lowering order.
    pub(super) fn position(&self, name: &str) -> Option<usize> {
        self.resident
            .get(name)
            .map(|(index, _)| *index)
            .or_else(|| self.package?.row(name).map(|row| self.resident.len() + row))
    }

    /// Every function whose name starts with `prefix`, by its name and
    /// runtime parameter count, the program's own first, decoding no
    /// package body.
    pub(super) fn prefixed(
        &self,
        prefix: String,
    ) -> impl Iterator<Item = CallableCandidate<'a>> + '_ {
        let package = self
            .package
            .map(|package| package.functions_prefixed(&prefix))
            .into_iter()
            .flatten()
            .filter(|(name, _)| !self.resident.contains_key(name))
            .map(|(name, n_params)| CallableCandidate { name, n_params });
        self.resident
            .range::<str, _>((Bound::Included(prefix.as_str()), Bound::Unbounded))
            .take_while(move |(name, _)| name.starts_with(prefix.as_str()))
            .map(|(name, (_, function))| CallableCandidate {
                name,
                n_params: function.n_params,
            })
            .chain(package)
    }

    /// The function `base` itself, when there is one, then its
    /// `base$ov$…` overloads.
    pub(super) fn family(&self, base: &str) -> Vec<String> {
        self.contains_key(base)
            .then(|| base.to_string())
            .into_iter()
            .chain(
                self.overloads(base)
                    .map(|candidate| candidate.name.to_string()),
            )
            .collect()
    }
}

impl CallableLookup for Bodies<'_> {
    fn contains(&self, name: &str) -> bool {
        self.contains_key(name)
    }

    fn overloads(&self, base: &str) -> impl Iterator<Item = CallableCandidate<'_>> {
        self.prefixed(format!("{base}{OV_SEP}"))
    }
}
