//! Lowering units: the pieces of a checked program that lower on their own.
//!
//! A unit is one declaration's checked syntax: a module `def`, one method a
//! struct spells, the wrapper of a trait default a struct inherits, a trait
//! default as its trait's member, or a module `comptime` constant whose
//! applications are lifted. It lowers to one function (none for a
//! constant) and every function lifted from it.

use crate::checked::{
    AnnotationSite, CheckedDefaultWrapper, CheckedProgram, CheckedTraitMember, ConformanceFacts,
    DeclarationEffect, DiscoveryResult, GenericSite, NodeFacts,
};
use crate::package::{LibraryRoot, ProgramConstants, ProgramOverloads};
use mojito_ast::ast::{Expr, Method, Stmt, StmtKind};
use mojito_common::token::{SourceSpan, Span, SyntaxId};
use mojito_types::origin::encoding_packaged_owners;
use mojito_types::types::{ParamDecl, Ty};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// One lowering unit of a checked program, borrowed from it.
#[derive(Debug, Clone, Copy)]
pub enum LoweringUnit<'a> {
    /// A module `def`.
    Function(&'a Stmt),
    /// The `method` at `index` of the struct `structure`.
    Method {
        structure: &'a Stmt,
        method: &'a Method,
        index: usize,
    },
    /// The wrapper of a trait default the struct `structure` inherits.
    Wrapper {
        structure: &'a Stmt,
        wrapper: &'a CheckedDefaultWrapper,
    },
    /// A trait default, as its trait's member.
    TraitMember(&'a CheckedTraitMember),
    /// A module `comptime` constant.
    Constant(&'a Stmt),
}

impl LoweringUnit<'_> {
    /// The module and name of the struct the unit is a member of, and its
    /// position among the struct's members.
    fn structure(&self) -> Option<(&Option<String>, &str, usize)> {
        let (structure, member) = match self {
            Self::Method {
                structure, index, ..
            } => (structure, *index),
            Self::Wrapper { structure, wrapper } => (structure, wrapper.method_index),
            _ => return None,
        };
        let StmtKind::Struct { name, .. } = &structure.kind else {
            return None;
        };
        Some((&structure.module, name, member))
    }
}

impl CheckedProgram {
    /// Every lowering unit of the program, in the order it lowers them: its
    /// statements' declarations in order, each struct's methods before its
    /// wrappers, and every module constant last.
    pub fn lowering_units(&self) -> Vec<LoweringUnit<'_>> {
        let mut units = Vec::new();
        let mut constants = Vec::new();
        for statement in self.statements() {
            match &statement.kind {
                StmtKind::Def { .. } => units.push(LoweringUnit::Function(statement)),
                StmtKind::Struct { name, methods, .. } => {
                    units.extend(methods.iter().enumerate().map(|(index, method)| {
                        LoweringUnit::Method {
                            structure: statement,
                            method,
                            index,
                        }
                    }));
                    units.extend(
                        self.default_wrappers()
                            .iter()
                            .filter(|wrapper| {
                                wrapper.declaration == *name && wrapper.module == statement.module
                            })
                            .map(|wrapper| LoweringUnit::Wrapper {
                                structure: statement,
                                wrapper,
                            }),
                    );
                }
                StmtKind::Trait { name, .. } => units.extend(
                    self.trait_members()
                        .iter()
                        .filter(|member| {
                            member.trait_name == *name && member.module == statement.module
                        })
                        .map(LoweringUnit::TraitMember),
                ),
                StmtKind::Comptime { .. } => constants.push(LoweringUnit::Constant(statement)),
                _ => {}
            }
        }
        units.extend(constants);
        units
    }
}

/// What kind of lowering unit a [`CheckedUnit`] holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum UnitKind {
    Function,
    /// A struct's method, at its position among the struct's members.
    Method(usize),
    Wrapper,
    TraitMember,
    Constant,
}

/// One lowering unit's checked syntax and every fact the check recorded
/// for it.
///
/// It is what a package keeps of a body, as upstream's keeps each body's
/// IR as its semantic checks left it, and what an importer lowers when it
/// first reaches the body.
///
/// Its syntax is the unit's declaration alone: a struct's member holds
/// its struct with no method but its own. Its facts are the check's
/// outputs keyed by that syntax (`DiscoveryResult`'s, before the checked
/// arena is built from them), one record per occurrence, so the unit's
/// arena is built from them as the whole program's is.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedUnit {
    pub kind: UnitKind,
    statements: Vec<Stmt>,
    default_wrappers: Vec<CheckedDefaultWrapper>,
    /// A trait member's declaration, whose syntax `statements` holds.
    member: Option<CheckedTraitMember>,
    facts: UnitFacts,
}

