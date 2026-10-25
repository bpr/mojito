//! The members of an `enum`, generated from its cases as upstream's
//! `StructEmitter::synthesizeEnumMembers` declares them.
//!
//! An enum is an ordinary struct. For
//!
//! ```mojo
//! enum Token:
//!     case eof
//!     case ident(String)
//!     case span(Int, Int)
//! ```
//!
//! it gets the `EnumLike` case lists, its one field (the case-indexed
//! `__EnumStorage` over the case payload types), a `comptime` member per
//! case without a payload, and a static constructor per case with one, plus
//! the four members upstream's `__Enum` supplies as defaults
//! (`docs/notes/enums.md`). The members are written as Mojo and parsed, so
//! they link, check, and lower as written source does; each is located at
//! the case it is generated for, or at the declaration.

use std::fmt::Write;

use mojito_ast::ast::{EnumCase, Expr, Stmt, StmtKind, Type};
use mojito_ast::visit::{MutVisitor, walk_block_mut};
use mojito_common::error::ParseError;
use mojito_common::token::Span;
use mojito_lexer::lexer::Lexer;

use crate::parser::Parser;

/// Parse a source file, generating each `enum`'s members from its cases.
pub fn parse_module(source: &str) -> Result<Vec<Stmt>, ParseError> {
    let mut program = Parser::new(Lexer::new(source)).parse_program()?;
    synthesize_enum_members(&mut program, source)?;
    Ok(program)
}

/// Generate the members of every `enum` in `program`, whose cases' payload
/// types are spelled in `source`.
pub fn synthesize_enum_members(program: &mut [Stmt], source: &str) -> Result<(), ParseError> {
    for statement in program {
        let span = statement.span;
        let StmtKind::Struct {
            name,
            conforms,
            fields,
            associated,
            methods,
            enum_cases: Some(cases),
            ..
        } = &mut statement.kind
        else {
            continue;
        };
        check_member_names(cases, associated, methods)?;
        check_case_payloads(name, conforms, cases, span)?;
        let payloads = cases
            .iter()
            .map(|case| payload_spellings(case, source))
            .collect::<Result<Vec<_>, _>>()?;
        let writes = conforms.iter().any(|conformance| conformance == "Writable");
        let written = |member: &str| methods.iter().any(|method| method.name == member);
        let mut text = generated_members(name, cases, &payloads);
        for (member, payload) in [("write_to", "{}"), ("write_repr_to", "repr({})")] {
            if writes && !written(member) {
                text.push_str(&generated_write(member, payload, name, cases));
            }
        }
        let members = parse_members(&text)?;
        let StmtKind::Struct {
            fields: generated_fields,
            associated: mut generated_associated,
            methods: mut generated_methods,
            ..
        } = members.kind
        else {
            unreachable!("the generated members parse as one struct");
        };
        // A member generated for a case is located at the case, and every
        // other one at the declaration.
        for member in &mut generated_associated {
            let at = cases
                .iter()
                .find(|case| case.name == member.name)
                .map_or(span, |case| case.span);
            relocate_expr(&mut member.value, at);
            for condition in &mut member.where_clauses {
                relocate_expr(condition, at);
            }
        }
        for method in &mut generated_methods {
            let at = cases
                .iter()
                .find(|case| case.name == method.name)
                .map_or(span, |case| case.span);
            walk_block_mut(&mut Relocate(at), &mut method.body);
        }
        conforms.push(ENUM_TRAIT.to_string());
        fields.extend(generated_fields);
        associated.extend(generated_associated);
        methods.extend(generated_methods);
    }
    Ok(())
}

/// `std.builtin._enum.__Enum` by its linked name, which linking leaves as it
/// is, as upstream's compiler finds the module implementing enums by path.
const ENUM_TRAIT: &str = "__module$std$builtin$$enum$__Enum";

/// The names `__Enum` and the generated members declare, which no member
/// written in an enum may declare. `_enum_is_exhaustive` is not among them:
/// an enum may declare it to open its cases.
const SYNTHESIZED_NAMES: [&str; 8] = [
    "__enum_storage",
    "_enum_case_names",
    "_enum_case_types",
    "_get_enum_discriminant",
    "_unsafe_get_enum_payload",
    "_enum_elt_type_for_case",
    "__enum_init",
    "__get_enum_storage",
];

/// The members every struct gets, which a case may not be named for either.
const STRUCT_MEMBER_NAMES: [&str; 6] = [
    "_enum_is_exhaustive",
    "__init__",
    "__del__",
    "__copy_ctor_is_trivial",
    "__move_ctor_is_trivial",
    "__del__is_trivial",
];

