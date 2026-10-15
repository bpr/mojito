//! `eliminate-dead-symbols`: upstream's `EliminateDeadSymbols`, the
//! symbol DCE its pre-elaboration pipeline runs after destructor insertion,
//! over a program's own MIR.
//!
//! Upstream roots the walk at the `export` symbols and removes every other
//! symbol no live symbol refers to. Here the roots are the program's entry
//! roots and the symbols its compilation exports by name (a native entry
//! a caller compiles, such as a scalar `compute`): every function and
//! struct nothing reachable from them names goes, its declaration with it.

use super::pre_elaboration::ProgramPass;
use mojito_common::timing;
use mojito_mir::mir::{
    MirFunction, MirFunctionDeclaration, MirProgram, MirStructDeclaration, SymbolTable,
    destroyed_structs, entry_roots,
};
use mojito_symbol::symbol::{OV_SEP, split_method_symbol};
use mojito_types::types::TyKind;
use std::collections::{HashMap, HashSet};

/// The pass, under upstream's flag spelling.
pub const ELIMINATE_DEAD_SYMBOLS: ProgramPass = ProgramPass {
    name: "eliminate-dead-symbols",
    run: eliminate_dead_symbols,
};

/// Remove `program`'s functions and structs that neither its entry roots
/// nor `exports` reach, with their declarations, keeping the rest in order.
///
/// A live function makes live every symbol its body or declaration names,
/// every member of its overload family, every function nested in it, and
/// the destructor of every struct its drops may tear down
/// ([`destroyed_structs`]), as upstream's destructor insertion leaves a
/// call to it at each death. A live struct makes live every symbol its
/// declaration names and its other methods: the elaborator reaches a
/// struct's constructors, display, hash, and trait-requirement methods from
/// its type alone, as upstream nests a struct's members under its symbol. A
/// body that drops a value of a type only an instantiation resolves, or a
/// program that imports bodies the walk cannot see, keeps every live
/// struct's destructor.
pub fn eliminate_dead_symbols(program: &mut MirProgram, exports: &[String]) {
    let (live, live_structs) = {
        let (functions, structs) = Symbols::of(program).live(exports);
        let owned = |names: HashSet<&str>| -> HashSet<String> {
            names.into_iter().map(str::to_string).collect()
        };
        (owned(functions), owned(structs))
    };
    let mut dead = HashSet::new();
    let mut instructions = 0;
    program.functions.retain(|(name, function)| {
        let keep = live.contains(name);
        if !keep {
            instructions += function.instruction_count();
            dead.insert(name.clone());
        }
        keep
    });
    let declarations = &mut program.declarations;
    declarations
        .functions
        .retain(|declaration| !dead.contains(&declaration.lowered_name));
    let structs = declarations.structs.len();
    declarations
        .structs
        .retain(|declaration| live_structs.contains(&declaration.name));
    timing::count("dead_functions", dead.len() as u64);
    timing::count("dead_instructions", instructions as u64);
    timing::count(
        "dead_structs",
        (structs - declarations.structs.len()) as u64,
    );
}

/// A program's own symbols, indexed for the liveness walk.
struct Symbols<'p> {
    program: &'p MirProgram,
    functions: HashMap<&'p str, &'p MirFunction>,
    declarations: HashMap<&'p str, &'p MirFunctionDeclaration>,
    structs: HashMap<&'p str, &'p MirStructDeclaration>,
    /// Every own function and struct name: what a use may name.
    table: SymbolTable<'p>,
    /// Each own struct's methods but its destructor.
    methods: HashMap<&'p str, Vec<&'p str>>,
    /// Each own struct's destructor.
    destructors: HashMap<&'p str, &'p str>,
    /// Each overload family's members, by the family's base name.
    families: HashMap<&'p str, Vec<&'p str>>,
    /// The functions nested in each function.
    nested: HashMap<&'p str, Vec<&'p str>>,
}

