//! The declarations the elaborator looks up by name: the program's own,
//! indexed when the elaboration starts, then the attached package's,
//! resolved the first time a name reaches them, as upstream's elaborator
//! resolves an imported declaration on first lookup.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::link::ImportedDeclarations;
use std::cell::OnceCell;
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

/// What substitution reads about a struct by name: whether it is a generic
/// template, and its shape. The program's own structs are indexed up
/// front; a package struct's shape is built the first time it is asked.
#[derive(Default)]
pub(super) struct StructCatalog {
    own: HashMap<String, StructShape>,
    imported: Option<ImportedDeclarations>,
    /// Each package struct's shape once asked, by package position.
    package_shapes: Vec<OnceCell<Option<StructShape>>>,
}

impl StructCatalog {
    pub(super) fn new(declarations: &MirDeclarations) -> Self {
        let imported = declarations.imported.clone();
        Self {
            own: declarations
                .structs
                .iter()
                .map(|declaration| (declaration.name.clone(), StructShape::of(declaration)))
                .collect(),
            package_shapes: imported.as_ref().map_or_else(Vec::new, |imported| {
                std::iter::repeat_with(OnceCell::new)
                    .take(imported.package().struct_count())
                    .collect()
            }),
            imported,
        }
    }

    /// A catalog of the generic templates `names` alone, each with no
    /// shape beyond its being generic.
    #[cfg(test)]
    pub(super) fn generic(names: &[&str]) -> Self {
        Self {
            own: names
                .iter()
                .map(|name| {
                    (
                        (*name).to_string(),
                        StructShape {
                            param_decls: vec![ParamDecl::Type {
                                id: mojito_types::param_expr::ParamId::new(name, 0),
                                name: "T".to_string(),
                                bounds: Vec::new(),
                                callable_bound: None,
                                default: None,
                                infer_only: false,
                                variadic: false,
                                constraints: Vec::new(),
                            }],
                            members: Vec::new(),
                            values: Vec::new(),
                            fields: Vec::new(),
                        },
                    )
                })
                .collect(),
            imported: None,
            package_shapes: Vec::new(),
        }
    }

    /// The shape of the struct `name`.
    pub(super) fn shape(&self, name: &str) -> Option<&StructShape> {
        if let Some(shape) = self.own.get(name) {
            return Some(shape);
        }
        let imported = self.imported.as_ref()?;
        let position = imported.package().struct_position(name)?;
        self.package_shapes[position]
            .get_or_init(|| imported.structure(name).map(StructShape::of))
            .as_ref()
    }

    /// Join to `index` each non-generic struct `ty` reaches, through its
    /// own structure and its structs' fields, that `index` lacks.
    pub(super) fn fill_layout(&self, index: &RefCell<StructFieldIndex>, ty: &Ty) {
        let reached = mojito_mir::mir::reached_struct_fields(ty, |name| {
            index.borrow().get(name).map(<[Ty]>::to_vec).or_else(|| {
                self.shape(name)
                    .filter(|shape| shape.param_decls.is_empty())
                    .map(|shape| shape.fields.iter().map(|(_, ty)| ty.clone()).collect())
            })
        });
        let mut index = index.borrow_mut();
        for (name, fields) in reached {
            if index.get(&name).is_none() {
                index.insert(name, fields);
            }
        }
    }

    /// Whether the struct `name` is a generic template, which substitution
    /// renames a concrete application of to its instance symbol.
    pub(super) fn is_generic(&self, name: &str) -> bool {
        self.shape(name)
            .is_some_and(|shape| !shape.param_decls.is_empty())
    }
}

impl StructShape {
    fn of(declaration: &MirStructDeclaration) -> Self {
        Self {
            param_decls: declaration.param_decls.clone(),
            members: declaration.associated_types.clone(),
            values: declaration.associated_values.clone(),
            fields: declaration.fields.clone(),
        }
    }
}
