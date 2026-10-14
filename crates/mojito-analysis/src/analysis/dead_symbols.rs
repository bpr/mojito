//! `eliminate-dead-symbols`: upstream's `EliminateDeadSymbols`, the
//! symbol DCE its pre-elaboration pipeline runs after destructor insertion,
//! over a program's own MIR.
//!
//! Upstream roots the walk at the `export` symbols and removes every other
//! symbol no live symbol refers to. Mojito has no `export`, and a native
//! caller may ask for any concrete function by name, so every concrete
//! function is a root: only the parametric bodies no root reaches go. A
//! declaration stays, without its body, as an unreached package
//! declaration does.

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

/// Remove the bodies of `program`'s parametric functions that no root
/// reaches, keeping the rest in order.
///
/// A live function makes live every symbol its body or declaration names,
/// every member of its overload family, and every function nested in it. A
/// live struct makes live every symbol its declaration names and all its
/// methods: the elaborator reaches a struct's lifecycle, display, hash, and
/// trait-requirement methods from its type alone, as upstream nests a
/// struct's members under its symbol.
pub fn eliminate_dead_symbols(program: &mut MirProgram) {
    let live: HashSet<String> = Symbols::of(program)
        .live()
        .into_iter()
        .map(str::to_string)
        .collect();
    let mut functions = 0;
    let mut instructions = 0;
    program.functions.retain(|(name, function)| {
        let keep = live.contains(name);
        if !keep {
            functions += 1;
            instructions += function.instruction_count();
        }
        keep
    });
    timing::count("dead_functions", functions);
    timing::count("dead_instructions", instructions as u64);
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

    /// The live functions: the roots and everything they reach.
    fn live(mut self) -> HashSet<&'p str> {
        let mut pending: Vec<&'p str> = self
            .functions
            .keys()
            .copied()
            .filter(|name| !self.parametric(name))
            .collect();
        pending.extend(
            entry_roots(self.program)
                .iter()
                .filter_map(|root| self.functions.get_key_value(root.as_str()))
                .map(|(&name, _)| name),
        );
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
        live
    }

    /// Whether the function `name` is parametric: it, the struct it is a
    /// method of, or a body it is nested in declares compile-time
    /// parameters. A function without a declaration, or a method of no
    /// struct the program declares, counts as concrete, so it stays a root.
    fn parametric(&self, name: &str) -> bool {
        let Some(declaration) = self.declarations.get(name) else {
            return false;
        };
        let owner = split_method_symbol(name).map(|(owner, _)| owner);
        if owner.is_some_and(|owner| !self.structs.contains_key(owner)) {
            return false;
        }
        !declaration.param_decls.is_empty()
            || !declaration.region_binders.is_empty()
            || declaration.enclosing.is_some()
            || owner
                .and_then(|owner| self.structs.get(owner))
                .is_some_and(|owner| !owner.param_decls.is_empty())
    }
}

/// The overload family `name` belongs to: its spelling before the overload
/// qualifier.
fn family(name: &str) -> &str {
    name.split_once(OV_SEP).map_or(name, |(base, _)| base)
}
