//! Package part decoding: a package's interface, its declarations with the
//! `files` table its bodies name, and one body at a time.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::{CallThroughArg, CallThroughCallee, CallThroughEffect};
use mojito_checked::interface::PackageInterface;

/// The `files` table of a package's declarations, which every body's
/// locations name.
#[derive(Debug, Clone, Default)]
pub struct PackageFiles(BTreeMap<usize, Option<String>>);

/// Decode a package's interface record.
pub(in crate::mir::text) fn interface(
    text: &str,
) -> Result<PackageInterface, Vec<ArtifactDiagnostic>> {
    let value = whole_value(text, false)?;
    let mut decoder = Decoder::new(false);
    let interface = decoder.interface(&value);
    match interface {
        Some(interface) if decoder.diagnostics.is_empty() => Ok(interface),
        _ => Err(decoder.diagnostics),
    }
}

/// Decode a package's declarations artifact, whose function list is empty.
pub(in crate::mir::text) fn declarations(
    text: &str,
) -> Result<(MirDeclarations, PackageFiles), Vec<ArtifactDiagnostic>> {
    let value = whole_value(text, true)?;
    let mut decoder = Decoder::new(false);
    let declarations = decoder.package_declarations(&value);
    match declarations {
        Some(declarations) if decoder.diagnostics.is_empty() => {
            Ok((declarations, PackageFiles(decoder.files)))
        }
        _ => Err(decoder.diagnostics),
    }
}

/// Decode one body of a package whose locations name `files`.
pub(in crate::mir::text) fn function(
    text: &str,
    files: &PackageFiles,
) -> Result<(String, MirFunction), Vec<ArtifactDiagnostic>> {
    let value = whole_value(text, false)?;
    let mut decoder = Decoder::new(false);
    decoder.files.clone_from(&files.0);
    let function = decoder.function(&value);
    match function {
        Some(function) if decoder.diagnostics.is_empty() => Ok(function),
        _ => Err(decoder.diagnostics),
    }
}

/// The one value `text` holds, after the version header when `headed`.
fn whole_value(text: &str, headed: bool) -> Result<Value, Vec<ArtifactDiagnostic>> {
    let mut parser = Parser::new(text);
    if headed {
        parser.header();
    }
    let value = parser.value();
    parser.space();
    if parser.pos < text.len() {
        parser.error(
            (parser.pos, text.len()),
            "trailing tokens after package part",
        );
    }
    if !parser.diagnostics.is_empty() {
        return Err(parser.diagnostics);
    }
    value.ok_or_else(|| vec![diagnostic((0, text.len()), "missing package part")])
}

impl Decoder {
    fn interface(&mut self, value: &Value) -> Option<PackageInterface> {
        let fields = self.record(value, "interface").ok()?;
        let modules = self.strings(self.field(fields, "modules").ok()?);
        let names = self.strings(self.field(fields, "names").ok()?);
        let asked_traits = self.strings(self.field(fields, "asked_traits").ok()?);
        let transfer_effects = self.summaries(
            self.field(fields, "transfer_effects").ok()?,
            |decoder, effects| Some(decoder.transfer_set(effects).0),
        )?;
        let call_through_effects = self.summaries(
            self.field(fields, "call_through_effects").ok()?,
            |decoder, effects| {
                decoder
                    .list(effects)
                    .ok()?
                    .iter()
                    .map(|effect| decoder.call_through(effect))
                    .collect()
            },
        )?;
        self.unknown(
            fields,
            &[
                "modules",
                "names",
                "asked_traits",
                "transfer_effects",
                "call_through_effects",
            ],
        );
        Some(PackageInterface {
            modules: modules.into_iter().collect(),
            names: names.into_iter().collect(),
            transfer_effects,
            call_through_effects,
            asked_traits: asked_traits.into_iter().collect(),
        })
    }

    fn summaries<E>(
        &mut self,
        value: &Value,
        effects: impl Fn(&mut Self, &Value) -> Option<Vec<E>>,
    ) -> Option<BTreeMap<String, Vec<E>>> {
        self.list(value)
            .ok()?
            .iter()
            .map(|summary| {
                let fields = self.record(summary, "summary").ok()?;
                let callable = self.req(summary, fields, "callable", Self::symbol)?;
                let decoded = effects(self, self.field(fields, "effects").ok()?)?;
                self.unknown(fields, &["callable", "effects"]);
                Some((callable, decoded))
            })
            .collect()
    }

    fn call_through(&mut self, value: &Value) -> Option<CallThroughEffect> {
        let fields = self.record(value, "call_through").ok()?;
        let callee = self.field(fields, "callee").ok()?;
        let (tag, inner) = self.positional_value(callee)?;
        let callee = match tag {
            "runtime_param" => CallThroughCallee::RuntimeParam(self.uint(inner)?),
            "value_param" => CallThroughCallee::ValueParam(self.symbol(inner)?),
            _ => {
                self.error(callee.span, format!("unknown call-through callee `{tag}`"));
                return None;
            }
        };
        let args = self
            .list(self.field(fields, "args").ok()?)
            .ok()?
            .iter()
            .map(|argument| {
                let fields = self.record(argument, "arg").ok()?;
                let place = self
                    .option_value(self.field(fields, "place").ok())
                    .map(|place| self.sig_origin(place));
                let carried = self
                    .list(self.field(fields, "carried").ok()?)
                    .ok()?
                    .iter()
                    .map(|origin| self.sig_origin(origin))
                    .collect::<Option<Vec<_>>>()?;
                let local = self.req(argument, fields, "local", Self::boolean)?;
                self.unknown(fields, &["place", "carried", "local"]);
                Some(CallThroughArg {
                    place: match place {
                        Some(place) => Some(place?),
                        None => None,
                    },
                    carried,
                    local,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        self.unknown(fields, &["callee", "args"]);
        Some(CallThroughEffect { callee, args })
    }

    fn package_declarations(&mut self, value: &Value) -> Option<MirDeclarations> {
        let fields = self.record(value, "artifact").ok()?;
        self.decode_files(self.field(fields, "files").ok()?);
        let structs = self.struct_declarations(self.field(fields, "structs").ok()?);
        let functions = self.function_declarations(self.field(fields, "decls").ok()?);
        let traits = self.strings(self.field(fields, "traits").ok()?);
        if self
            .list(self.field(fields, "functions").ok()?)
            .is_ok_and(|functions| !functions.is_empty())
        {
            self.error(value.span, "a package's declarations hold no function body");
        }
        self.unknown(
            fields,
            &[
                "features",
                "files",
                "traits",
                "structs",
                "decls",
                "functions",
            ],
        );
        Some(MirDeclarations {
            structs,
            functions,
            traits,
        })
    }
}
