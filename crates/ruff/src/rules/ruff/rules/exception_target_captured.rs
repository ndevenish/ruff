use ruff_diagnostics::Violation;
use ruff_macros::{derive_message_formats, violation};

use ruff_python_ast::{self as ast, ExceptHandler};

use crate::checkers::ast::Checker;

/// ## What it does
/// Checks for exception target names used in a closure without being
/// assigned to a different name.
///
/// ## Why is this bad?
/// Exception target names are cleared at the end of the `except:`
/// block, which could cause a ``NameError` if the closure is executed
/// outside of the exception-handling block
///
/// ## Example
/// ```python
/// try:
///     ...
/// except Exception as e:
///     def _some_function():
///         print("Exception", e)
///     _some_function()
/// ```
///
/// Use instead:
/// ```python
/// try:
///     ...
/// except Exception as e:
///     def _some_function(e):
///         print("Exception", e)
///     _some_function(e)
/// ```
/// or
/// ```python
/// try:
///     ...
/// except Exception as e:
///     saved_exception = e
///     def _some_function(e):
///         print("Exception", saved_exception)
///     _some_function()
/// ```
#[violation]
pub struct ExceptionTargetCaptured {
    name: String,
}

impl Violation for ExceptionTargetCaptured {
    #[derive_message_formats]
    fn message(&self) -> String {
        format!(
            "`except as <target>` names should not be directly captured by a function definition"
        )
    }
}

/// RUF018
pub(crate) fn exception_target_captured(_checker: &Checker, _except_handler: &ExceptHandler) {
    // If we don't have a name, we don't need to check
    let ExceptHandler::ExceptHandler(ast::ExceptHandlerExceptHandler {
        name: Some(name),
        body,
        ..
    }) = _except_handler
    else {
        return;
    };
}
