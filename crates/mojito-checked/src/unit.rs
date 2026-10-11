//! Lowering units: the pieces of a checked program that lower on their own.
//!
//! A unit is one declaration's checked syntax: a module `def`, one method a
//! struct spells, the wrapper of a trait default a struct inherits, a trait
//! default as its trait's member, or a module `comptime` constant whose
//! applications are lifted. It lowers to one function (none for a
//! constant) and every function lifted from it.

use crate::checked::{
    AnnotationSite, CheckedCallContract, CheckedCallTransfer, CheckedCapture,
    CheckedComprehensionBinding, CheckedDefaultWrapper, CheckedProgram, CheckedTraitMember,
    CheckedTupleUnpackElement, ConformanceFacts, DeclarationEffect, DiscoveryResult, EffectFacts,
    GenericInstantiation, GenericSite, InteriorInvalidation, IterationProtocol,
    MethodInstantiation, RebindAssertion, SemanticAdjustment,
};
use crate::fact_store::{FactMap, FactSet, FactVec};
use crate::package::{ProgramConstants, ProgramOverloads};
use mojito_ast::ast::{Expr, Method, Stmt, StmtKind};
use mojito_common::token::{SourceSpan, SyntaxId};
use mojito_types::origin::{OriginPlace, OwnerId, encoding_packaged_owners};
use mojito_types::types::{ParamDecl, SimdDtype, SimdWidth, SliceKind, Ty};
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
/// arena is built from them), so the unit's arena is built from them as
/// the whole program's is.
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
    /// answered by `overloads` and `constants`.
    pub fn program(
        self,
        overloads: Arc<ProgramOverloads>,
        constants: Arc<ProgramConstants>,
    ) -> UnitProgram {
        let mut discovery = DiscoveryResult {
            statements: self.statements,
            overload_targets: FactMap::default(),
            contextual_bases: FactMap::default(),
            generic_instantiations: FactMap::default(),
            method_instantiations: FactMap::default(),
            struct_instantiations: FactVec::default(),
            call_transfers: FactMap::default(),
            implicit_conversions: FactMap::default(),
            implicit_conversion_types: FactMap::default(),
            conversion_source_borrows: FactMap::default(),
            conversion_raises: FactMap::default(),
            checked_types: FactMap::default(),
            generic_parameters: FactMap::default(),
            expression_types: FactMap::default(),
            expression_bindings: FactMap::default(),
            statement_bindings: FactMap::default(),
            declaration_captures: FactMap::default(),
            comprehension_bindings: FactMap::default(),
            expression_place_types: FactMap::default(),
            binding_types: FactMap::default(),
            expression_effects: FactMap::default(),
            selected_calls: FactMap::default(),
            subscript_descriptors: FactMap::default(),
            iteration_protocols: FactMap::default(),
            simd_constructions: FactMap::default(),
            operation_adjustments: FactMap::default(),
            parameterized_method_calls: FactMap::default(),
            tuple_unpack_plans: FactMap::default(),
            interior_references: FactMap::default(),
            interior_invalidations: FactMap::default(),
            explicit_destroy_types: HashMap::new(),
            struct_destructors: HashMap::new(),
            conformances: ConformanceFacts::default(),
            explicit_destroy_calls: FactSet::default(),
            reference_value_uses: FactMap::default(),
            copy_place_value_uses: FactSet::default(),
            call_place_uses: FactSet::default(),
            borrowed_read_call_places: FactSet::default(),
            read_temporary_arguments: FactSet::default(),
            implicitly_copied_consuming_receivers: FactSet::default(),
            truthiness_conditions: FactSet::default(),
            rebind_assertions: FactMap::default(),
            declaration_effects: FactMap::default(),
            overloads,
            constants,
            default_wrappers: self.default_wrappers,
            trait_members: Vec::new(),
        };
        self.facts.install(&mut discovery);
        UnitProgram {
            kind: self.kind,
            program: discovery.finalize(),
            member: self.member,
        }
    }

    /// The unit as package bytecode: its file table, then the unit, whose
    /// syntax keeps its occurrence identities and binding owners.
    ///
    /// # Errors
    ///
    /// When a fact does not encode.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let (bytes, files) = mojito_common::token::encoding_occurrences(|| {
            encoding_packaged_owners(|| postcard::to_stdvec(self))
        });
        let bytes = bytes.map_err(|error| error.to_string())?;
        postcard::to_stdvec(&(files, bytes)).map_err(|error| error.to_string())
    }

    /// Decode a unit [`Self::encode`] wrote. Its syntax draws fresh
    /// identities, one per identity the encoding held.
    ///
    /// # Errors
    ///
    /// When the bytes do not decode.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let (files, bytes): (Vec<Option<String>>, Vec<u8>) =
            postcard::from_bytes(bytes).map_err(|error| error.to_string())?;
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
    /// key names, or the occurrence that one was derived from; a
    /// struct's compile-time parameters belong to every member of it, and
    /// a member's own sites to the member.
    pub fn units(&self, units: &[LoweringUnit<'_>]) -> Vec<CheckedUnit> {
        let mut owners = Owners::default();
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
                for syntax in occurrences.0 {
                    let units = owners.syntax.entry(syntax).or_default();
                    if units.last() != Some(&position) {
                        units.push(position);
                    }
                }
                if let Some((module, name, member)) = unit.structure() {
                    owners
                        .structs
                        .entry((module.clone(), name.to_string()))
                        .or_default()
                        .push(position);
                    owners
                        .members
                        .entry((module.clone(), name.to_string(), member))
                        .or_default()
                        .push(position);
                }
                checked
            })
            .collect();
        let mut facts: Vec<UnitFacts> = sliced.iter().map(|_| UnitFacts::default()).collect();
        UnitFacts::distribute(self, &owners, &mut facts);
        for (unit, facts) in sliced.iter_mut().zip(facts) {
            unit.facts = facts;
        }
        sliced
    }
}

