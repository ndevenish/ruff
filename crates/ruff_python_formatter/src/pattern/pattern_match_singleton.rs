use ruff_python_ast::node::AnyNodeRef;
use ruff_python_ast::{Constant, PatternMatchSingleton};

use crate::expression::parentheses::{NeedsParentheses, OptionalParentheses};
use crate::prelude::*;

#[derive(Default)]
pub struct FormatPatternMatchSingleton;

impl FormatNodeRule<PatternMatchSingleton> for FormatPatternMatchSingleton {
    fn fmt_fields(&self, item: &PatternMatchSingleton, f: &mut PyFormatter) -> FormatResult<()> {
        match item.value {
            Constant::None => text("None").fmt(f),
            Constant::Bool(true) => text("True").fmt(f),
            Constant::Bool(false) => text("False").fmt(f),
            _ => unreachable!(),
        }
    }
}

impl NeedsParentheses for PatternMatchSingleton {
    fn needs_parentheses(
        &self,
        _parent: AnyNodeRef,
        _context: &PyFormatContext,
    ) -> OptionalParentheses {
        OptionalParentheses::Never
    }
}
