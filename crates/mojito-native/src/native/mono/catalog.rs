//! The declarations the elaborator looks up by name: the program's own,
//! indexed when the elaboration starts, then the attached package's,
//! resolved the first time a name reaches them, as upstream's elaborator
//! resolves an imported declaration on first lookup.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::link::ImportedDeclarations;
use std::collections::BTreeMap;
use std::ops::Bound;

/// One kind of declaration, by name: the program's own first, then the
/// package's.
pub(super) struct Catalog<'a, T> {
    own: BTreeMap<&'a str, &'a T>,
    imported: Option<&'a ImportedDeclarations>,
}

/// A declaration kind a package can hold.
pub(super) trait Declared: Sized {
    fn imported<'a>(imported: &'a ImportedDeclarations, name: &str) -> Option<&'a Self>;
}

impl Declared for MirFunctionDeclaration {
    fn imported<'a>(imported: &'a ImportedDeclarations, name: &str) -> Option<&'a Self> {
        imported.function(name)
    }
}

impl Declared for MirStructDeclaration {
    fn imported<'a>(imported: &'a ImportedDeclarations, name: &str) -> Option<&'a Self> {
        imported.structure(name)
    }
}

impl<'a, T: Declared> Catalog<'a, T> {
    pub(super) fn new(
        own: impl IntoIterator<Item = (&'a str, &'a T)>,
        imported: Option<&'a ImportedDeclarations>,
    ) -> Self {
        Self {
            own: own.into_iter().collect(),
            imported,
        }
    }

    /// The declaration `name`, the package's decoded on first use.
    pub(super) fn get(&self, name: &str) -> Option<&'a T> {
        self.own
            .get(name)
            .copied()
            .or_else(|| T::imported(self.imported?, name))
    }

    pub(super) fn contains_key(&self, name: &str) -> bool {
        self.get(name).is_some()
    }
}

impl<'a> Catalog<'a, MirFunctionDeclaration> {
    /// Every function name starting with `prefix`, own first, each in name
    /// order, decoding nothing: a range of each by-name table.
    pub(super) fn prefixed<'s>(&'s self, prefix: &'s str) -> impl Iterator<Item = &'a str> + 's {
        let own = self
            .own
            .range::<str, _>((Bound::Included(prefix), Bound::Unbounded))
            .map(|(name, _)| *name)
            .take_while(move |name| name.starts_with(prefix));
        let imported = self.imported.into_iter().flat_map(move |imported| {
            imported
                .package()
                .functions_prefixed(prefix)
                .map(|(name, _)| name)
        });
        own.chain(imported.filter(|name| !self.own.contains_key(name)))
    }
}

impl<T: Declared> std::ops::Index<&str> for Catalog<'_, T> {
    type Output = T;

    fn index(&self, name: &str) -> &T {
        self.get(name)
            .unwrap_or_else(|| panic!("`{name}` has a declaration"))
    }
}

/// The struct declarations substitution reads by name: the program's own,
/// indexed up front, then the attached package's, decoded the first time
/// a name reaches them.
#[derive(Default)]
pub(super) struct StructCatalog {
    own: HashMap<String, MirStructDeclaration>,
    imported: Option<ImportedDeclarations>,
}

impl StructCatalog {
    pub(super) fn new(declarations: &MirDeclarations) -> Self {
        Self {
            own: declarations
                .structs
                .iter()
                .map(|declaration| (declaration.name.clone(), declaration.clone()))
                .collect(),
            imported: declarations.imported.clone(),
        }
    }

    /// A catalog of the generic templates `names` alone, each declaring
    /// one type parameter and nothing else.
    #[cfg(test)]
    pub(super) fn generic(names: &[&str]) -> Self {
        Self {
            own: names
                .iter()
                .map(|name| {
                    (
                        (*name).to_string(),
                        MirStructDeclaration {
                            name: (*name).to_string(),
                            module: String::new(),
                            fields: Vec::new(),
                            ref_fields: Vec::new(),
                            fieldwise_init: false,
                            param_decls: vec![ParamDecl::Type {
                                id: mojito_types::param_expr::ParamId::new(name, 0),
                                name: "T".to_string(),
                                bounds: Vec::new(),
                                trait_param: None,
                                callable_bound: None,
                                default: None,
                                infer_only: false,
                                variadic: false,
                                constraints: Vec::new(),
                            }],
                            explicit_destructors: HashMap::new(),
                            explicit_destroy: None,
                            conformances: Vec::new(),
                            declared: Vec::new(),
                            register_passable: false,
                            associated_types: Vec::new(),
                            parameterized_associated_types: Vec::new(),
                            associated_values: Vec::new(),
                            parameterized_associated_values: Vec::new(),
                            members: Vec::new(),
                            witness_tables: Vec::new(),
                        },
                    )
                })
                .collect(),
            imported: None,
        }
    }

