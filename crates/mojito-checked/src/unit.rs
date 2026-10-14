//! Lowering units: the pieces of a checked program that lower on their own.
//!
//! A unit is one declaration's checked syntax: a module `def`, one method a
//! struct spells, the wrapper of a trait default a struct inherits, a trait
//! default as its trait's member, or a module `comptime` constant whose
//! applications are lifted. It lowers to one function (none for a
//! constant) and every function lifted from it.

use crate::checked::{
    AnnotationSite, CheckedDefaultWrapper, CheckedProgram, CheckedStmtId, CheckedStmtOp,
    CheckedTables, CheckedTraitMember, DeclarationEffect, GenericSite, InstalledSites, StructShell,
    WalkResult, Walked,
};
use crate::package::{LibraryRoot, ProgramConstants, ProgramOverloads};
use mojito_common::token::SyntaxId;
use mojito_types::origin::encoding_packaged_owners;
use mojito_types::types::{ParamDecl, Ty};
use std::collections::HashMap;
use std::sync::Arc;

/// One lowering unit of a checked program, borrowed from it: the nodes of
/// one declaration, as upstream's unit is an op.
#[derive(Debug, Clone, Copy)]
pub enum LoweringUnit<'a> {
    /// A module `def`, and its statement node.
    Function { root: CheckedStmtId },
    /// The method at `index` among the members of the struct whose
    /// statement node is `root`; `member` is the method's position among
    /// its node's methods.
    Method {
        index: usize,
        root: CheckedStmtId,
        member: usize,
    },
    /// The wrapper of a trait default the struct whose statement node is
    /// `root` inherits, at `index` among the program's default wrappers.
    Wrapper {
        root: CheckedStmtId,
        wrapper: &'a CheckedDefaultWrapper,
        index: usize,
    },
    /// A trait default, as its trait's member, and its own `def`'s
    /// statement node.
    TraitMember {
        member: &'a CheckedTraitMember,
        root: CheckedStmtId,
    },
    /// A module `comptime` constant, and its statement node.
    Constant { root: CheckedStmtId },
}

impl LoweringUnit<'_> {
    /// The module and name of the struct the unit is a member of, and its
    /// position among the struct's members, read from its node in
    /// `tables`.
    fn structure<'t>(
        &self,
        tables: &'t CheckedTables,
    ) -> Option<(&'t Option<String>, &'t str, usize)> {
        let (root, member) = match self {
            Self::Method { root, index, .. } => (root, *index),
            Self::Wrapper { root, wrapper, .. } => (root, wrapper.method_index),
            _ => return None,
        };
        let node = tables.statement(*root)?;
        let CheckedStmtOp::Struct(declaration) = &node.op else {
            return None;
        };
        Some((&node.module, &declaration.name, member))
    }
}

