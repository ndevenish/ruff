use ruff_formatter::write;
use ruff_python_ast::StmtDelete;
use ruff_text_size::Ranged;

use crate::builders::{parenthesize_if_expands, PyFormatterExtensions};
use crate::comments::{dangling_node_comments, SourceComment, SuppressionKind};
use crate::expression::maybe_parenthesize_expression;
use crate::expression::parentheses::Parenthesize;
use crate::prelude::*;

#[derive(Default)]
pub struct FormatStmtDelete;

impl FormatNodeRule<StmtDelete> for FormatStmtDelete {
    fn fmt_fields(&self, item: &StmtDelete, f: &mut PyFormatter) -> FormatResult<()> {
        let StmtDelete { range: _, targets } = item;

        write!(f, [text("del"), space()])?;

        match targets.as_slice() {
            [] => {
                write!(
                    f,
                    [
                        // Handle special case of delete statements without targets.
                        // ```
                        // del (
                        //     # Dangling comment
                        // )
                        text("("),
                        block_indent(&dangling_node_comments(item)),
                        text(")"),
                    ]
                )
            }
            [single] => {
                write!(
                    f,
                    [maybe_parenthesize_expression(
                        single,
                        item,
                        Parenthesize::IfBreaks
                    )]
                )
            }
            targets => {
                let item = format_with(|f| {
                    f.join_comma_separated(item.end())
                        .nodes(targets.iter())
                        .finish()
                });
                parenthesize_if_expands(&item).fmt(f)
            }
        }
    }

    fn fmt_dangling_comments(
        &self,
        _dangling_comments: &[SourceComment],
        _f: &mut PyFormatter,
    ) -> FormatResult<()> {
        // Handled in `fmt_fields`
        Ok(())
    }

    fn is_suppressed(
        &self,
        trailing_comments: &[SourceComment],
        context: &PyFormatContext,
    ) -> bool {
        SuppressionKind::has_skip_comment(trailing_comments, context.source())
    }
}