impl CheckedUnit {
    /// The unit's checked program, its overload and constant lookups
    /// answered by `overloads` and `constants`, and a member's struct's
    /// compile-time parameters by `struct_parameters`, from the struct's
    /// declaration.
    ///
    /// # Errors
    ///
    /// When a fact names an occurrence the unit's syntax does not hold.
    pub fn program(
        self,
        overloads: Arc<ProgramOverloads>,
        constants: Arc<ProgramConstants>,
        struct_parameters: &dyn Fn(&str) -> Option<Vec<ParamDecl>>,
    ) -> Result<UnitProgram, String> {
        let wanted = self
            .facts
            .nodes
            .iter()
            .filter(|record| record.location.is_none())
            .map(|record| record.occurrence)
            .collect();
        let locations = Locations::of(&self.statements, &self.default_wrappers, Some(&wanted));
        let structure = match (&self.kind, self.statements.first()) {
            (
                UnitKind::Method(_) | UnitKind::Wrapper,
                Some(Stmt {
                    kind: StmtKind::Struct { name, .. },
                    module,
                    ..
                }),
            ) => struct_parameters(name).map(|parameters| {
                (
                    GenericSite::Struct {
                        module: module.clone(),
                        declaration: name.clone(),
                    },
                    parameters,
                )
            }),
            _ => None,
        };
        let mut discovery = DiscoveryResult {
            statements: self.statements,
            node_facts: HashMap::with_capacity(self.facts.nodes.len()),
            struct_instantiations: Vec::new(),
            checked_types: self.facts.sites.checked_types.into_iter().collect(),
            generic_parameters: self.facts.sites.generic_parameters.into_iter().collect(),
            declaration_effects: self.facts.sites.declaration_effects.into_iter().collect(),
            explicit_destroy_types: HashMap::new(),
            struct_destructors: HashMap::new(),
            conformances: ConformanceFacts::default(),
            overloads,
            constants,
            default_wrappers: self.default_wrappers,
            trait_members: Vec::new(),
        };
        for record in self.facts.nodes {
            discovery
                .node_facts
                .insert(record.key(&locations)?, record.facts);
        }
        if let Some((site, parameters)) = structure {
            discovery.generic_parameters.insert(site, parameters);
        }
        Ok(UnitProgram {
            kind: self.kind,
            program: discovery.finalize(),
            member: self.member,
        })
    }

    /// The unit as package bytecode: its file table, which names each file
    /// relative to the library `root`, then the unit, whose syntax keeps
    /// its occurrence identities and binding owners.
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
    /// `root`. Its syntax draws fresh identities, one per identity the
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

    /// The syntax of `unit`, with no facts yet.
    fn syntax(unit: LoweringUnit<'_>) -> Self {
        let (kind, statements, default_wrappers, member) = match unit {
            LoweringUnit::Function(statement) => (
                UnitKind::Function,
                vec![statement.clone()],
                Vec::new(),
                None,
            ),
            LoweringUnit::Constant(statement) => (
                UnitKind::Constant,
                vec![statement.clone()],
                Vec::new(),
                None,
            ),
            LoweringUnit::Method {
                structure,
                method,
                index,
            } => (
                UnitKind::Method(index),
                vec![member_of(structure, Some(method))],
                Vec::new(),
                None,
            ),
            LoweringUnit::Wrapper { structure, wrapper } => (
                UnitKind::Wrapper,
                vec![member_of(structure, None)],
                vec![wrapper.clone()],
                None,
            ),
            LoweringUnit::TraitMember(member) => (
                UnitKind::TraitMember,
                vec![member.declaration.clone()],
                Vec::new(),
                Some(member.clone()),
            ),
        };
        Self {
            kind,
            statements,
            default_wrappers,
            member,
            facts: UnitFacts::default(),
        }
    }
}

/// A unit's own checked program, and the unit in it.
#[derive(Debug)]
pub struct UnitProgram {
    kind: UnitKind,
    program: CheckedProgram,
    member: Option<CheckedTraitMember>,
}

impl UnitProgram {
    pub const fn program(&self) -> &CheckedProgram {
        &self.program
    }

    /// The unit, borrowed from its program; `None` when the unit's syntax
    /// is not of its kind.
    pub fn unit(&self) -> Option<LoweringUnit<'_>> {
        let statement = self.program.statements().first();
        Some(match self.kind {
            UnitKind::Function => LoweringUnit::Function(statement?),
            UnitKind::Constant => LoweringUnit::Constant(statement?),
            UnitKind::TraitMember => LoweringUnit::TraitMember(self.member.as_ref()?),
            UnitKind::Method(index) => {
                let structure = statement?;
                let StmtKind::Struct { methods, .. } = &structure.kind else {
                    return None;
                };
                LoweringUnit::Method {
                    structure,
                    method: methods.first()?,
                    index,
                }
            }
            UnitKind::Wrapper => LoweringUnit::Wrapper {
                structure: statement?,
                wrapper: self.program.default_wrappers().first()?,
            },
        })
    }
}