/// Every clash between the members an enum gets and the members written in
/// it, with upstream's `checkEnumMemberNames` words.
fn check_member_names(
    cases: &[EnumCase],
    associated: &[mojito_ast::ast::StructComptime],
    methods: &[mojito_ast::ast::Method],
) -> Result<(), ParseError> {
    let written = associated
        .iter()
        .map(|member| member.name.as_str())
        .chain(methods.iter().map(|method| method.name.as_str()));
    for name in written.clone() {
        if SYNTHESIZED_NAMES.contains(&name) {
            return Err(ParseError::Message(format!(
                "'{name}' is synthesized for enums and cannot be declared"
            )));
        }
    }
    let mut seen = std::collections::HashSet::new();
    for case in cases {
        let name = case.name.as_str();
        let error = |message: String| Err(ParseError::Message(message).at(case.span));
        if !seen.insert(name)
            || SYNTHESIZED_NAMES.contains(&name)
            || STRUCT_MEMBER_NAMES.contains(&name)
        {
            return error(format!("invalid redefinition of '{name}'"));
        }
        if name.len() > 4 && name.starts_with("__") && name.ends_with("__") {
            return error(format!(
                "case '{name}' must not use the name of a special method that takes 'self'; rename the case"
            ));
        }
        if written.clone().any(|member| member == name) {
            return error(format!(
                "enum case '{name}' conflicts with a member of the same name"
            ));
        }
    }
    Ok(())
}

/// The payload and conformance rules upstream's declaration resolution
/// applies to an enum: `NoneType` marks a case without a payload, so it is no
/// payload type; a payload holding the enum itself by value has no size; and
/// an enum's storage is never trivially register-passable.
fn check_case_payloads(
    name: &str,
    conforms: &[String],
    cases: &[EnumCase],
    span: Span,
) -> Result<(), ParseError> {
    if conforms
        .iter()
        .any(|conformance| conformance == "TrivialRegisterPassable")
    {
        return Err(ParseError::Message(
            "enums cannot conform to 'TrivialRegisterPassable'".to_string(),
        )
        .at(span));
    }
    for case in cases {
        let error = |message: String| Err(ParseError::Message(message).at(case.span));
        if matches!(case.payload.as_slice(), [Type::Named(payload, args)] if payload == "NoneType" && args.is_empty())
        {
            return error(format!(
                "enum case payload must not be 'NoneType'; a case without a payload is written 'case {}'",
                case.name
            ));
        }
        if case.payload.iter().any(|payload| match payload {
            Type::SelfType => true,
            Type::Named(payload, args) => payload == name && args.is_empty(),
            _ => false,
        }) {
            return error(format!(
                "case '{}' has recursive payload type '{name}'",
                case.name
            ));
        }
    }
    Ok(())
}

/// The source spelling of each of a case's payload types, read from inside
/// its parentheses (`case span(Int, Self.T)`), split at the commas outside
/// any bracket.
fn payload_spellings(case: &EnumCase, source: &str) -> Result<Vec<String>, ParseError> {
    if case.payload.is_empty() {
        return Ok(Vec::new());
    }
    let text = source.get(case.span.0..case.span.1).unwrap_or_default();
    let unreadable = || {
        ParseError::Message(format!(
            "cannot read the payload types of case '{}'",
            case.name
        ))
        .at(case.span)
    };
    let open = text.find('(').ok_or_else(unreadable)?;
    let close = text.rfind(')').ok_or_else(unreadable)?;
    let inner = text.get(open + 1..close).ok_or_else(unreadable)?;
    let mut spellings = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for line in inner.lines() {
        let line = line.split('#').next().unwrap_or_default();
        for character in line.chars() {
            match character {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    spellings.push(std::mem::take(&mut current));
                    continue;
                }
                _ => {}
            }
            current.push(character);
        }
        current.push(' ');
    }
    spellings.push(current);
    let spellings = spellings
        .into_iter()
        .map(|spelling| spelling.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|spelling| !spelling.is_empty())
        .collect::<Vec<_>>();
    if spellings.len() == case.payload.len() {
        Ok(spellings)
    } else {
        Err(unreadable())
    }
}