impl CheckedProgram {
    /// Every lowering unit of the program, in the order it lowers them: its
    /// statements' declarations in order, each struct's methods before its
    /// wrappers, and every module constant last.
    pub fn lowering_units(&self) -> Vec<LoweringUnit<'_>> {
        let mut units = Vec::new();
        let mut constants = Vec::new();
        for root in self.tables().roots() {
            let Some(node) = self.tables().statement(*root) else {
                continue;
            };
            let root = *root;
            match &node.op {
                CheckedStmtOp::Def(_) => units.push(LoweringUnit::Function { root }),
                CheckedStmtOp::Struct(declaration) => {
                    let name = &declaration.name;
                    units.extend((0..declaration.methods.len()).map(|index| {
                        LoweringUnit::Method {
                            index,
                            root,
                            member: index,
                        }
                    }));
                    units.extend(
                        self.default_wrappers()
                            .iter()
                            .enumerate()
                            .filter(|(_, wrapper)| {
                                wrapper.declaration == *name && wrapper.module == node.module
                            })
                            .map(|(index, wrapper)| LoweringUnit::Wrapper {
                                root,
                                wrapper,
                                index,
                            }),
                    );
                }
                CheckedStmtOp::Trait(declaration) => units.extend(
                    self.trait_members()
                        .iter()
                        .filter(|member| {
                            (&member.trait_name, &member.module)
                                == (&declaration.name, &node.module)
                        })
                        .filter_map(|member| {
                            Some(LoweringUnit::TraitMember {
                                member,
                                root: declaration.methods.get(member.method_index)?.default?,
                            })
                        }),
                ),
                CheckedStmtOp::Comptime { .. } => constants.push(LoweringUnit::Constant { root }),
                _ => {}
            }
        }
        units.extend(constants);
        units
    }

    /// Each lowering unit's checked nodes and site facts, in
    /// [`Self::lowering_units`]' order.
    ///
    /// A unit's nodes are its subtree of the program's tables. A
    /// site fact belongs to every unit whose nodes hold the occurrence its
    /// key names, or the occurrence that one was derived from, and a
    /// member's own sites to the member; a struct's own sites belong to
    /// none, since its declaration holds them.
    ///
    /// # Errors
    ///
    /// When a unit names a node the program's tables do not hold.
    pub fn units(&self) -> Result<Vec<CheckedUnit>, String> {
        let tables = self.tables();
        let units = self.lowering_units();
        let mut owners = Owners::default();
        let mut syntax = Vec::with_capacity(units.len());
        let mut sliced: Vec<CheckedUnit> = units
            .iter()
            .enumerate()
            .map(|(position, unit)| {
                let checked = CheckedUnit::extracted(unit, tables);
                let nodes = &checked.tables;
                if nodes.roots().is_empty() || nodes.statements.is_empty() {
                    return Err("a lowering unit names no statement of the program".to_string());
                }
                let mut occurrences = Vec::new();
                let mut visit = |node: Walked<'_>| {
                    occurrences.push(match node {
                        Walked::Statement(statement) => statement.syntax_id,
                        Walked::Expression(expression) => expression.syntax_id,
                    });
                    WalkResult::Advance
                };
                nodes.walk(nodes.roots(), &mut visit);
                nodes.walk(nodes.wrapper_roots(0), &mut visit);
                let mut ranks = HashMap::new();
                for occurrence in occurrences {
                    let next = ranks.len();
                    ranks.entry(occurrence).or_insert(next);
                    let units = owners.syntax.entry(occurrence).or_default();
                    if units.last() != Some(&position) {
                        units.push(position);
                    }
                }
                syntax.push(UnitSyntax { ranks });
                if let Some((module, name, member)) = unit.structure(tables) {
                    owners
                        .members
                        .entry((module.clone(), name.to_string(), member))
                        .or_default()
                        .push(position);
                }
                Ok(checked)
            })
            .collect::<Result<_, String>>()?;
        let checked_types = shares(self.checked_types(), &owners, &syntax);
        let generic_parameters = shares(self.generic_parameters(), &owners, &syntax);
        let declaration_effects = shares(self.declaration_effects(), &owners, &syntax);
        for ((unit, checked_types), (generic_parameters, declaration_effects)) in sliced
            .iter_mut()
            .zip(checked_types)
            .zip(generic_parameters.into_iter().zip(declaration_effects))
        {
            unit.sites = SiteFacts {
                checked_types,
                generic_parameters,
                declaration_effects,
            };
        }
        Ok(sliced)
    }
}

/// What kind of lowering unit a [`CheckedUnit`] holds.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum UnitKind {
    Function,
    /// A struct's method, at its position among the struct's members.
    Method {
        index: usize,
    },
    Wrapper,
    TraitMember,
    Constant,
}

/// One lowering unit's checked nodes and the facts of its declaration
/// sites.
///
/// It is what a package keeps of a body, as upstream's keeps each body's
/// IR as its semantic checks left it, each op carrying its own type and
/// attributes, and what an importer lowers when it first reaches the body.
///
/// Its nodes are the unit's declaration alone: a struct's member holds a
/// shell of its struct, its name and no method but its own. The struct's
/// compile-time parameters are its declaration's.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct CheckedUnit {
    kind: UnitKind,
    /// A wrapper's declaration, whose body `tables` holds.
    wrapper: Option<CheckedDefaultWrapper>,
    /// A trait member's declaration, whose syntax `tables` holds.
    trait_member: Option<CheckedTraitMember>,
    tables: CheckedTables,
    sites: SiteFacts,
}

impl CheckedUnit {
    /// The unit's checked program, its overload and constant lookups
    /// answered by `overloads` and `constants`, and a member's struct's
    /// compile-time parameters by `struct_parameters`, from the struct's
    /// declaration.
    pub fn program(
        self,
        overloads: Arc<ProgramOverloads>,
        constants: Arc<ProgramConstants>,
        struct_parameters: &dyn Fn(&str) -> Option<Vec<ParamDecl>>,
    ) -> UnitProgram {
        let mut sites = InstalledSites {
            checked_types: self.sites.checked_types.into_iter().collect(),
            generic_parameters: self.sites.generic_parameters.into_iter().collect(),
            declaration_effects: self.sites.declaration_effects.into_iter().collect(),
        };
        if matches!(self.kind, UnitKind::Method { .. } | UnitKind::Wrapper)
            && let Some(structure) = self
                .tables
                .roots()
                .first()
                .and_then(|root| self.tables.statement(*root))
            && let CheckedStmtOp::Struct(declaration) = &structure.op
        {
            let site = GenericSite::Struct {
                module: structure.module.clone(),
                declaration: declaration.name.clone(),
            };
            if let Some(parameters) = struct_parameters(&declaration.name) {
                sites.generic_parameters.insert(site, parameters);
            }
        }
        UnitProgram {
            kind: self.kind,
            program: CheckedProgram::installed(
                self.tables,
                sites,
                overloads,
                constants,
                self.wrapper,
                self.trait_member,
            ),
        }
    }

