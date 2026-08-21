use mojito::{
    BackendKind, Compiler, CompilerError, LinkOptions, ModuleError, ParseError, Stmt, lex, parse,
    parse_diagnostics,
};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Obtain the program to check/run: when a real file path is given, **link** it
/// with its imported modules (`from module import …`); for stdin (or `-`), parse
/// the source alone (imports left unresolved — there is no base directory). Either
/// way, **compile-time elaboration** resolves `comptime if`/`comptime for` before
/// the program is handed to the checker.
fn load_program(file: Option<&str>, link_options: &LinkOptions) -> Result<Vec<Stmt>, String> {
    let program = match file {
        Some(path) if path != "-" => {
            let source = read_source(file).map_err(|e| format!("cannot read input: {e}"))?;
            mojito::link_source_with_options(&source, Path::new(path), link_options.clone())
                .map_err(|e| format_module_error(&e, path, &source))?
        }
        _ => {
            let source = read_source(file).map_err(|e| format!("cannot read input: {e}"))?;
            parse(&source).map_err(|e| format_parse_error(file.unwrap_or("-"), &source, &e))?
        }
    };
    mojito::elaborate(program).map_err(|e| e.to_string())
}

/// mojito doubles as a small **syntax-analysis tool**. With no arguments it
/// runs the built-in demo; otherwise the first argument selects a pipeline stage
/// to run over a file (or stdin), so you can inspect the tokens or the AST:
///
/// ```text
/// mojito lex   [FILE]   # the token stream, one per line
/// mojito parse [FILE]   # the parsed AST (pretty-printed)
/// mojito check [FILE]   # lex + parse + type-check; report ok / the error
/// mojito run   [FILE]   # the full pipeline; print output + final bindings
/// mojito emit-mir [FILE] # compile and print executable textual MIR
/// mojito exec  [FILE]   # execute a verified textual MIR artifact
/// mojito demo           # the built-in showcase (also the no-arg default)
/// ```
///
/// A `FILE` of `-`, or its absence, reads from standard input.
fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let cli = match parse_cli_args(raw) {
        Ok(cli) => cli,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let (command, file) = (
        cli.args.first().map(String::as_str),
        cli.args.get(1).map(String::as_str),
    );
    if command != Some("compile") && (cli.emit.is_some() || cli.output.is_some()) {
        eprintln!("--emit and --output are only valid with the compile command");
        return ExitCode::FAILURE;
    }
    if !matches!(command, Some("compile" | "run"))
        && (cli.native_opt.is_some() || cli.target.is_some())
    {
        eprintln!("--native-opt and --target are only valid with the compile and run commands");
        return ExitCode::FAILURE;
    }
    match command {
        None => ExitCode::SUCCESS,
        Some("lex") => stage("lex", file, run_lex),
        Some("parse") => stage_parse(file),
        Some("check") => program_stage("check", file, &cli.link_options, run_check),
        Some("own") => program_stage("own", file, &cli.link_options, run_own),
        Some("run") => stage_run(file, &cli),
        Some("emit-mir") => stage_emit_mir(file, &cli.link_options),
        Some("compile") => stage_compile(file, &cli),
        Some("exec") => stage_exec(file, cli.backend),
        Some("-h" | "--help" | "help") => {
            print_usage();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown command '{other}'\n");
            print_usage();
            ExitCode::FAILURE
        }
    }
}

struct CliArgs {
    backend: BackendKind,
    args: Vec<String>,
    link_options: LinkOptions,
    /// `--emit KIND` for `compile` (parsed by the backend; raw here so the
    /// default build carries no backend types).
    emit: Option<String>,
    /// `-o`/`--output PATH` for `compile`.
    output: Option<String>,
    /// `--native-opt LEVEL` for `compile`/`run --backend pliron` (parsed by
    /// the backend; raw here so the default build carries no backend types).
    native_opt: Option<String>,
    /// `--target TRIPLE` for `compile`/`run --backend pliron` (parsed into
    /// the checked native target; raw here for the same reason).
    target: Option<String>,
}

