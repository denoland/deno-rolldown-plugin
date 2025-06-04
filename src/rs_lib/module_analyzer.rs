use std::sync::Arc;

use deno_error::JsErrorBox;
use deno_graph::analysis::DependencyDescriptor;
use deno_graph::analysis::DynamicArgument;
use deno_graph::analysis::DynamicDependencyDescriptor;
use deno_graph::analysis::DynamicDependencyKind;
use deno_graph::analysis::DynamicTemplatePart;
use deno_graph::analysis::ImportAttributes;
use deno_graph::analysis::ModuleAnalyzer;
use deno_graph::analysis::ModuleInfo;
use deno_graph::analysis::StaticDependencyDescriptor;
use deno_graph::analysis::StaticDependencyKind;
use deno_graph::MediaType;
use deno_graph::ModuleSpecifier;
use deno_graph::Position;
use deno_graph::PositionRange;
use oxc::allocator::Allocator;
use oxc::ast::ast::ExportAllDeclaration;
use oxc::ast::ast::ExportNamedDeclaration;
use oxc::ast::ast::Expression;
use oxc::ast::ast::ImportDeclaration;
use oxc::ast::ast::ImportExpression;
use oxc::ast_visit::walk::walk_program;
use oxc::ast_visit::Visit;
use oxc::parser::Parser;
use oxc::span::SourceType;
use oxc::span::Span;

pub struct OxcModuleAnalyzer;

// hastly generated with AI
#[async_trait::async_trait(?Send)]
impl ModuleAnalyzer for OxcModuleAnalyzer {
  async fn analyze(
    &self,
    _specifier: &ModuleSpecifier,
    source_text: Arc<str>,
    media_type: MediaType,
  ) -> Result<ModuleInfo, JsErrorBox> {
    let allocator = Allocator::default();
    let source_type = match media_type {
      MediaType::JavaScript => SourceType::default().with_unambiguous(true),
      MediaType::Mjs => SourceType::default().with_module(true),
      MediaType::Cjs => SourceType::default().with_module(false),
      MediaType::Jsx => SourceType::default().with_module(true).with_jsx(true),
      MediaType::TypeScript => SourceType::default()
        .with_unambiguous(true)
        .with_typescript(true),
      MediaType::Mts => SourceType::default()
        .with_module(true)
        .with_typescript(true),
      MediaType::Cts => SourceType::default()
        .with_module(false)
        .with_typescript(true),
      MediaType::Tsx => SourceType::default()
        .with_unambiguous(true)
        .with_typescript(true)
        .with_jsx(true),
      MediaType::Dts => SourceType::default()
        .with_unambiguous(true)
        .with_typescript(true),
      MediaType::Dmts | MediaType::Dcts => SourceType::default()
        .with_module(true)
        .with_typescript(true),
      // Unsupported media types for JS parsing
      MediaType::Json
      | MediaType::Css
      | MediaType::Html
      | MediaType::Wasm
      | MediaType::Sql
      | MediaType::SourceMap
      | MediaType::Unknown => {
        // should never hit here
        return Err(JsErrorBox::from_err(std::io::Error::new(
          std::io::ErrorKind::InvalidInput,
          format!("Unsupported media type for analysis: {media_type:?}"),
        )));
      }
    };
    let parser = Parser::new(&allocator, &source_text, source_type);
    let parser_return = parser.parse();

    let mut visitor = DependencyCollector {
      source_text: &source_text,
      dependencies: Vec::new(),
    };
    walk_program(&mut visitor, &parser_return.program);

    Ok(ModuleInfo {
      is_script: parser_return.program.source_type.is_script(),
      dependencies: visitor.dependencies,
      jsx_import_source: Default::default(),
      // not needed for bundling
      jsx_import_source_types: Default::default(),
      ts_references: Default::default(),
      self_types_specifier: Default::default(),
      jsdoc_imports: Default::default(),
    })
  }
}

struct DependencyCollector<'a> {
  source_text: &'a str,
  dependencies: Vec<DependencyDescriptor>,
}

impl<'a> Visit<'_> for DependencyCollector<'a> {
  fn visit_import_declaration(&mut self, node: &ImportDeclaration) {
    self.dependencies.push(DependencyDescriptor::Static(
      StaticDependencyDescriptor {
        kind: StaticDependencyKind::Import,
        specifier: node.source.value.to_string(),
        specifier_range: span_to_position_range(
          self.source_text,
          node.source.span,
        ),
        types_specifier: None,
        import_attributes: ImportAttributes::default(),
      },
    ));
  }

  fn visit_export_named_declaration(&mut self, node: &ExportNamedDeclaration) {
    if let Some(source) = &node.source {
      self.dependencies.push(DependencyDescriptor::Static(
        StaticDependencyDescriptor {
          kind: StaticDependencyKind::Export,
          specifier: source.value.to_string(),
          specifier_range: span_to_position_range(
            self.source_text,
            source.span,
          ),
          types_specifier: None,
          import_attributes: ImportAttributes::default(),
        },
      ));
    }
  }

  fn visit_export_all_declaration(&mut self, node: &ExportAllDeclaration) {
    self.dependencies.push(DependencyDescriptor::Static(
      StaticDependencyDescriptor {
        kind: StaticDependencyKind::Export,
        specifier: node.source.value.to_string(),
        specifier_range: span_to_position_range(
          self.source_text,
          node.source.span,
        ),
        types_specifier: None,
        import_attributes: ImportAttributes::default(),
      },
    ));
  }

  fn visit_import_expression(&mut self, node: &ImportExpression) {
    let (argument, argument_range) = match &node.source {
      Expression::StringLiteral(lit) => (
        DynamicArgument::String(lit.value.to_string()),
        span_to_position_range(self.source_text, lit.span),
      ),
      Expression::TemplateLiteral(tpl) => {
        let mut parts = Vec::new();
        for quasi in &tpl.quasis {
          parts.push(DynamicTemplatePart::String {
            value: quasi
              .value
              .cooked
              .as_ref()
              .map(|c| c.into_string())
              .unwrap_or_default(),
          });
        }
        for _expr in &tpl.expressions {
          parts.push(DynamicTemplatePart::Expr);
        }
        (
          DynamicArgument::Template(parts),
          span_to_position_range(self.source_text, tpl.span),
        )
      }
      _ => (
        DynamicArgument::Expr,
        span_to_position_range(self.source_text, node.span),
      ),
    };

    self.dependencies.push(DependencyDescriptor::Dynamic(
      DynamicDependencyDescriptor {
        kind: DynamicDependencyKind::Import,
        argument,
        argument_range,
        // todo...
        import_attributes: ImportAttributes::default(),
        types_specifier: None,
      },
    ));
  }
}

fn span_to_position_range(source: &str, span: Span) -> PositionRange {
  PositionRange {
    start: byte_index_to_position(source, span.start),
    end: byte_index_to_position(source, span.end),
  }
}

// todo: this is bad
fn byte_index_to_position(source: &str, index: u32) -> Position {
  let index = index as usize;
  let mut line = 0;
  let mut last_line_start = 0;

  for (i, b) in source.bytes().enumerate() {
    if i == index {
      break;
    }
    if b == b'\n' {
      line += 1;
      last_line_start = i + 1;
    }
  }

  Position {
    line,
    character: index - last_line_start,
  }
}
