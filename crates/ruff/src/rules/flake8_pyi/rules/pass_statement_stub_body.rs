use ruff_diagnostics::{AlwaysAutofixableViolation, Diagnostic, Edit, Fix};
use ruff_macros::{derive_message_formats, violation};
use ruff_python_ast::Stmt;
use ruff_text_size::Ranged;

use crate::checkers::ast::Checker;
use crate::registry::Rule;

#[violation]
pub struct PassStatementStubBody;

impl AlwaysAutofixableViolation for PassStatementStubBody {
    #[derive_message_formats]
    fn message(&self) -> String {
        format!("Empty body should contain `...`, not `pass`")
    }

    fn autofix_title(&self) -> String {
        format!("Replace `pass` with `...`")
    }
}

/// PYI009
pub(crate) fn pass_statement_stub_body(checker: &mut Checker, body: &[Stmt]) {
    let [stmt] = body else {
        return;
    };
    if stmt.is_pass_stmt() {
        let mut diagnostic = Diagnostic::new(PassStatementStubBody, stmt.range());
        if checker.patch(Rule::PassStatementStubBody) {
            diagnostic.set_fix(Fix::automatic(Edit::range_replacement(
                format!("..."),
                stmt.range(),
            )));
        };
        checker.diagnostics.push(diagnostic);
    }
}