impl DiscoveryResult {
    /// Each of `units`' checked syntax and facts, in order.
    ///
    /// A fact belongs to every unit whose syntax holds the occurrence its
    /// key names, or the occurrence that one was derived from, and a
    /// member's own sites to the member; a struct's own sites belong to
    /// none, since its declaration holds them.
    pub fn units(&self, units: &[LoweringUnit<'_>]) -> Vec<CheckedUnit> {
        let mut owners = Owners::default();
        let mut syntax = Vec::with_capacity(units.len());
        let mut sliced: Vec<CheckedUnit> = units
            .iter()
            .enumerate()
            .map(|(position, unit)| {
                let checked = CheckedUnit::syntax(*unit);
                let mut occurrences = Occurrences::default();
                mojito_ast::visit::walk_block(&mut occurrences, &checked.statements);
                for wrapper in &checked.default_wrappers {
                    mojito_ast::visit::walk_block(&mut occurrences, &wrapper.body);
                }
                self.scan_unit_expressions(
                    &checked.statements,
                    &checked.default_wrappers,
                    &mut |expression| occurrences.0.push(expression.syntax_id),
                );
                let mut ranks = HashMap::new();
                for occurrence in occurrences.0 {
                    let next = ranks.len();
                    ranks.entry(occurrence).or_insert(next);
                    let units = owners.syntax.entry(occurrence).or_default();
                    if units.last() != Some(&position) {
                        units.push(position);
                    }
                }
                syntax.push(UnitSyntax {
                    ranks,
                    locations: Locations::of(&checked.statements, &checked.default_wrappers, None),
                });
                if let Some((module, name, member)) = unit.structure() {
                    owners
                        .members
                        .entry((module.clone(), name.to_string(), member))
                        .or_default()
                        .push(position);
                }
                checked
            })
            .collect();
        let mut nodes: Vec<HashMap<SourceSpan, NodeFacts>> =
            sliced.iter().map(|_| HashMap::new()).collect();
        for (key, facts) in &self.node_facts {
            for unit in key.units(&owners) {
                nodes[*unit].insert(key.clone(), facts.clone());
            }
        }
        let checked_types = shares(&self.checked_types, &owners, &syntax);
        let generic_parameters = shares(&self.generic_parameters, &owners, &syntax);
        let declaration_effects = shares(&self.declaration_effects, &owners, &syntax);
        for ((((unit, nodes), syntax), checked_types), (generic_parameters, declaration_effects)) in
            sliced
                .iter_mut()
                .zip(nodes)
                .zip(&syntax)
                .zip(checked_types)
                .zip(generic_parameters.into_iter().zip(declaration_effects))
        {
            unit.facts = UnitFacts {
                nodes: NodeRecord::ordered(nodes, syntax),
                sites: SiteFacts {
                    checked_types,
                    generic_parameters,
                    declaration_effects,
                },
            };
        }
        sliced
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

impl UnitKey for SourceSpan {
    fn units<'o>(&self, owners: &'o Owners) -> &'o [usize] {
        self.syntax
            .map_or(&[][..], |syntax| owners.of_syntax(syntax))
    }
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

/// A unit's share of the check's facts: one record per occurrence the
/// check recorded any fact at, as an op holds its own attributes, and the
/// facts of its declaration sites.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct UnitFacts {
    nodes: Vec<NodeRecord>,
    sites: SiteFacts,
}

/// A unit's share of each of the check's tables keyed by a declaration
/// site, as a declaration's op holds its attributes.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct SiteFacts {
    checked_types: Vec<(AnnotationSite, Ty)>,
    generic_parameters: Vec<(GenericSite, Vec<ParamDecl>)>,
    declaration_effects: Vec<(AnnotationSite, DeclarationEffect)>,
}

/// Every fact the check recorded at one occurrence of a unit's syntax.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct NodeRecord {
    occurrence: SyntaxId,
    /// Where the facts were recorded, when that is not the location of the
    /// occurrence's node in the unit's syntax: a derived occurrence's, or
    /// one the check gave a node it spelled at another node's location.
    location: Option<SourceSpan>,
    facts: NodeFacts,
}

impl NodeRecord {
    /// The key the record's facts were recorded under.
    fn key(&self, locations: &Locations) -> Result<SourceSpan, String> {
        let location = self
            .location
            .as_ref()
            .or_else(|| locations.get(self.occurrence))
            .ok_or("a fact names an occurrence its unit does not hold")?;
        Ok(SourceSpan::syntax(
            location.source.clone(),
            location.span,
            self.occurrence,
        ))
    }

