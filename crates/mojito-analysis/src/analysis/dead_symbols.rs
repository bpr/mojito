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
    MirFunction, MirFunctionDeclaration, MirProgram, MirStructDeclaration, SymbolTable, entry_roots,
};
use mojito_symbol::symbol::{OV_SEP, split_method_symbol};
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
/// every member of its overload family, and every function nested in it. A
/// live struct makes live every symbol its declaration names and all its
/// methods: the elaborator reaches a struct's lifecycle, display, hash, and
/// trait-requirement methods from its type alone, as upstream nests a
/// struct's members under its symbol.
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
    /// Each own struct's methods.
    methods: HashMap<&'p str, Vec<&'p str>>,
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
        let mut families: HashMap<_, Vec<_>> = HashMap::new();
        let mut nested: HashMap<_, Vec<_>> = HashMap::new();
        for (name, _) in &program.functions {
            let name = name.as_str();
            if let Some((owner, _)) = split_method_symbol(name)
                && structs.contains_key(owner)
            {
                methods.entry(owner).or_default().push(name);
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
        while let Some(name) = pending.pop() {
            if let Some(function) = self.functions.get(name)
                && live.insert(name)
            {
                pending.extend(self.table.function_uses(function));
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