/// The units each fact key belongs to.
#[derive(Default)]
struct Owners {
    syntax: HashMap<SyntaxId, Vec<usize>>,
    /// By struct.
    structs: HashMap<(Option<String>, String), Vec<usize>>,
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
            Self::Struct {
                module,
                declaration,
            } => owners.structs.get(&(module.clone(), declaration.clone())),
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

/// Every key of `entries` with its value, in an order that does not depend
/// on hashing.
fn sorted_entries<K: serde::Serialize + Clone, V: Clone>(
    entries: impl Iterator<Item = (K, V)>,
) -> Vec<(K, V)> {
    let mut keyed: Vec<_> = entries
        .map(|(key, value)| {
            let order = mojito_common::token::encoding_occurrences(|| {
                encoding_packaged_owners(|| postcard::to_stdvec(&key).unwrap_or_default())
            })
            .0;
            (order, key, value)
        })
        .collect();
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    keyed
        .into_iter()
        .map(|(_, key, value)| (key, value))
        .collect()
}

macro_rules! unit_facts {
    (
        maps { $($map:ident: $key:ty => $value:ty,)* }
        sets { $($set:ident,)* }
    ) => {
        /// A unit's share of each of the check's fact tables.
        #[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
        struct UnitFacts {
            $($map: Vec<($key, $value)>,)*
            $($set: Vec<SourceSpan>,)*
        }

        impl UnitFacts {
            /// Give each unit its share of `discovery`'s facts.
            fn distribute(discovery: &DiscoveryResult, owners: &Owners, facts: &mut [Self]) {
                $(
                    let mut shares: Vec<Vec<($key, $value)>> = facts.iter().map(|_| Vec::new()).collect();
                    for (key, value) in discovery.$map.iter() {
                        for unit in key.units(owners) {
                            shares[*unit].push((key.clone(), value.clone()));
                        }
                    }
                    for (unit, share) in facts.iter_mut().zip(shares) {
                        unit.$map = sorted_entries(share.into_iter());
                    }
                )*
                $(
                    let mut shares: Vec<Vec<(SourceSpan, ())>> = facts.iter().map(|_| Vec::new()).collect();
                    for key in discovery.$set.iter() {
                        for unit in key.units(owners) {
                            shares[*unit].push((key.clone(), ()));
                        }
                    }
                    for (unit, share) in facts.iter_mut().zip(shares) {
                        unit.$set = sorted_entries(share.into_iter())
                            .into_iter()
                            .map(|(key, ())| key)
                            .collect();
                    }
                )*
            }

            /// Install the unit's facts in its `discovery`.
            fn install(self, discovery: &mut DiscoveryResult) {
                $(discovery.$map = FactMap::from(self.$map.into_iter().collect::<HashMap<_, _>>());)*
                $(discovery.$set = FactSet::from(self.$set.into_iter().collect::<HashSet<_>>());)*
            }
        }
    };
}

unit_facts! {
    maps {
        overload_targets: SourceSpan => String,
        contextual_bases: SourceSpan => String,
        generic_instantiations: SourceSpan => GenericInstantiation,
        method_instantiations: SourceSpan => MethodInstantiation,
        call_transfers: SourceSpan => Vec<CheckedCallTransfer>,
        implicit_conversions: SourceSpan => String,
        implicit_conversion_types: SourceSpan => Ty,
        conversion_source_borrows: SourceSpan => bool,
        conversion_raises: SourceSpan => Ty,
        checked_types: AnnotationSite => Ty,
        generic_parameters: GenericSite => Vec<ParamDecl>,
        expression_types: SourceSpan => Ty,
        expression_bindings: SourceSpan => OwnerId,
        statement_bindings: SourceSpan => OwnerId,
        declaration_captures: SourceSpan => Vec<CheckedCapture>,
        comprehension_bindings: SourceSpan => Vec<CheckedComprehensionBinding>,
        expression_place_types: SourceSpan => Ty,
        binding_types: SourceSpan => Ty,
        expression_effects: SourceSpan => EffectFacts,
        selected_calls: SourceSpan => CheckedCallContract,
        subscript_descriptors: SourceSpan => (Vec<Option<SliceKind>>, bool),
        iteration_protocols: SourceSpan => IterationProtocol,
        simd_constructions: SourceSpan => (SimdDtype, SimdWidth),
        operation_adjustments: SourceSpan => SemanticAdjustment,
        parameterized_method_calls: SourceSpan => Vec<ParamDecl>,
        tuple_unpack_plans: SourceSpan => Vec<CheckedTupleUnpackElement>,
        interior_references: SourceSpan => OriginPlace,
        interior_invalidations: SourceSpan => Vec<InteriorInvalidation>,
        reference_value_uses: SourceSpan => bool,
        rebind_assertions: SourceSpan => RebindAssertion,
        declaration_effects: AnnotationSite => DeclarationEffect,
    }
    sets {
        explicit_destroy_calls,
        copy_place_value_uses,
        call_place_uses,
        borrowed_read_call_places,
        read_temporary_arguments,
        implicitly_copied_consuming_receivers,
        truthiness_conditions,
    }
}