    /// One record per key of `nodes`, in an order that does not depend on
    /// hashing, each naming its location only when `syntax` does not.
    fn ordered(nodes: HashMap<SourceSpan, NodeFacts>, syntax: &UnitSyntax) -> Vec<Self> {
        let mut keyed: Vec<_> = nodes
            .into_iter()
            .filter_map(|(key, facts)| {
                let occurrence = key.syntax?;
                let order = (
                    occurrence_order(occurrence, &syntax.ranks),
                    key.source.clone(),
                    key.span,
                );
                let location = key.without_syntax();
                let location =
                    (syntax.locations.get(occurrence) != Some(&location)).then_some(location);
                Some((
                    order,
                    Self {
                        occurrence,
                        location,
                        facts,
                    },
                ))
            })
            .collect();
        keyed.sort_by(|left, right| left.0.cmp(&right.0));
        keyed.into_iter().map(|(_, record)| record).collect()
    }
}

/// One lowering unit's syntax as [`UnitFacts::distribute`] reads it.
struct UnitSyntax {
    /// Each occurrence's position in the unit's walk.
    ranks: HashMap<SyntaxId, usize>,
    locations: Locations<'static>,
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

/// Where occurrences of a unit's syntax sit: the location of the first
/// node of the walk that has it, for each occurrence `wanted` names, or
/// every one.
struct Locations<'w> {
    wanted: Option<&'w HashSet<SyntaxId>>,
    found: HashMap<SyntaxId, SourceSpan>,
}

impl<'w> Locations<'w> {
    fn of(
        statements: &[Stmt],
        wrappers: &[CheckedDefaultWrapper],
        wanted: Option<&'w HashSet<SyntaxId>>,
    ) -> Self {
        let mut locations = Self {
            wanted,
            found: HashMap::new(),
        };
        mojito_ast::visit::walk_block(&mut locations, statements);
        for wrapper in wrappers {
            mojito_ast::visit::walk_block(&mut locations, &wrapper.body);
        }
        locations
    }

    fn get(&self, syntax: SyntaxId) -> Option<&SourceSpan> {
        self.found.get(&syntax)
    }

    fn record(&mut self, syntax: SyntaxId, source: Option<&String>, span: Span) {
        if self.wanted.is_none_or(|wanted| wanted.contains(&syntax)) {
            self.found
                .entry(syntax)
                .or_insert_with(|| SourceSpan::new(source.cloned(), span));
        }
    }
}

impl mojito_ast::visit::Visitor for Locations<'_> {
    fn visit_stmt(&mut self, statement: &Stmt) {
        self.record(
            statement.syntax_id,
            statement.module.as_ref(),
            statement.span,
        );
    }

    fn visit_expr(&mut self, expression: &Expr) {
        self.record(
            expression.syntax_id,
            expression.source.as_ref(),
            expression.span,
        );
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

/// Every syntax occurrence a walk reaches.
#[derive(Default)]
struct Occurrences(Vec<SyntaxId>);

impl mojito_ast::visit::Visitor for Occurrences {
    fn visit_stmt(&mut self, statement: &Stmt) {
        self.0.push(statement.syntax_id);
    }

    fn visit_expr(&mut self, expression: &Expr) {
        self.0.push(expression.syntax_id);
    }
}

/// The struct `structure` as a member of it lowers in it: its name and
/// compile-time parameters, holding no method but `method`.
fn member_of(structure: &Stmt, method: Option<&Method>) -> Stmt {
    let StmtKind::Struct {
        name, type_params, ..
    } = &structure.kind
    else {
        return structure.clone();
    };
    Stmt {
        kind: StmtKind::Struct {
            name: name.clone(),
            decorators: Vec::new(),
            type_params: type_params.clone(),
            conforms: Vec::new(),
            callable_conformance: None,
            conformance_conditions: Vec::new(),
            where_clauses: Vec::new(),
            fields: Vec::new(),
            associated: Vec::new(),
            methods: method.cloned().into_iter().collect(),
            fieldwise_init: false,
        },
        span: structure.span,
        module: structure.module.clone(),
        syntax_id: structure.syntax_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mojito_ast::ast::ExprKind;

    #[test]
    fn a_unit_decodes_under_another_root() {
        let mut statement = Stmt::new(
            StmtKind::Expr(Expr::new(ExprKind::Bool(true), (4, 8))),
            (4, 8),
        );
        mojito_ast::ast::stamp_source(std::slice::from_mut(&mut statement), "/a/stdlib/std/x.mojo");
        let unit = CheckedUnit::syntax(LoweringUnit::Function(&statement));
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
        assert_eq!(decoded.statements.len(), 1);
        let statement = &decoded.statements[0];
        assert_eq!(statement.module.as_deref(), Some("/b/stdlib/std/x.mojo"));
        let source = match &statement.kind {
            StmtKind::Expr(expression) => expression.source.as_deref(),
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