/// Extract global options from anywhere on the command line. Local imports win,
/// then CLI roots in occurrence order, then the bundled stdlib fallback.
fn parse_cli_args(raw: Vec<String>) -> Result<CliArgs, String> {
    let mut backend = BackendKind::Vm;
    let mut args = Vec::new();
    let mut roots = Vec::<PathBuf>::new();
    let mut emit = None;
    let mut output = None;
    let mut native_opt = None;
    let mut target = None;
    let mut iter = raw.into_iter();
    while let Some(arg) = iter.next() {
        if let Some(name) = arg.strip_prefix("--backend=") {
            backend = BackendKind::parse(name)?;
        } else if arg == "--backend" {
            let name = iter.next().ok_or("--backend requires a name")?;
            backend = BackendKind::parse(&name)?;
        } else if let Some(path) = arg.strip_prefix("--module-path=") {
            require_path("--module-path", path, &mut roots)?;
        } else if arg == "--module-path" || arg == "-I" {
            let path = iter
                .next()
                .ok_or_else(|| format!("{arg} requires a path"))?;
            require_path(&arg, &path, &mut roots)?;
        } else if let Some(path) = arg.strip_prefix("--stdlib=") {
            require_path("--stdlib", path, &mut roots)?;
        } else if arg == "--stdlib" {
            let path = iter.next().ok_or("--stdlib requires a path")?;
            require_path("--stdlib", &path, &mut roots)?;
        } else if let Some(kind) = arg.strip_prefix("--emit=") {
            emit = Some(kind.to_string());
        } else if arg == "--emit" {
            emit = Some(iter.next().ok_or("--emit requires a kind")?);
        } else if let Some(path) = arg.strip_prefix("--output=") {
            output = Some(path.to_string());
        } else if arg == "-o" || arg == "--output" {
            output = Some(
                iter.next()
                    .ok_or_else(|| format!("{arg} requires a path"))?,
            );
        } else if let Some(level) = arg.strip_prefix("--native-opt=") {
            native_opt = Some(level.to_string());
        } else if arg == "--native-opt" {
            native_opt = Some(iter.next().ok_or("--native-opt requires a level")?);
        } else if let Some(triple) = arg.strip_prefix("--target=") {
            target = Some(triple.to_string());
        } else if arg == "--target" {
            target = Some(iter.next().ok_or("--target requires a triple")?);
        } else if arg.starts_with('-') && arg != "-" && !matches!(arg.as_str(), "-h" | "--help") {
            return Err(format!("unknown option '{arg}'"));
        } else {
            args.push(arg);
        }
    }
    roots.extend(LinkOptions::default().search_roots);
    Ok(CliArgs {
        backend,
        args,
        link_options: LinkOptions {
            search_roots: roots,
        },
        emit,
        output,
        native_opt,
        target,
    })
}

fn require_path(option: &str, path: &str, roots: &mut Vec<PathBuf>) -> Result<(), String> {
    if path.is_empty() {
        Err(format!("{option} requires a non-empty path"))
    } else {
        roots.push(PathBuf::from(path));
        Ok(())
    }
}

fn format_module_error(err: &ModuleError, entry_path: &str, entry_source: &str) -> String {
    match err {
        ModuleError::Parse { module, err } => {
            if module == entry_path {
                format_parse_error(module, entry_source, err)
            } else {
                match std::fs::read_to_string(module) {
                    Ok(source) => format_parse_error(module, &source, err),
                    Err(_) => format!("in module '{module}': {err}"),
                }
            }
        }
        _ => err.to_string(),
    }
}

/// Run a stage that operates on the **linked program** (so `from module import …`
/// is resolved when a file path is given). Used by `check`/`own`.
fn program_stage(
    name: &str,
    file: Option<&str>,
    link_options: &LinkOptions,
    f: fn(&[Stmt]) -> Result<(), String>,
) -> ExitCode {
    let program = match load_program(file, link_options) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{name} error: {e}");
            return ExitCode::FAILURE;
        }
    };
    match f(&program) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{name} error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `run`, routed through the selected backend (over the linked program).