impl<'p> Symbols<'p> {
    fn of(program: &'p MirProgram) -> Self {
        let functions: HashMap<_, _> = program
            .functions
            .iter()
            .map(|(name, function)| (name.as_str(), function))
            .collect();
        let declarations: HashMap<_, _> = program
            .declarations
            .functions
            .iter()
            .map(|declaration| (declaration.lowered_name.as_str(), declaration))
            .collect();
        let structs: HashMap<_, _> = program
            .declarations
            .structs
            .iter()
            .map(|declaration| (declaration.name.as_str(), declaration))
            .collect();
        let table = SymbolTable::new(functions.keys().chain(structs.keys()).copied());
        let mut methods: HashMap<_, Vec<_>> = HashMap::new();
        let mut destructors = HashMap::new();
        let mut families: HashMap<_, Vec<_>> = HashMap::new();
        let mut nested: HashMap<_, Vec<_>> = HashMap::new();
        for (name, _) in &program.functions {
            let name = name.as_str();
            if let Some((owner, method)) = split_method_symbol(name)
                && structs.contains_key(owner)
            {
                if method == "__deinit__" {
                    destructors.insert(owner, name);
                } else {
                    methods.entry(owner).or_default().push(name);
                }
            }
            families.entry(family(name)).or_default().push(name);
            if let Some(enclosing) = declarations
                .get(name)
                .and_then(|declaration| declaration.enclosing.as_deref())
            {
                nested.entry(enclosing).or_default().push(name);
            }
        }
        Self {
            program,
            functions,
            declarations,
            structs,
            table,
            methods,
            destructors,
            families,
            nested,
        }
    }

    /// The live functions and structs: the roots, the entry roots and
    /// `exports`, and everything they reach.
    fn live(mut self, exports: &[String]) -> (HashSet<&'p str>, HashSet<&'p str>) {
        let mut pending: Vec<&'p str> = entry_roots(self.program)
            .iter()
            .chain(exports)
            .filter_map(|root| self.functions.get_key_value(root.as_str()))
            .map(|(&name, _)| name)
            .collect();
        let mut live = HashSet::new();
        let mut live_structs = HashSet::new();
        // Set once a live body drops a value of a symbolic type. A body the
        // program imports is not walked, and may drop any value.
        let mut every_destructor = self.program.declarations.imported.is_some();
        while let Some(name) = pending.pop() {
            if let Some(function) = self.functions.get(name)
                && live.insert(name)
            {
                pending.extend(self.table.function_uses(function));
                let destroyed = destroyed_structs(function, &|name| {
                    self.structs
                        .get(name)
                        .map(|declaration| declaration.fields.as_slice())
                });
                if destroyed.symbolic && !every_destructor {
                    every_destructor = true;
                    pending.extend(
                        live_structs
                            .iter()
                            .filter_map(|name| self.destructors.get(name).copied()),
                    );
                }
                pending.extend(destroyed.structs.iter().filter_map(|ty| match ty.kind() {
                    TyKind::Struct(name, _) => self.destructors.get(name.as_str()).copied(),
                    _ => None,
                }));
                if let Some(declaration) = self.declarations.get(name) {
                    pending.extend(self.table.uses(*declaration));
                }
                pending.extend(self.families.get(family(name)).into_iter().flatten());
                pending.extend(self.nested.get(name).into_iter().flatten());
            }
            if let Some(declaration) = self.structs.get(name)
                && live_structs.insert(name)
            {
                pending.extend(self.table.uses(*declaration));
                pending.extend(self.methods.get(name).into_iter().flatten());
                if every_destructor {
                    pending.extend(self.destructors.get(name).copied());
                }
            }
        }
        (live, live_structs)
    }
}

/// The overload family `name` belongs to: its spelling before the overload
/// qualifier.
fn family(name: &str) -> &str {
    name.split_once(OV_SEP).map_or(name, |(base, _)| base)
}
