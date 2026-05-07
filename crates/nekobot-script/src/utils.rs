use swc_core::{
    common::{FileName, GLOBALS, Mark, SourceMap, sync::Lrc},
    ecma::{
        ast::{EsVersion, Pass, Program},
        codegen::{self, Emitter, text_writer::JsWriter},
        parser::{Lexer, Parser, StringInput, Syntax, TsSyntax},
        transforms::typescript,
    },
};

/// Transpile a TypeScript module string to JavaScript (ES2020) with type stripping.
/// Returns an error if parsing, transformation, or code generation fails.
pub fn transpile(ts_code: String) -> anyhow::Result<String> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(FileName::Anon.into(), ts_code);

    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax {
            tsx: false,
            decorators: false,
            dts: false,
            ..Default::default()
        }),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );

    let mut parser = Parser::new_from(lexer);
    let module = parser
        .parse_typescript_module()
        .map_err(|e| anyhow::anyhow!("failed to parse TypeScript: {:?}", e))?;

    GLOBALS.set(&Default::default(), || -> anyhow::Result<String> {
        // --- detect `any` if needed (插入检测逻辑) ---

        let mut program = Program::Module(module);
        let top_level_mark = Mark::fresh(Mark::root());
        let unresolved_mark = Mark::fresh(Mark::root());
        let mut pass = typescript::strip(top_level_mark, unresolved_mark);
        pass.process(&mut program);

        let Program::Module(js_module) = program else {
            anyhow::bail!("unexpected program variant after type stripping");
        };

        let mut buf = vec![];
        {
            let wr = JsWriter::new(cm.clone(), "\n", &mut buf, None);
            let mut emitter = Emitter {
                cfg: codegen::Config::default().with_target(EsVersion::Es2020),
                cm,
                comments: None,
                wr,
            };
            emitter
                .emit_module(&js_module)
                .map_err(|e| anyhow::anyhow!("failed to emit JS: {e}"))?;
        }
        String::from_utf8(buf).map_err(|e| anyhow::anyhow!("invalid UTF-8: {e}"))
    })
}