/// The generated members, as the body of a struct named for the enum.
fn generated_members(name: &str, cases: &[EnumCase], payloads: &[Vec<String>]) -> String {
    let case_type = |payload: &[String]| match payload {
        [] => "NoneType".to_string(),
        [single] => single.clone(),
        several => format!("Tuple[{}]", several.join(", ")),
    };
    let case_types = payloads
        .iter()
        .map(|payload| case_type(payload))
        .collect::<Vec<_>>();
    let storage = if case_types.is_empty() {
        "__EnumStorage".to_string()
    } else {
        format!("__EnumStorage[{}]", case_types.join(", "))
    };
    // An enum without cases lists none, spelled with its element type.
    let names = if cases.is_empty() {
        "type=__mlir_type.`!kgen.string`".to_string()
    } else {
        cases
            .iter()
            .map(|case| format!("{:?}.value", case.name))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let types = std::iter::once("Trait=AnyType".to_string())
        .chain(case_types.iter().cloned())
        .collect::<Vec<_>>()
        .join(", ");
    let mut text = format!(
        "struct {name}:\n\
         \x20   comptime _enum_case_names = ParameterList.of[{names}].values\n\
         \x20   comptime _enum_case_types = TypeList.of[{types}].values\n\
         \x20   var __enum_storage: {storage}\n"
    );
    for (index, (case, payload)) in cases.iter().zip(payloads).enumerate() {
        let case_name = &case.name;
        let _ = match payload.as_slice() {
            [] => writeln!(
                text,
                "    comptime {case_name}: Self = Self.__enum_init({storage}(None, index={index}))"
            ),
            [single] => writeln!(
                text,
                "    @staticmethod\n\
                 \x20   def {case_name}(var payload: {single}, out result: Self):\n\
                 \x20       __mlir_op.`lit.ownership.mark_initialized`(__get_mvalue_as_litref(result))\n\
                 \x20       Pointer(to=result.__enum_storage).unsafe_write({storage}(payload^, index={index}))"
            ),
            several => {
                let parameters = several
                    .iter()
                    .enumerate()
                    .map(|(position, ty)| format!("var payload{position}: {ty}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let elements = (0..several.len())
                    .map(|position| format!("payload{position}^"))
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(
                    text,
                    "    @staticmethod\n\
                     \x20   def {case_name}({parameters}, out result: Self):\n\
                     \x20       __mlir_op.`lit.ownership.mark_initialized`(__get_mvalue_as_litref(result))\n\
                     \x20       Pointer(to=result.__enum_storage).unsafe_write({storage}(Tuple({elements}), index={index}))"
                )
            }
        };
    }
    let _ = writeln!(
        text,
        "    @staticmethod\n\
         \x20   def __enum_init(var storage: {storage}, out result: Self):\n\
         \x20       __mlir_op.`lit.ownership.mark_initialized`(__get_mvalue_as_litref(result))\n\
         \x20       Pointer(to=result.__enum_storage).unsafe_write(storage^)\n\
         \x20   def __get_enum_storage(ref self) -> ref[self] {storage}:\n\
         \x20       return self.__enum_storage\n\
         \x20   def _get_enum_discriminant(self) -> Int:\n\
         \x20       return self.__enum_storage.get_discriminant()\n\
         \x20   def _unsafe_get_enum_payload[id: Int](ref self) -> ref[self] TypeList[Trait=AnyType, Self._enum_case_types]()[id]:\n\
         \x20       return rebind[TypeList[Trait=AnyType, Self._enum_case_types]()[id]](self.__enum_storage[id])"
    );
    text
}

/// `Writable`'s default for an enum, `Name.case` or `Name.case(payload)`
/// with the payload written as `payload` spells it (its text, or its
/// `repr`), as upstream's `_enum_reflection_write_to` writes any `EnumLike`.
fn generated_write(member: &str, payload: &str, name: &str, cases: &[EnumCase]) -> String {
    let mut text = format!(
        "    def {member}(self, mut writer: Some[Writer]):\n\
         \x20       writer.write(\"{name}.\")\n\
         \x20       var discriminant = self._get_enum_discriminant()\n"
    );
    for (index, case) in cases.iter().enumerate() {
        let keyword = if index == 0 { "if" } else { "elif" };
        let case_name = &case.name;
        let _ = writeln!(
            text,
            "        {keyword} discriminant == {index}:\n\
             \x20           writer.write(\"{case_name}\")"
        );
        if !case.payload.is_empty() {
            let value = payload.replace("{}", &format!("self._unsafe_get_enum_payload[{index}]()"));
            let _ = writeln!(text, "            writer.write(\"(\", {value}, \")\")");
        }
    }
    text
}

/// The generated struct. A payload type the parser rejects here was already
/// accepted inside its case, so a failure is the generator's.
fn parse_members(text: &str) -> Result<Stmt, ParseError> {
    let mut program = Parser::new(Lexer::new(text)).parse_program()?;
    match program.pop() {
        Some(statement) if program.is_empty() => Ok(statement),
        _ => Err(ParseError::Message(
            "internal error: the generated enum members are not one struct".to_string(),
        )),
    }
}

fn relocate_expr(expr: &mut Expr, at: Span) {
    let mut statement = Stmt::new(StmtKind::Expr(expr.clone()), at);
    walk_block_mut(&mut Relocate(at), std::slice::from_mut(&mut statement));
    if let StmtKind::Expr(relocated) = statement.kind {
        *expr = relocated;
    }
}

/// Moves every generated node to the source span it is generated for.
struct Relocate(Span);

impl MutVisitor for Relocate {
    fn visit_stmt_mut(&mut self, statement: &mut Stmt) {
        statement.span = self.0;
    }

    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        expr.span = self.0;
    }
}