    /// The unit as package bytecode: its file table, which names each file
    /// relative to the library `root`, then the unit, whose nodes keep
    /// their occurrence identities and binding owners.
    ///
    /// # Errors
    ///
    /// When a fact does not encode, or the unit names a file outside
    /// `root`.
    pub fn encode(&self, root: &LibraryRoot) -> Result<Vec<u8>, String> {
        let (bytes, files) = mojito_common::token::encoding_occurrences(|| {
            encoding_packaged_owners(|| postcard::to_stdvec(self))
        });
        let bytes = bytes.map_err(|error| error.to_string())?;
        let files = files
            .into_iter()
            .map(|file| file.map(|file| root.relative(&file)).transpose())
            .collect::<Result<Vec<_>, _>>()?;
        postcard::to_stdvec(&(files, bytes)).map_err(|error| error.to_string())
    }

    /// Decode a unit [`Self::encode`] wrote, its files under the library
    /// `root`. Its nodes draw fresh identities, one per identity the
    /// encoding held.
    ///
    /// # Errors
    ///
    /// When the bytes do not decode.
    pub fn decode(bytes: &[u8], root: &LibraryRoot) -> Result<Self, String> {
        let (files, bytes): (Vec<Option<String>>, Vec<u8>) =
            postcard::from_bytes(bytes).map_err(|error| error.to_string())?;
        let files = files
            .into_iter()
            .map(|file| file.map(|file| root.join(&file)))
            .collect();
        mojito_common::token::decoding_occurrences(files, || {
            encoding_packaged_owners(|| postcard::from_bytes(&bytes))
        })
        .map_err(|error| error.to_string())
    }

    /// The unit `unit` of the program whose nodes are `tables`, with no
    /// site facts yet: its nodes are its subtree of `tables`.
    fn extracted(unit: &LoweringUnit<'_>, tables: &CheckedTables) -> Self {
        let mut wrapper = None;
        let mut trait_member = None;
        let (kind, root, shell, body) = match unit {
            LoweringUnit::Function { root } => (UnitKind::Function, *root, None, &[][..]),
            LoweringUnit::Constant { root } => (UnitKind::Constant, *root, None, &[][..]),
            LoweringUnit::Method {
                index,
                root,
                member,
            } => (
                UnitKind::Method { index: *index },
                *root,
                Some(StructShell::Method(*member)),
                &[][..],
            ),
            LoweringUnit::Wrapper {
                root,
                wrapper: inherited,
                index,
            } => {
                wrapper = Some((*inherited).clone());
                (
                    UnitKind::Wrapper,
                    *root,
                    Some(StructShell::Wrapper),
                    tables.wrapper_roots(*index),
                )
            }
            LoweringUnit::TraitMember { member, root } => {
                trait_member = Some((*member).clone());
                (UnitKind::TraitMember, *root, None, &[][..])
            }
        };
        Self {
            kind,
            wrapper,
            trait_member,
            tables: tables.extract(root, shell, body),
            sites: SiteFacts::default(),
        }
    }
}

/// A unit's own checked program, and the unit in it.
#[derive(Debug)]
pub struct UnitProgram {
    kind: UnitKind,
    program: CheckedProgram,
}

impl UnitProgram {
    pub const fn program(&self) -> &CheckedProgram {
        &self.program
    }

    /// The unit, borrowed from its program; `None` when the unit's nodes
    /// are not of its kind.
    pub fn unit(&self) -> Option<LoweringUnit<'_>> {
        let tables = self.program.tables();
        let root = *tables.roots().first()?;
        Some(match &self.kind {
            UnitKind::Function => LoweringUnit::Function { root },
            UnitKind::Constant => LoweringUnit::Constant { root },
            UnitKind::TraitMember => LoweringUnit::TraitMember {
                member: self.program.trait_members().first()?,
                root,
            },
            UnitKind::Method { index } => {
                let CheckedStmtOp::Struct(declaration) = &tables.statement(root)?.op else {
                    return None;
                };
                declaration.methods.first()?;
                LoweringUnit::Method {
                    index: *index,
                    root,
                    member: 0,
                }
            }
            UnitKind::Wrapper => LoweringUnit::Wrapper {
                root,
                wrapper: self.program.default_wrappers().first()?,
                index: 0,
            },
        })
    }
}

