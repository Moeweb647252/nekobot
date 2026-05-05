//! TypeScript processing: parse, detect `any`, strip types, emit JS.

use swc_core::common::{FileName, GLOBALS, Mark, SourceMap, sync::Lrc};
use swc_core::ecma::{
    ast::{EsVersion, Pass, Program, TsKeywordTypeKind, TsType},
    codegen::{self, Emitter, text_writer::JsWriter},
    parser::{Parser, StringInput, Syntax, TsSyntax, lexer::Lexer},
    transforms::typescript,
    visit::{Visit, VisitWith},
};

/// Parse TypeScript, reject `any` types, strip type annotations, and emit
/// the resulting JavaScript code as a string.
pub fn transpile(ts_code: &str) -> anyhow::Result<String> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        FileName::Custom("eval_ts.ts".into()).into(),
        ts_code.to_owned(),
    );

    // 1. Parse TS → AST (Module)
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
        .map_err(|e| anyhow::anyhow!("failed to parse TypeScript: {e:?}"))?;

    // Steps 2-4 must run inside GLOBALS.set (required by SWC internals).
    GLOBALS.set(&Default::default(), || -> anyhow::Result<String> {
        // 2. Detect `any` type usage
        let mut detector = AnyDetector { found: false };
        module.visit_with(&mut detector);
        if detector.found {
            anyhow::bail!("`any` type is not allowed in eval_ts");
        }

        // 3. Strip type annotations via SWC Pass
        let mut program = Program::Module(module);
        let mut pass = typescript::strip(Mark::new(), Mark::new());
        pass.process(&mut program);

        // 4. Emit JS code string
        let js_module = match program {
            Program::Module(m) => m,
            _ => anyhow::bail!("expected module after type stripping"),
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

/// AST visitor that flags any occurrence of the `any` keyword type.
struct AnyDetector {
    found: bool,
}

impl Visit for AnyDetector {
    fn visit_ts_type(&mut self, ty: &TsType) {
        if let TsType::TsKeywordType(kw) = ty
            && kw.kind == TsKeywordTypeKind::TsAnyKeyword
        {
            self.found = true;
        }
        ty.visit_children_with(self);
    }
}