    /// The declaration of the struct `name`, a package's decoded on first
    /// use.
    pub(super) fn declaration(&self, name: &str) -> Option<&MirStructDeclaration> {
        self.own
            .get(name)
            .or_else(|| self.imported.as_ref()?.structure(name))
    }

    /// Join to `index` each non-generic struct `ty` reaches, through its
    /// own structure and its structs' fields, that `index` lacks.
    pub(super) fn fill_layout(&self, index: &RefCell<StructFieldIndex>, ty: &Ty) {
        let reached = mojito_mir::mir::reached_struct_fields(ty, |name| {
            index.borrow().get(name).map(<[Ty]>::to_vec).or_else(|| {
                self.declaration(name)
                    .filter(|declaration| declaration.argument_binders().next().is_none())
                    .map(|declaration| {
                        declaration
                            .fields
                            .iter()
                            .map(|(_, ty)| ty.clone())
                            .collect()
                    })
            })
        });
        let mut index = index.borrow_mut();
        for (name, fields) in reached {
            if index.get(&name).is_none() {
                index.insert(name, fields);
            }
        }
    }

    /// `arguments`, an instance of the struct `name`, with each origin's
    /// `mut=` argument closed: a known `Bool` stays, and any other slot is
    /// what `close` answers from the slot and the origin argument it
    /// governs (a member's `Self`, whose receiver bound the binder), or
    /// erased ([`CtValue::RemovedArg`]) where it answers nothing, as
    /// upstream's generator spells a parameter it does not bind. An
    /// elaborated instance is keyed on the closed slot and on no origin.
    /// `arguments` as they stand when they hold no such slot.
    pub(super) fn closed_mutability<'a>(
        &self,
        name: &str,
        arguments: &'a [TyArg],
        close: impl Fn(&CtValue, Option<&mojito_types::origin::Origin>) -> Option<CtValue>,
    ) -> std::borrow::Cow<'a, [TyArg]> {
        let Some(declaration) = self.declaration(name).filter(|declaration| {
            declaration.param_decls.len() == arguments.len()
                && declaration
                    .param_decls
                    .iter()
                    .any(|decl| decl.is_origin_mutability_binder(&declaration.param_decls))
        }) else {
            return std::borrow::Cow::Borrowed(arguments);
        };
        let decls = &declaration.param_decls;
        let governed = |decl: &ParamDecl| {
            decls
                .iter()
                .zip(arguments)
                .find_map(|(candidate, argument)| match (candidate, argument) {
                    (
                        ParamDecl::Origin {
                            mutability: mojito_types::types::OriginMutability::Binder(binder),
                            ..
                        },
                        TyArg::Origin(origin),
                    ) if binder.id == *decl.id() => Some(origin),
                    _ => None,
                })
        };
        std::borrow::Cow::Owned(
            decls
                .iter()
                .zip(arguments)
                .map(|(decl, argument)| match argument {
                    TyArg::Val(value) if decl.is_origin_mutability_binder(decls) => {
                        TyArg::Val(match value {
                            CtValue::Bool(_) | CtValue::RemovedArg => value.clone(),
                            _ => close(value, governed(decl)).unwrap_or(CtValue::RemovedArg),
                        })
                    }
                    _ => argument.clone(),
                })
                .collect(),
        )
    }

    /// Whether the struct `name` is a generic template, which substitution
    /// renames a concrete application of to its instance symbol.
    pub(super) fn is_generic(&self, name: &str) -> bool {
        self.declaration(name).is_some_and(|declaration| {
            ParamDecl::instance_binders(&declaration.param_decls)
                .next()
                .is_some()
        })
    }
}