/// The units each fact key belongs to.
#[derive(Default)]
struct Owners {
    syntax: HashMap<SyntaxId, Vec<usize>>,
    /// By struct member.
    members: HashMap<(Option<String>, String, usize), Vec<usize>>,
}

impl Owners {
    /// The units holding `syntax`, or the occurrence it was derived from.
    fn of_syntax(&self, syntax: SyntaxId) -> &[usize] {
        let mut current = syntax;
        loop {
            if let Some(units) = self.syntax.get(&current) {
                return units;
            }
            match current.derivation() {
                Some((parent, _)) => current = parent,
                None => return &[],
            }
        }
    }
}

/// A fact key: which units its fact belongs to.
trait UnitKey {
    fn units<'o>(&self, owners: &'o Owners) -> &'o [usize];
}

impl UnitKey for AnnotationSite {
    fn units<'o>(&self, owners: &'o Owners) -> &'o [usize] {
        let units = match self {
            Self::FunctionParam { syntax, .. }
            | Self::FunctionReturn { syntax, .. }
            | Self::FunctionType { syntax, .. } => return owners.of_syntax(*syntax),
            // A field's type is its struct's declaration's, which a
            // package's declaration record holds.
            Self::StructField { .. } => None,
            Self::MethodParam {
                module,
                declaration,
                method,
                ..
            }
            | Self::MethodReturn {
                module,
                declaration,
                method,
            } => owners
                .members
                .get(&(module.clone(), declaration.clone(), *method)),
        };
        units.map_or(&[][..], Vec::as_slice)
    }
}

impl UnitKey for GenericSite {
    fn units<'o>(&self, owners: &'o Owners) -> &'o [usize] {
        let units = match self {
            Self::Function { syntax, .. } => return owners.of_syntax(*syntax),
            // A struct's parameters are its declaration's, which a
            // package's declaration record holds and a member's unit is
            // given when it lowers.
            Self::Struct { .. } => None,
            Self::Method {
                module,
                declaration,
                method,
            } => owners
                .members
                .get(&(module.clone(), declaration.clone(), *method)),
        };
        units.map_or(&[][..], Vec::as_slice)
    }
}

/// A unit's share of each of the check's tables keyed by a declaration
/// site, as a declaration's op holds its attributes.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct SiteFacts {
    checked_types: Vec<(AnnotationSite, Ty)>,
    generic_parameters: Vec<(GenericSite, Vec<ParamDecl>)>,
    declaration_effects: Vec<(AnnotationSite, DeclarationEffect)>,
}

/// One lowering unit's syntax as [`CheckedProgram::units`] orders its
/// site facts by it.
struct UnitSyntax {
    /// Each occurrence's position in the unit's walk.
    ranks: HashMap<SyntaxId, usize>,
}

/// The order a fact keyed by `syntax` takes among a unit's: by where the
/// occurrence it was derived from sits in the unit's walk (`ranks`), then
/// by its derivation.
fn occurrence_order(syntax: SyntaxId, ranks: &HashMap<SyntaxId, usize>) -> (usize, Vec<u32>) {
    let mut ordinals = Vec::new();
    let mut current = syntax;
    loop {
        if let Some(rank) = ranks.get(&current) {
            ordinals.reverse();
            return (*rank, ordinals);
        }
        match current.derivation() {
            Some((parent, ordinal)) => {
                ordinals.push(ordinal);
                current = parent;
            }
            None => return (usize::MAX, ordinals),
        }
    }
}

/// A site fact's key: the occurrence it names, if any, orders it.
trait SiteKey: UnitKey + std::fmt::Debug {
    fn syntax(&self) -> Option<SyntaxId>;
}

impl SiteKey for AnnotationSite {
    fn syntax(&self) -> Option<SyntaxId> {
        match self {
            Self::FunctionParam { syntax, .. }
            | Self::FunctionReturn { syntax, .. }
            | Self::FunctionType { syntax, .. } => Some(*syntax),
            Self::StructField { .. } | Self::MethodParam { .. } | Self::MethodReturn { .. } => None,
        }
    }
}

