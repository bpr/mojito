use super::{ArtifactDiagnostic, ArtifactReport, ArtifactSourceMap, ParsedArtifact};
use crate::mir::{
    Const, FuncRef, MirBlock, MirCaptureAccess, MirCaptureMode, MirClosureCapture, MirDeclarations,
    MirFunction, MirFunctionDeclaration, MirInstr, MirInteriorOrigin, MirIntrinsicSubscript,
    MirLoan, MirParamArg, MirPlace, MirProgram, MirStructDeclaration, MirSubscriptArg,
    MirSubscriptCall, MirTerm, Proj, Reg, SpanTable, UseMode,
};
use mojito_ast::ast::{ArgConvention, Dtype, InfixOp, PrefixOp};
use mojito_checked::checked::{
    CheckedCallArgument, CheckedCallArgumentSource, CheckedConst, CheckedIteratorCall,
    CheckedResultAdapter, IterationMode, TransferEffect, TransferSet,
};
use mojito_common::literal::{FloatLiteral, IntLiteral};
use mojito_common::token::SourceSpan;
use mojito_types::ct::{CtExpr, CtValue};
use mojito_types::origin::{
    CallableEnvironment, CaptureAccess, CaptureOrigin, CaptureOriginSet, CaptureSetParamId,
    Mutability, Origin, OriginParamId, OriginPlace, OriginSeg, OwnerId, PointerOrigin, RefSig,
    RefTy, SigMutability, SigOrigin,
};
use mojito_types::types::{
    CallableDefault, ConstraintOperand, DependentType, GenericConstraint, PackPredicateRef,
    ParamDecl, SliceKind, TrivialLifecycle, Ty, TyArg,
};
use std::collections::{BTreeMap, HashMap, HashSet};

const MAX_DIAGNOSTICS: usize = 64;

#[derive(Debug, Clone)]
struct Value {
    kind: ValueKind,
    span: (usize, usize),
}

#[derive(Debug, Clone)]
enum ValueKind {
    Atom(String),
    String(String),
    List(Vec<Value>),
    Positional(String, Box<Value>),
    Record(String, Vec<Field>),
}

#[derive(Debug, Clone)]
struct Field {
    name: String,
    name_span: (usize, usize),
    value: Value,
}

pub(super) fn artifact(
    input: &[u8],
    source_name: String,
) -> Result<ParsedArtifact, ArtifactReport> {
    let source = match std::str::from_utf8(input) {
        Ok(source) => source,
        Err(error) => {
            let start = error.valid_up_to();
            return Err(report(
                source_name,
                vec![diagnostic(
                    (start, start + 1),
                    "artifact is not valid UTF-8",
                )],
            ));
        }
    };
    if input.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(report(
            source_name,
            vec![diagnostic((0, 3), "byte-order mark is not permitted")],
        ));
    }
    let mut parser = Parser::new(source);
    parser.header();
    let value = parser.value();
    parser.space();
    if parser.pos < source.len() {
        parser.error((parser.pos, source.len()), "trailing tokens after artifact");
    }
    if !parser.diagnostics.is_empty() {
        return Err(report(source_name, parser.diagnostics));
    }
    let Some(value) = value else {
        return Err(report(
            source_name,
            vec![diagnostic((0, input.len()), "missing artifact record")],
        ));
    };
    match Decoder::new().program(&value) {
        Ok((program, source_map)) => Ok(ParsedArtifact {
            program,
            source_map,
        }),
        Err(diagnostics) => Err(report(source_name, diagnostics)),
    }
}

fn report(source_name: String, diagnostics: Vec<ArtifactDiagnostic>) -> ArtifactReport {
    ArtifactReport {
        source_name,
        diagnostics,
    }
}

fn diagnostic(span: (usize, usize), message: impl Into<String>) -> ArtifactDiagnostic {
    ArtifactDiagnostic {
        span,
        message: message.into(),
        context: Vec::new(),
    }
}

struct Parser<'a> {
    source: &'a str,
    pos: usize,
    diagnostics: Vec<ArtifactDiagnostic>,
}

mod decls;
mod instrs;
mod operands;
mod origins;
mod prims;
mod reader;
mod types;

struct Decoder {
    diagnostics: Vec<ArtifactDiagnostic>,
    source_map: ArtifactSourceMap,
    files: BTreeMap<usize, Option<String>>,
}

fn parse_int_literal(value: &str) -> Option<IntLiteral> {
    value.strip_prefix('-').map_or_else(
        || IntLiteral::parse_radix(value, 10),
        |digits| IntLiteral::parse_radix(digits, 10).map(|value| value.neg()),
    )
}

#[cfg(test)]
mod tests;
