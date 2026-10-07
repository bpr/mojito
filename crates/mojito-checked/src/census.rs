//! The instantiation census: which mechanism instantiates each generic body
//! of one compilation.
//!
//! The AST cloner reports the bodies it mints by class, the checker reports
//! which of those it inferred and which it derived from a checked template,
//! and the driver adds what the parametric bodies kept in MIR serve with no
//! clone. `docs/parametric-mir-plan.md` orders its stages by these counts.

/// Why the AST cloner minted a body, as one class per body.
///
/// A body that several classes describe takes the one whose stage of the
/// plan lands last, since the cloner keeps it until then: a `def` clone is
/// tested in the order `PackDef`, `ComptimeForDef`, `ComptimeIfDef`,
/// `ValueDef`, `TypeDef`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CloneClass {
    /// A `def` clone with only type arguments baked in and no compile-time
    /// control flow in its template: an explicit application.
    TypeDef,
    /// A `def` clone whose template holds a `comptime if`.
    ComptimeIfDef,
    /// A `def` clone whose template holds a `comptime for`, with no pack.
    ComptimeForDef,
    /// A `def` clone that expands a type pack.
    PackDef,
    /// A `def` clone keyed by any other value, with no compile-time control
    /// flow in its template.
    ValueDef,
    /// A per-instantiation method clone whose template body holds no
    /// compile-time control flow.
    InstanceMethod,
    /// A per-instantiation method clone whose template body holds a
    /// `comptime if` or a `comptime for`.
    InstanceMethodComptime,
    /// A per-call method clone, for the method's own compile-time parameters.
    PerCallMethod,
    /// A method clone minted for a compile-time evaluation's own
    /// subprogram.
    Ctfe,
}

impl CloneClass {
    pub const ALL: [Self; 9] = [
        Self::TypeDef,
        Self::ComptimeIfDef,
        Self::ComptimeForDef,
        Self::PackDef,
        Self::ValueDef,
        Self::InstanceMethod,
        Self::InstanceMethodComptime,
        Self::PerCallMethod,
        Self::Ctfe,
    ];

    /// The class's `--timings` counter.
    pub const fn counter(self) -> &'static str {
        match self {
            Self::TypeDef => "instantiation.cloned.def_type_arguments",
            Self::ComptimeIfDef => "instantiation.cloned.def_comptime_if",
            Self::ComptimeForDef => "instantiation.cloned.def_comptime_for",
            Self::PackDef => "instantiation.cloned.def_pack",
            Self::ValueDef => "instantiation.cloned.def_value",
            Self::InstanceMethod => "instantiation.cloned.method_per_instantiation",
            Self::InstanceMethodComptime => {
                "instantiation.cloned.method_per_instantiation_comptime"
            }
            Self::PerCallMethod => "instantiation.cloned.method_per_call",
            Self::Ctfe => "instantiation.cloned.ctfe",
        }
    }
}

/// The bodies one elaboration minted, by class, and the source names the
/// ones that reach MIR lower under.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CloneCensus {
    counts: std::collections::BTreeMap<CloneClass, usize>,
    names: std::collections::BTreeSet<String>,
}

impl CloneCensus {
    pub fn add(&mut self, class: CloneClass, bodies: usize) {
        if bodies > 0 {
            *self.counts.entry(class).or_default() += bodies;
        }
    }

    /// Record the source name a minted body lowers under: a `def` clone's
    /// name, or a method's `Struct.method`.
    pub fn name(&mut self, source_name: String) {
        self.names.insert(source_name);
    }

    /// Whether the cloner minted the body lowered as `symbol`: one of the
    /// recorded names, or one extended by a `$` suffix, which is how an
    /// overload qualifier and a body lifted out of a clone are spelled.
    pub fn minted(&self, symbol: &str) -> bool {
        self.names.iter().any(|name| {
            symbol
                .strip_prefix(name.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('$'))
        })
    }

    pub fn count(&self, class: CloneClass) -> usize {
        self.counts.get(&class).copied().unwrap_or_default()
    }

    pub fn total(&self) -> usize {
        self.counts.values().sum()
    }
}

/// Which mechanism instantiated each generic body of one compilation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstantiationCensus {
    /// The bodies the AST cloner minted for the converged program.
    pub cloned: CloneCensus,
    /// Distinct cloned bodies the checker inferred, over every discovery
    /// round and every compile-time evaluation's subprogram.
    pub inferred: usize,
    /// Distinct cloned bodies the checker served from a checked template and
    /// never inferred, over the same rounds.
    pub derived: usize,
    /// Parametric bodies in the converged MIR that no clone replaces: the
    /// templates, which run erased on the VM's oracle path.
    pub erased_bodies: usize,
    /// The erased bodies `main` reaches, and the concrete instances
    /// monomorphization substitutes from them. `None` when the program has
    /// no `main` or monomorphization refuses it.
    pub erased_served: Option<ErasedServed>,
    /// Cloned bodies that are still parametric in MIR: a clone that keeps a
    /// parameter of its own (`Variant$t2[…].write_to`, generic in its writer).
    /// Each is counted in `cloned` and in no erased row.
    pub parametric_clones: usize,
    /// What `main` reaches of the parametric clones, as `erased_served` is
    /// for the erased bodies.
    pub parametric_clones_served: Option<ErasedServed>,
}

/// What a set of parametric bodies serves: how many of them `main` reaches,
/// and the concrete instances substituted from those.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErasedServed {
    pub bodies: usize,
    pub instances: usize,
}