impl SiteKey for GenericSite {
    fn syntax(&self) -> Option<SyntaxId> {
        match self {
            Self::Function { syntax, .. } => Some(*syntax),
            Self::Struct { .. } | Self::Method { .. } => None,
        }
    }
}

/// `entries` in an order that does not depend on hashing.
fn ordered_sites<K: SiteKey, V>(
    entries: Vec<(K, V)>,
    ranks: &HashMap<SyntaxId, usize>,
) -> Vec<(K, V)> {
    let mut keyed: Vec<_> = entries
        .into_iter()
        .map(|(key, value)| {
            let order = (
                key.syntax().map(|syntax| occurrence_order(syntax, ranks)),
                format!("{key:?}"),
            );
            (order, key, value)
        })
        .collect();
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    keyed
        .into_iter()
        .map(|(_, key, value)| (key, value))
        .collect()
}

/// Each unit's share of the site table `table`, in an order that does not
/// depend on hashing.
fn shares<K: SiteKey + Clone, V: Clone>(
    table: &HashMap<K, V>,
    owners: &Owners,
    syntax: &[UnitSyntax],
) -> Vec<Vec<(K, V)>> {
    let mut shares: Vec<Vec<(K, V)>> = syntax.iter().map(|_| Vec::new()).collect();
    for (key, value) in table {
        for unit in key.units(owners) {
            shares[*unit].push((key.clone(), value.clone()));
        }
    }
    shares
        .into_iter()
        .zip(syntax)
        .map(|(share, syntax)| ordered_sites(share, &syntax.ranks))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unit_decodes_under_another_root() {
        use crate::checked::{
            CheckedExpr, CheckedNodeId, CheckedOp, CheckedStmt, EffectFacts, ValueCategory,
        };
        use mojito_ast::ast::{Expr, ExprKind, Stmt, StmtKind};
        let mut statement = Stmt::new(
            StmtKind::Expr(Expr::new(ExprKind::Bool(true), (4, 8))),
            (4, 8),
        );
        mojito_ast::ast::stamp_source(std::slice::from_mut(&mut statement), "/a/stdlib/std/x.mojo");
        let StmtKind::Expr(expression) = &statement.kind else {
            return;
        };
        let mut program = CheckedTables::default();
        program.expressions.push(CheckedExpr {
            id: CheckedNodeId(0),
            op: CheckedOp::Bool(true),
            span: expression.span,
            source: expression.source.clone(),
            syntax_id: expression.syntax_id,
            ty: None,
            place_ty: None,
            binding_ty: None,
            category: ValueCategory::Value,
            binding: None,
            contextual_base: None,
            effects: EffectFacts::default(),
            adjustments: Vec::new(),
            comprehension_bindings: Vec::new(),
            call_transfers: Vec::new(),
        });
        program.statements.push(CheckedStmt {
            id: CheckedStmtId(0),
            op: CheckedStmtOp::Expr(CheckedNodeId(0)),
            span: statement.span,
            module: statement.module.clone(),
            syntax_id: statement.syntax_id,
            binding: None,
            binding_ty: None,
            declaration: None,
        });
        let unit = CheckedUnit::extracted(
            &LoweringUnit::Function {
                root: CheckedStmtId(0),
            },
            &program,
        );
        let built = LibraryRoot {
            stamped: "/a/stdlib".to_string(),
            canonical: "/a/stdlib".to_string(),
        };
        let bytes = unit.encode(&built).expect("encodes");
        assert!(
            !bytes.windows(2).any(|window| window == b"/a"),
            "the unit names its build's root"
        );
        let installed = LibraryRoot {
            stamped: "/b/stdlib".to_string(),
            canonical: "/b/stdlib".to_string(),
        };
        let decoded = CheckedUnit::decode(&bytes, &installed).expect("decodes");
        assert_eq!(decoded.tables.roots().len(), 1);
        let statement = decoded
            .tables
            .statement(decoded.tables.roots()[0])
            .expect("the root is a node");
        assert_eq!(statement.module.as_deref(), Some("/b/stdlib/std/x.mojo"));
        let source = match &statement.op {
            CheckedStmtOp::Expr(expression) => decoded
                .tables
                .expression(*expression)
                .and_then(|expression| expression.source.as_deref()),
            _ => None,
        };
        assert_eq!(source, Some("/b/stdlib/std/x.mojo"));
        let elsewhere = LibraryRoot {
            stamped: "/c/stdlib".to_string(),
            canonical: "/c/stdlib".to_string(),
        };
        assert!(unit.encode(&elsewhere).is_err());
    }
}
