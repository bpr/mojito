//! The bodies an elaboration instantiates: the program's own, and an
//! imported package's, each lowered from its unit the first time the
//! elaboration reaches it, and both looked up by name or name prefix, as
//! upstream's symbol table finds a callee's overload set.
//!
//! Every body arrives with its structured control flow, as upstream's
//! generators keep `hlcf`; each is fetched as a template with that control
//! flow lowered to blocks (`lower-control-flow`), lowered once: the
//! program's own in a [`Templates`] arena that outlives the elaboration,
//! a package's by the package ([`PackageBodies::template`]).

use mojito_mir::mir::text::package::PackageBodies;
use mojito_mir::mir::{MirFunction, MirProgram, has_structured_control_flow, lower_control_flow};
use mojito_symbol::symbol::{CallableCandidate, CallableLookup, OV_SEP};
use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::ops::Bound;

/// The program's own bodies with their control flow lowered, each lowered
/// the first time the elaboration fetches it, by the body's position in
/// the program.
pub(super) struct Templates {
    lowered: Vec<OnceCell<MirFunction>>,
}

impl Templates {
    /// An empty arena for the bodies of `program`.
    pub(super) fn of(program: &MirProgram) -> Self {
        Self {
            lowered: program.functions.iter().map(|_| OnceCell::new()).collect(),
        }
    }

    /// The template of `function`, the program's body at `index`: itself
    /// when it holds no structured control flow, else its lowered copy.
    fn template<'a>(&'a self, index: usize, function: &'a MirFunction) -> &'a MirFunction {
        if !has_structured_control_flow(function) {
            return function;
        }
        self.lowered[index].get_or_init(|| {
            let mut template = function.clone();
            lower_control_flow(&mut template);
            template
        })
    }
}

/// Every function body an elaboration may instantiate, by name: the
/// program's own, each with its position in the program, shadowing the
/// package's.
pub(super) struct Bodies<'a> {
    resident: BTreeMap<&'a str, (usize, &'a MirFunction)>,
    package: Option<&'a PackageBodies>,
    templates: &'a Templates,
}

impl<'a> Bodies<'a> {
    /// The bodies of `program`, whose templates `templates` holds.
    pub(super) fn new(program: &'a MirProgram, templates: &'a Templates) -> Self {
        Self {
            resident: program
                .functions
                .iter()
                .enumerate()
                .map(|(index, (name, function))| (name.as_str(), (index, function)))
                .collect(),
            package: None,
            templates,
        }
    }

    /// These bodies with `package`'s behind them: a callee the program
    /// does not define is the package's.
    pub(super) const fn importing(mut self, package: &'a PackageBodies) -> Self {
        self.package = Some(package);
        self
    }

    /// The template of `name`: its body with its control flow lowered.
    pub(super) fn get(&self, name: &str) -> Option<&'a MirFunction> {
        self.resident
            .get(name)
            .map(|&(index, function)| self.templates.template(index, function))
            .or_else(|| self.package?.template(name))
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