fn stage_run(file: Option<&str>, cli: &CliArgs) -> ExitCode {
    let result = match cli.backend {
        BackendKind::Pliron => run_program_native(file, cli),
        backend => run_program(file, backend, &cli.link_options),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("run error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `run --backend pliron`: compile the program natively (the advertised
/// subset — scalars, string literals, and `print` through the runtime ABI;
/// anything outside it rejects with the backend's contextual diagnostic,
/// never a VM fallback), link a temporary executable, run it, and forward its
/// output and exit status. Checked native traps map back to the VM's
/// runtime-error text for parity.
#[cfg(feature = "backend-pliron")]
fn run_program_native(file: Option<&str>, cli: &CliArgs) -> Result<(), String> {
    use mojito::backend::pliron;

    let opt = native_opt_level(cli)?;
    let mut module = compile_native_module(file, cli)?;
    let exe = std::env::temp_dir().join(format!(".mojito-run.{}", std::process::id()));
    let ran = module
        .write_executable(&exe, opt)
        .map_err(|e| e.to_string())
        .and_then(|()| {
            // `output()` would silently null the child's stdin; `input()`
            // programs must see the CLI's own stdin (EOF included).
            std::process::Command::new(&exe)
                .stdin(std::process::Stdio::inherit())
                .output()
                .map_err(|e| format!("cannot run native executable: {e}"))
        });
    let _ = std::fs::remove_file(&exe);
    let output = ran?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    let trap = output
        .status
        .code()
        .and_then(pliron::TrapCategory::from_exit_code);
    // A recognized trap already reported itself on the executable's stderr;
    // suppress that line and re-render the category as the CLI diagnostic
    // (VM-parity text when the category has a VM analog).
    if trap.is_none() {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
    }
    match output.status.code() {
        Some(0) => Ok(()),
        Some(code) => match trap {
            // The runtime wrote `unhandled error: <message>` to stderr;
            // re-render that exact text as the CLI diagnostic — byte parity
            // with the VM's `RuntimeError::Raised` display.
            Some(pliron::TrapCategory::UnhandledError) => {
                Err(String::from_utf8_lossy(&output.stderr)
                    .trim_end()
                    .to_string())
            }
            Some(category) => match category.vm_message() {
                Some(message) => Err(format!("Type error: {message}")),
                None => Err(format!(
                    "native runtime trap: {}",
                    category.runtime_message()
                )),
            },
            None => Err(format!("native executable exited with status {code}")),
        },
        None => Err("native executable terminated by a signal".to_string()),
    }
}

#[cfg(not(feature = "backend-pliron"))]
fn run_program_native(_file: Option<&str>, _cli: &CliArgs) -> Result<(), String> {
    Err(
        "this mojito build lacks the `backend-pliron` feature; rebuild with \
         `cargo build --features backend-pliron` (requires LLVM 22 — see \
         docs/notes/pliron-stage0.md)"
            .to_string(),
    )
}

fn run_program(
    file: Option<&str>,
    backend: BackendKind,
    link_options: &LinkOptions,
) -> Result<(), String> {
    let compiler = Compiler::new(link_options.clone(), backend);
    let compiled = compile_input(&compiler, file)?;
    let execution = compiler
        .execute(&compiled)
        .map_err(|error| error.to_string())?;
    if !execution.output.is_empty() {
        print!("{}", execution.output);
    }
    // Echo final bindings only for stdin snippets: file-based programs run
    // through `main`, and their observable output must match `mojo run`
    // exactly (a module-scope `comptime` value would otherwise leak an extra
    // `NAME = value` line into differential comparisons).
    if !matches!(file, Some(path) if path != "-") {
        for (n, v) in execution.bindings {
            println!("{n} = {v}");
        }
    }
    Ok(())
}

/// `emit-mir`: compile source through ownership analysis and print the exact
/// post-drop artifact accepted by `exec` and future backends.
fn stage_emit_mir(file: Option<&str>, link_options: &LinkOptions) -> ExitCode {
    let compiler = Compiler::new(link_options.clone(), BackendKind::Vm);
    let result = compile_input(&compiler, file)
        .and_then(|compiled| compiled.emit_mir().map_err(|error| error.to_string()));
    match result {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("emit-mir error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `compile`: native compilation of the scalar subset through the Pliron
/// backend. Text kinds (`plir`, `ll`) print to stdout unless `--output` is
/// given; binary kinds (`bc`, `obj`, `exe`) require `--output`.
#[cfg(feature = "backend-pliron")]
fn stage_compile(file: Option<&str>, cli: &CliArgs) -> ExitCode {
    match run_compile(file, cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("compile error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(feature = "backend-pliron")]
fn run_compile(file: Option<&str>, cli: &CliArgs) -> Result<(), String> {
    use mojito::backend::pliron;

    if cli.backend != BackendKind::Pliron {
        return Err(
            "compile requires --backend pliron (the only native compile backend)".to_string(),
        );
    }
    let emit = match cli.emit.as_deref() {
        Some(kind) => pliron::EmitKind::parse(kind)?,
        None => pliron::EmitKind::LlvmIr,
    };
    if emit.is_binary() && cli.output.is_none() {
        return Err("--emit bc|obj|exe requires --output PATH".to_string());
    }
    let opt = native_opt_level(cli)?;
    let mut module = compile_native_module(file, cli)?;

    let write_text = |text: &str| -> Result<(), String> {
        match &cli.output {
            Some(path) => {
                std::fs::write(path, text).map_err(|e| format!("cannot write {path}: {e}"))
            }
            None => {
                print!("{text}");
                Ok(())
            }
        }
    };
    let output_path = || Path::new(cli.output.as_deref().expect("checked for binary kinds"));
    match emit {
        pliron::EmitKind::Plir => write_text(module.plir_text()),
        pliron::EmitKind::LlvmIr => {
            let text = module.llvm_ir(opt).map_err(|e| e.to_string())?;
            write_text(&text)
        }
        pliron::EmitKind::Bitcode => module
            .write_bitcode(output_path(), opt)
            .map_err(|e| e.to_string()),
        pliron::EmitKind::Object => module
            .write_object(output_path(), opt)
            .map_err(|e| e.to_string()),
        pliron::EmitKind::Exe => module
            .write_executable(output_path(), opt)
            .map_err(|e| e.to_string()),
    }
}

/// Compile the input through the production pipeline and hand the cached
/// post-drop MIR to the Pliron backend, entering from `main` (plus
/// `__toplevel__` when present). The shared front half of `compile` and
/// `run --backend pliron`.
#[cfg(feature = "backend-pliron")]
fn compile_native_module(
    file: Option<&str>,
    cli: &CliArgs,
) -> Result<mojito::backend::pliron::NativeModule, String> {
    use mojito::backend::pliron;

    // Read the source once (stdin cannot be re-read), compile it through the
    // production pipeline, and hand the cached post-drop MIR to the backend.
    let source = read_source(file).map_err(|e| format!("cannot read input: {e}"))?;
    let label = file.unwrap_or("-").to_string();
    let compiler = Compiler::new(cli.link_options.clone(), BackendKind::Vm);
    let compiled = match file {
        Some(path) if path != "-" => compiler.compile_source(&source, Path::new(path)),
        _ => compiler.compile_unlinked(&source),
    }
    .map_err(|error| match &error {
        CompilerError::Module(module) => format_module_error(module, &label, &source),
        _ => error.to_string(),
    })?;
    let mir = compiled.elaborated_mir();

    let mut entries = vec!["main".to_string()];
    if mir.functions.iter().any(|(name, _)| name == "__toplevel__") {
        entries.push("__toplevel__".to_string());
    }
    let options = pliron::CompileOptions {
        entries,
        sources: vec![(label, source)],
        target: native_target(cli)?,
        trace_lifecycle: false,
    };
    pliron::compile(mir, &options).map_err(|error| error.display_with_sources(&options.sources))
}

/// The `--native-opt` level (default `O0`).
#[cfg(feature = "backend-pliron")]
fn native_opt_level(cli: &CliArgs) -> Result<mojito::backend::pliron::OptLevel, String> {
    match cli.native_opt.as_deref() {
        Some(level) => mojito::backend::pliron::OptLevel::parse(level),
        None => Ok(mojito::backend::pliron::OptLevel::O0),
    }
}

/// The checked `--target` (default: the build host, when supported).
#[cfg(feature = "backend-pliron")]
fn native_target(cli: &CliArgs) -> Result<mojito::native::target::NativeTarget, String> {
    use mojito::native::target::{NativeTarget, Triple};
    match cli.target.as_deref() {
        Some(triple) => Ok(NativeTarget::new(Triple::parse(triple)?)),
        None => NativeTarget::host().ok_or_else(|| {
            "this host is not a supported native target; pass an explicit --target TRIPLE"
                .to_string()
        }),
    }
}

/// `compile` in a build without the Pliron backend: explain how to get one.
#[cfg(not(feature = "backend-pliron"))]
fn stage_compile(_file: Option<&str>, _cli: &CliArgs) -> ExitCode {
    eprintln!(
        "compile error: this mojito build lacks the `backend-pliron` feature; \
         rebuild with `cargo build --features backend-pliron` \
         (requires LLVM 22 — see docs/notes/pliron-stage0.md)"
    );
    ExitCode::FAILURE
}

fn compile_input(
    compiler: &Compiler,
    file: Option<&str>,
) -> Result<mojito::CompiledProgram, String> {
    let source = read_source(file).map_err(|e| format!("cannot read input: {e}"))?;
    match file {
        Some(path) if path != "-" => compiler.compile_source(&source, Path::new(path)),
        _ => compiler.compile_unlinked(&source),
    }
    .map_err(|error| match &error {
        CompilerError::Module(module) => format_module_error(module, file.unwrap_or("-"), &source),
        _ => error.to_string(),
    })
}

/// `exec`: load a verified textual MIR artifact and execute it on the selected
/// backend. Artifacts carry no imports, so the module-root options are
/// irrelevant here; the loading gate (parse + canonical MIR verification) is
/// the artifact's semantic gate.
fn stage_exec(file: Option<&str>, backend: mojito::BackendKind) -> ExitCode {
    let bytes = match read_bytes(file) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("exec: cannot read input: {e}");
            return ExitCode::FAILURE;
        }
    };
    let label = file.unwrap_or("-");
    match mojito::run_artifact(&bytes, label, backend) {
        Ok(execution) => {
            // Artifacts are compiled programs: print their output verbatim and
            // never echo bindings, matching file-based `run`'s parity rationale.
            if !execution.output.is_empty() {
                print!("{}", execution.output);
            }
            ExitCode::SUCCESS
        }
        Err(mojito::ArtifactRunError::Load(report)) => {
            let source = std::str::from_utf8(&bytes).ok();
            for diagnostic in &report.diagnostics {
                let message = match &source {
                    Some(source) => {
                        let mut message = format_source_error(
                            label,
                            source,
                            diagnostic.span.0,
                            &diagnostic.message,
                        );
                        for context in &diagnostic.context {
                            message.push_str(&format!("\n  in {context}"));
                        }
                        message
                    }
                    None => format!(
                        "{label}: bytes {}..{}: {}",
                        diagnostic.span.0, diagnostic.span.1, diagnostic.message
                    ),
                };
                eprintln!("exec error: {message}");
            }
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("exec error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Read the named file, or standard input, as raw bytes — the artifact parser
/// owns UTF-8 validation and BOM rejection, so `exec` must not pre-decode.
fn read_bytes(file: Option<&str>) -> std::io::Result<Vec<u8>> {
    match file {
        None | Some("-") => {
            let mut bytes = Vec::new();
            std::io::stdin().read_to_end(&mut bytes)?;
            Ok(bytes)
        }
        Some(path) => std::fs::read(path),
    }
}

fn print_usage() {
    eprint!(
        "mojito — a compiler and register VM for a subset of Mojo\n\n\
         usage: mojito [COMMAND] [FILE]\n\n\
         global options:\n\
         \x20 -I, --module-path PATH  add a module search root (repeatable)\n\
         \x20 --stdlib PATH          add a stdlib search root (repeatable)\n\
         \x20 --backend NAME         select the run backend\n\
         \x20 --emit KIND            compile output: plir|ll|bc|obj|exe (default ll)\n\
         \x20 -o, --output PATH      compile output path (required for bc|obj|exe)\n\
         \x20 --native-opt LEVEL     native optimization level: 0|1 (default 0)\n\
         \x20 --target TRIPLE        native target triple (default: the host)\n\n\
         commands:\n\
         \x20 lex   [FILE]   print the token stream (one per line)\n\
         \x20 parse [FILE]   print the parsed AST\n\
         \x20 check [FILE]   type-check and report ok or the first error\n\
         \x20 run   [FILE]   evaluate and print output + final bindings\n\
         \x20 emit-mir [FILE] compile and print executable textual MIR\n\
         \x20 compile [FILE] native-compile via --backend pliron (experimental)\n\
         \x20 exec  [FILE]   execute a verified textual MIR artifact\n\
         \x20 demo           run the built-in showcase (default)\n\n\
         FILE defaults to '-' (standard input).\n"
    );
}

/// Read the named file, or standard input when it is absent or `-`.
fn read_source(file: Option<&str>) -> std::io::Result<String> {
    match file {
        None | Some("-") => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s)?;
            Ok(s)
        }
        Some(path) => std::fs::read_to_string(path),
    }
}

/// Run one stage over the source, turning any I/O or stage error into a non-zero
/// exit code with a message on stderr.
fn stage(name: &str, file: Option<&str>, f: fn(&str) -> Result<(), String>) -> ExitCode {
    let source = match read_source(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: cannot read input: {}", name, e);
            return ExitCode::FAILURE;
        }
    };
    match f(&source) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{} error: {}", name, e);
            ExitCode::FAILURE
        }
    }
}

fn stage_parse(file: Option<&str>) -> ExitCode {
    let source = match read_source(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("parse: cannot read input: {}", e);
            return ExitCode::FAILURE;
        }
    };
    let report = parse_diagnostics(&source, 20);
    if report.errors.is_empty() {
        println!("{:#?}", report.program);
        ExitCode::SUCCESS
    } else {
        for e in &report.errors {
            eprintln!(
                "parse error: {}",
                format_parse_error(file.unwrap_or("-"), &source, e)
            );
        }
        if report.truncated {
            eprintln!("parse error: stopped after 20 diagnostics");
        }
        ExitCode::FAILURE
    }
}

fn format_parse_error(label: &str, source: &str, err: &ParseError) -> String {
    let Some(byte) = err.byte_pos() else {
        return err.to_string();
    };
    format_source_error(label, source, byte, &err.to_string())
}

fn format_source_error(label: &str, source: &str, byte: usize, message: &str) -> String {
    let byte = byte.min(source.len());
    let mut line_no = 1usize;
    let mut line_start = 0usize;
    for (idx, ch) in source.char_indices() {
        if idx >= byte {
            break;
        }
        if ch == '\n' {
            line_no += 1;
            line_start = idx + 1;
        }
    }
    let line_end = source[line_start..]
        .find('\n')
        .map(|n| line_start + n)
        .unwrap_or(source.len());
    let line = source[line_start..line_end].trim_end_matches('\r');
    let col = source[line_start..byte].chars().count() + 1;
    let caret = format!("{}^", " ".repeat(col.saturating_sub(1)));
    format!("{label}:{line_no}:{col}: {message}\n{line}\n{caret}")
}

/// `lex`: print every token, one per line.
fn run_lex(source: &str) -> Result<(), String> {
    let tokens = lex(source).map_err(|e| e.to_string())?;
    for tok in tokens {
        println!("{:?}", tok);
    }
    Ok(())
}

/// `check`: type-check the linked program; report success or the first error.
/// Runs the whole static pipeline once — checking, typed-MIR verification, and
/// ownership all consume the same checked program.
fn run_check(program: &[Stmt]) -> Result<(), String> {
    mojito::validate_module_scope(program).map_err(|e| e.to_string())?;
    let checked = mojito::check_program(program).map_err(|e| e.to_string())?;
    let mir = mojito::mir::lower_checked_program(&checked);
    if !mir.invariant_errors.is_empty() {
        return Err(format!(
            "invalid checked program: {}",
            mir.invariant_errors.join("; ")
        ));
    }
    // The ownership analysis is part of a full check.
    mojito::check_ownership_program(&mir).map_err(|e| e.to_string())?;
    println!("ok");
    Ok(())
}

/// `own` — type-check, then run the ownership (move) analysis over verified
/// MIR. Reports `ok`, or the first move violation with its source byte range.
fn run_own(program: &[Stmt]) -> Result<(), String> {
    mojito::validate_module_scope(program).map_err(|e| e.to_string())?;
    let checked = mojito::check_program(program).map_err(|e| e.to_string())?;
    let mir = mojito::mir::lower_checked_program(&checked);
    if !mir.invariant_errors.is_empty() {
        return Err(format!(
            "invalid checked program: {}",
            mir.invariant_errors.join("; ")
        ));
    }
    match mojito::check_ownership_program(&mir) {
        Ok(()) => {
            println!("ok");
            Ok(())
        }
        Err(e) => {
            let (start, end) = e.span();
            Err(format!("{e} (bytes {start}..{end})"))
        }
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn module_roots_preserve_cli_order_before_bundled_stdlib() {
        let cli = parse_cli_args(vec![
            "check".into(),
            "main.mojo".into(),
            "--module-path".into(),
            "first".into(),
            "-I".into(),
            "second".into(),
            "--stdlib=third".into(),
        ])
        .unwrap();
        assert_eq!(
            &cli.link_options.search_roots[..3],
            &[
                PathBuf::from("first"),
                PathBuf::from("second"),
                PathBuf::from("third")
            ]
        );
    }

    #[test]
    fn module_root_options_require_paths() {
        assert!(parse_cli_args(vec!["check".into(), "--module-path".into()]).is_err());
        assert!(parse_cli_args(vec!["check".into(), "--stdlib=".into()]).is_err());
    }
}
