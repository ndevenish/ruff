use std::cmp::Ordering;

use ruff_python_ast::node::AnyNodeRef;
use ruff_python_ast::whitespace::indentation;
use ruff_python_ast::{self as ast, Comprehension, Expr, MatchCase, Parameters};
use ruff_python_trivia::{
    find_only_token_in_range, indentation_at_offset, SimpleToken, SimpleTokenKind, SimpleTokenizer,
};
use ruff_source_file::Locator;
use ruff_text_size::{Ranged, TextLen, TextRange};

use crate::comments::visitor::{CommentPlacement, DecoratedComment};
use crate::expression::expr_slice::{assign_comment_in_slice, ExprSliceCommentSection};
use crate::expression::expr_tuple::is_tuple_parenthesized;
use crate::other::parameters::{
    assign_argument_separator_comment_placement, find_parameter_separators,
};
use crate::pattern::pattern_match_sequence::SequenceType;

/// Manually attach comments to nodes that the default placement gets wrong.
pub(super) fn place_comment<'a>(
    comment: DecoratedComment<'a>,
    locator: &Locator,
) -> CommentPlacement<'a> {
    handle_parenthesized_comment(comment, locator)
        .or_else(|comment| handle_end_of_line_comment_around_body(comment, locator))
        .or_else(|comment| handle_own_line_comment_around_body(comment, locator))
        .or_else(|comment| handle_enclosed_comment(comment, locator))
}

/// Handle parenthesized comments. A parenthesized comment is a comment that appears within a
/// parenthesis, but not within the range of the expression enclosed by the parenthesis.
/// For example, the comment here is a parenthesized comment:
/// ```python
/// if (
///     # comment
///     True
/// ):
///     ...
/// ```
/// The parentheses enclose `True`, but the range of `True`doesn't include the `# comment`.
///
/// Default handling can get parenthesized comments wrong in a number of ways. For example, the
/// comment here is marked (by default) as a trailing comment of `x`, when it should be a leading
/// comment of `y`:
/// ```python
/// assert (
///     x
/// ), ( # comment
///     y
/// )
/// ```
///
/// Similarly, this is marked as a leading comment of `y`, when it should be a trailing comment of
/// `x`:
/// ```python
/// if (
///     x
///     # comment
/// ):
///    y
/// ```
///
/// As a generalized solution, if a comment has a preceding node and a following node, we search for
/// opening and closing parentheses between the two nodes. If we find a closing parenthesis between
/// the preceding node and the comment, then the comment is a trailing comment of the preceding
/// node. If we find an opening parenthesis between the comment and the following node, then the
/// comment is a leading comment of the following node.
fn handle_parenthesized_comment<'a>(
    comment: DecoratedComment<'a>,
    locator: &Locator,
) -> CommentPlacement<'a> {
    let Some(preceding) = comment.preceding_node() else {
        return CommentPlacement::Default(comment);
    };

    let Some(following) = comment.following_node() else {
        return CommentPlacement::Default(comment);
    };

    // TODO(charlie): Assert that there are no bogus tokens in these ranges. There are a few cases
    // where we _can_ hit bogus tokens, but the parentheses need to come before them. For example:
    // ```python
    // try:
    //     some_call()
    // except (
    //     UnformattedError
    //     # trailing comment
    // ) as err:
    //     handle_exception()
    // ```
    // Here, we lex from the end of `UnformattedError` to the start of `handle_exception()`, which
    // means we hit an "other" token at `err`. We know the parentheses must precede the `err`, but
    // this could be fixed by including `as err` in the node range.
    //
    // Another example:
    // ```python
    // @deco
    // # comment
    // def decorated():
    //     pass
    // ```
    // Here, we lex from the end of `deco` to the start of the arguments of `decorated`. We hit an
    // "other" token at `decorated`, but any parentheses must precede that.
    //
    // For now, we _can_ assert, but to do so, we stop lexing when we hit a token that precedes an
    // identifier.
    if comment.line_position().is_end_of_line() {
        let tokenizer = SimpleTokenizer::new(
            locator.contents(),
            TextRange::new(preceding.end(), comment.start()),
        );
        if tokenizer
            .skip_trivia()
            .take_while(|token| {
                !matches!(
                    token.kind,
                    SimpleTokenKind::As | SimpleTokenKind::Def | SimpleTokenKind::Class
                )
            })
            .any(|token| {
                debug_assert!(
                    !matches!(token.kind, SimpleTokenKind::Bogus),
                    "Unexpected token between nodes: `{:?}`",
                    locator.slice(TextRange::new(preceding.end(), comment.start()),)
                );

                token.kind() == SimpleTokenKind::LParen
            })
        {
            return CommentPlacement::leading(following, comment);
        }
    } else {
        let tokenizer = SimpleTokenizer::new(
            locator.contents(),
            TextRange::new(comment.end(), following.start()),
        );
        if tokenizer
            .skip_trivia()
            .take_while(|token| {
                !matches!(
                    token.kind,
                    SimpleTokenKind::As | SimpleTokenKind::Def | SimpleTokenKind::Class
                )
            })
            .any(|token| {
                debug_assert!(
                    !matches!(token.kind, SimpleTokenKind::Bogus),
                    "Unexpected token between nodes: `{:?}`",
                    locator.slice(TextRange::new(comment.end(), following.start()))
                );
                token.kind() == SimpleTokenKind::RParen
            })
        {
            return CommentPlacement::trailing(preceding, comment);
        }
    }

    CommentPlacement::Default(comment)
}

/// Handle a comment that is enclosed by a node.
fn handle_enclosed_comment<'a>(
    comment: DecoratedComment<'a>,
    locator: &Locator,
) -> CommentPlacement<'a> {
    match comment.enclosing_node() {
        AnyNodeRef::Parameters(parameters) => {
            handle_parameters_separator_comment(comment, parameters, locator).or_else(|comment| {
                if are_parameters_parenthesized(parameters, locator.contents()) {
                    handle_bracketed_end_of_line_comment(comment, locator)
                } else {
                    CommentPlacement::Default(comment)
                }
            })
        }
        AnyNodeRef::Arguments(_) | AnyNodeRef::TypeParams(_) | AnyNodeRef::PatternArguments(_) => {
            handle_bracketed_end_of_line_comment(comment, locator)
        }
        AnyNodeRef::Comprehension(comprehension) => {
            handle_comprehension_comment(comment, comprehension, locator)
        }

        AnyNodeRef::ExprAttribute(attribute) => {
            handle_attribute_comment(comment, attribute, locator)
        }
        AnyNodeRef::ExprBinOp(binary_expression) => {
            handle_trailing_binary_expression_left_or_operator_comment(
                comment,
                binary_expression,
                locator,
            )
        }
        AnyNodeRef::Keyword(keyword) => handle_keyword_comment(comment, keyword, locator),
        AnyNodeRef::PatternKeyword(pattern_keyword) => {
            handle_pattern_keyword_comment(comment, pattern_keyword, locator)
        }
        AnyNodeRef::ExprNamedExpr(_) => handle_named_expr_comment(comment, locator),
        AnyNodeRef::ExprDict(_) => handle_dict_unpacking_comment(comment, locator)
            .or_else(|comment| handle_bracketed_end_of_line_comment(comment, locator)),
        AnyNodeRef::ExprIfExp(expr_if) => handle_expr_if_comment(comment, expr_if, locator),
        AnyNodeRef::ExprSlice(expr_slice) => handle_slice_comments(comment, expr_slice, locator),
        AnyNodeRef::ExprStarred(starred) => {
            handle_trailing_expression_starred_star_end_of_line_comment(comment, starred, locator)
        }
        AnyNodeRef::ExprSubscript(expr_subscript) => {
            if let Expr::Slice(expr_slice) = expr_subscript.slice.as_ref() {
                handle_slice_comments(comment, expr_slice, locator)
            } else {
                CommentPlacement::Default(comment)
            }
        }
        AnyNodeRef::ModModule(_) => {
            handle_module_level_own_line_comment_before_class_or_function_comment(comment, locator)
        }
        AnyNodeRef::WithItem(_) => handle_with_item_comment(comment, locator),
        AnyNodeRef::PatternMatchSequence(pattern_match_sequence) => {
            if SequenceType::from_pattern(pattern_match_sequence, locator.contents())
                .is_parenthesized()
            {
                handle_bracketed_end_of_line_comment(comment, locator)
            } else {
                CommentPlacement::Default(comment)
            }
        }
        AnyNodeRef::PatternMatchClass(class) => handle_pattern_match_class_comment(comment, class),
        AnyNodeRef::PatternMatchAs(_) => handle_pattern_match_as_comment(comment, locator),
        AnyNodeRef::PatternMatchStar(_) => handle_pattern_match_star_comment(comment),
        AnyNodeRef::PatternMatchMapping(pattern) => {
            handle_bracketed_end_of_line_comment(comment, locator)
                .or_else(|comment| handle_pattern_match_mapping_comment(comment, pattern, locator))
        }
        AnyNodeRef::StmtFunctionDef(_) => handle_leading_function_with_decorators_comment(comment),
        AnyNodeRef::StmtClassDef(class_def) => {
            handle_leading_class_with_decorators_comment(comment, class_def)
        }
        AnyNodeRef::StmtImportFrom(import_from) => handle_import_from_comment(comment, import_from),
        AnyNodeRef::StmtWith(with_) => handle_with_comment(comment, with_),
        AnyNodeRef::ExprCall(_) => handle_call_comment(comment),
        AnyNodeRef::ExprConstant(_) => {
            if let Some(AnyNodeRef::ExprFString(fstring)) = comment.enclosing_parent() {
                CommentPlacement::dangling(fstring, comment)
            } else {
                CommentPlacement::Default(comment)
            }
        }
        AnyNodeRef::ExprFString(fstring) => CommentPlacement::dangling(fstring, comment),
        AnyNodeRef::ExprList(_)
        | AnyNodeRef::ExprSet(_)
        | AnyNodeRef::ExprGeneratorExp(_)
        | AnyNodeRef::ExprListComp(_)
        | AnyNodeRef::ExprSetComp(_)
        | AnyNodeRef::ExprDictComp(_) => handle_bracketed_end_of_line_comment(comment, locator),
        AnyNodeRef::ExprTuple(tuple) if is_tuple_parenthesized(tuple, locator.contents()) => {
            handle_bracketed_end_of_line_comment(comment, locator)
        }
        _ => CommentPlacement::Default(comment),
    }
}

/// Handle an end-of-line comment around a body.
fn handle_end_of_line_comment_around_body<'a>(
    comment: DecoratedComment<'a>,
    locator: &Locator,
) -> CommentPlacement<'a> {
    if comment.line_position().is_own_line() {
        return CommentPlacement::Default(comment);
    }

    // Handle comments before the first statement in a body
    // ```python
    // for x in range(10): # in the main body ...
    //     pass
    // else: # ... and in alternative bodies
    //     pass
    // ```
    if let Some(following) = comment.following_node() {
        if is_first_statement_in_body(following, comment.enclosing_node())
            && SimpleTokenizer::new(
                locator.contents(),
                TextRange::new(comment.end(), following.start()),
            )
            .skip_trivia()
            .next()
            .is_none()
        {
            return CommentPlacement::dangling(comment.enclosing_node(), comment);
        }
    }

    // Handle comments after a body
    // ```python
    // if True:
    //     pass # after the main body ...
    //
    // try:
    //     1 / 0
    // except ZeroDivisionError:
    //     print("Error") # ...  and after alternative bodies
    // ```
    // The first earlier branch filters out ambiguities e.g. around try-except-finally.
    if let Some(preceding) = comment.preceding_node() {
        if let Some(last_child) = last_child_in_body(preceding) {
            let innermost_child =
                std::iter::successors(Some(last_child), |parent| last_child_in_body(*parent))
                    .last()
                    .unwrap_or(last_child);
            return CommentPlacement::trailing(innermost_child, comment);
        }
    }

    CommentPlacement::Default(comment)
}

/// Check if the given statement is the first statement after the colon of a branch, be it in if
/// statements, for statements, after each part of a try-except-else-finally or function/class
/// definitions.
///
///
/// ```python
/// if True:    <- has body
///     a       <- first statement
///     b
/// elif b:     <- has body
///     c       <- first statement
///     d
/// else:       <- has body
///     e       <- first statement
///     f
///
/// class:      <- has body
///     a: int  <- first statement
///     b: int
///
/// ```
///
/// For nodes with multiple bodies, we check all bodies that don't have their own node. For
/// try-except-else-finally, each except branch has it's own node, so for the `StmtTry`, we check
/// the `try:`, `else:` and `finally:`, bodies, while `ExceptHandlerExceptHandler` has it's own
/// check. For for-else and while-else, we check both branches for the whole statement.
///
/// ```python
/// try:        <- has body (a)
///     6/8     <- first statement (a)
///     1/0
/// except:     <- has body (b)
///     a       <- first statement (b)
///     b
/// else:
///     c       <- first statement (a)
///     d
/// finally:
///     e       <- first statement (a)
///     f
/// ```
fn is_first_statement_in_body(statement: AnyNodeRef, has_body: AnyNodeRef) -> bool {
    match has_body {
        AnyNodeRef::StmtFor(ast::StmtFor { body, orelse, .. })
        | AnyNodeRef::StmtWhile(ast::StmtWhile { body, orelse, .. }) => {
            are_same_optional(statement, body.first())
                || are_same_optional(statement, orelse.first())
        }

        AnyNodeRef::StmtTry(ast::StmtTry {
            body,
            orelse,
            finalbody,
            ..
        }) => {
            are_same_optional(statement, body.first())
                || are_same_optional(statement, orelse.first())
                || are_same_optional(statement, finalbody.first())
        }

        AnyNodeRef::StmtIf(ast::StmtIf { body, .. })
        | AnyNodeRef::ElifElseClause(ast::ElifElseClause { body, .. })
        | AnyNodeRef::StmtWith(ast::StmtWith { body, .. })
        | AnyNodeRef::ExceptHandlerExceptHandler(ast::ExceptHandlerExceptHandler {
            body, ..
        })
        | AnyNodeRef::MatchCase(MatchCase { body, .. })
        | AnyNodeRef::StmtFunctionDef(ast::StmtFunctionDef { body, .. })
        | AnyNodeRef::StmtClassDef(ast::StmtClassDef { body, .. }) => {
            are_same_optional(statement, body.first())
        }

        AnyNodeRef::StmtMatch(ast::StmtMatch { cases, .. }) => {
            are_same_optional(statement, cases.first())
        }

        _ => false,
    }
}

/// Handles own-line comments around a body (at the end of the body, at the end of the header
/// preceding the body, or between bodies):
///
/// ```python
/// for x in y:
///     pass
///     # This should be a trailing comment of `pass` and not a leading comment of the `print`
/// # This is a dangling comment that should be remain before the `else`
/// else:
///     print("I have no comments")
///     # This should be a trailing comment of the print
/// # This is a trailing comment of the entire statement
///
/// if (
///     True
///     # This should be a trailing comment of `True` and not a leading comment of `pass`
/// ):
///     pass
/// ```
fn handle_own_line_comment_around_body<'a>(
    comment: DecoratedComment<'a>,
    locator: &Locator,
) -> CommentPlacement<'a> {
    if comment.line_position().is_end_of_line() {
        return CommentPlacement::Default(comment);
    }

    // If the following is the first child in an alternative body, this must be the last child in
    // the previous one
    let Some(preceding) = comment.preceding_node() else {
        return CommentPlacement::Default(comment);
    };

    // If there's any non-trivia token between the preceding node and the comment, than it means
    // we're past the case of the alternate branch, defer to the default rules
    // ```python
    // if a:
    //     preceding()
    //     # comment we place
    // else:
    //     # default placement comment
    //     def inline_after_else(): ...
    // ```
    let maybe_token = SimpleTokenizer::new(
        locator.contents(),
        TextRange::new(preceding.end(), comment.start()),
    )
    .skip_trivia()
    .next();
    if maybe_token.is_some() {
        return CommentPlacement::Default(comment);
    }

    // Check if we're between bodies and should attach to the following body.
    handle_own_line_comment_between_branches(comment, preceding, locator).or_else(|comment| {
        // Otherwise, there's no following branch or the indentation is too deep, so attach to the
        // recursively last statement in the preceding body with the matching indentation.
        handle_own_line_comment_after_branch(comment, preceding, locator)
    })
}

/// Handles own line comments between two branches of a node.
/// ```python
/// for x in y:
///     pass
/// # This one ...
/// else:
///     print("I have no comments")
/// # ... but not this one
/// ```
fn handle_own_line_comment_between_branches<'a>(
    comment: DecoratedComment<'a>,
    preceding: AnyNodeRef<'a>,
    locator: &Locator,
) -> CommentPlacement<'a> {
    // The following statement must be the first statement in an alternate body, otherwise check
    // if it's a comment after the final body and handle that case
    let Some(following) = comment.following_node() else {
        return CommentPlacement::Default(comment);
    };
    if !is_first_statement_in_alternate_body(following, comment.enclosing_node()) {
        return CommentPlacement::Default(comment);
    }

    // It depends on the indentation level of the comment if it is a leading comment for the
    // following branch or if it a trailing comment of the previous body's last statement.
    let comment_indentation = indentation_at_offset(comment.start(), locator)
        .unwrap_or_default()
        .len();

    let preceding_indentation = indentation(locator, &preceding).unwrap_or_default().len();

    // Compare to the last statement in the body
    match comment_indentation.cmp(&preceding_indentation) {
        Ordering::Greater => {
            // The comment might belong to an arbitrarily deeply nested inner statement
            // ```python
            // while True:
            //     def f_inner():
            //         pass
            //         # comment
            // else:
            //     print("noop")
            // ```
            CommentPlacement::Default(comment)
        }
        Ordering::Equal => {
            // The comment belongs to the last statement, unless the preceding branch has a body.
            // ```python
            // try:
            //     pass
            //     # I'm a trailing comment of the `pass`
            // except ZeroDivisionError:
            //     print()
            // # I'm a dangling comment of the try, even if the indentation matches the except
            // else:
            //     pass
            // ```
            if preceding.is_alternative_branch_with_node() {
                // The indentation is equal, but only because the preceding branch has a node. The
                // comment still belongs to the following branch, which may not have a node.
                CommentPlacement::dangling(comment.enclosing_node(), comment)
            } else {
                CommentPlacement::trailing(preceding, comment)
            }
        }
        Ordering::Less => {
            // The comment is leading on the following block
            if following.is_alternative_branch_with_node() {
                // For some alternative branches, there are nodes ...
                // ```python
                // try:
                //     pass
                // # I'm a leading comment of the `except` statement.
                // except ZeroDivisionError:
                //     print()
                // ```
                CommentPlacement::leading(following, comment)
            } else {
                // ... while for others, such as "else" of for loops and finally branches, the bodies
                // that are represented as a `Vec<Stmt>`, lacking a no node for the branch that we could
                // attach the comments to. We mark these as dangling comments and format them manually
                // in the enclosing node's formatting logic. For `try`, it's the formatters
                // responsibility to correctly identify the comments for the `finally` and `orelse`
                // block by looking at the comment's range.
                // ```python
                // for x in y:
                //     pass
                // # I'm a leading comment of the `else` branch but there's no `else` node.
                // else:
                //     print()
                // ```
                CommentPlacement::dangling(comment.enclosing_node(), comment)
            }
        }
    }
}

/// Determine where to attach an own line comment after a branch depending on its indentation
fn handle_own_line_comment_after_branch<'a>(
    comment: DecoratedComment<'a>,
    preceding_node: AnyNodeRef<'a>,
    locator: &Locator,
) -> CommentPlacement<'a> {
    let Some(last_child) = last_child_in_body(preceding_node) else {
        return CommentPlacement::Default(comment);
    };

    // We only care about the length because indentations with mixed spaces and tabs are only valid if
    // the indent-level doesn't depend on the tab width (the indent level must be the same if the tab width is 1 or 8).
    let comment_indentation = indentation_at_offset(comment.start(), locator)
        .unwrap_or_default()
        .len();

    // Keep the comment on the entire statement in case it's a trailing comment
    // ```python
    // if "first if":
    //     pass
    // elif "first elif":
    //     pass
    // # Trailing if comment
    // ```
    // Here we keep the comment a trailing comment of the `if`
    let preceding_indentation = indentation_at_offset(preceding_node.start(), locator)
        .unwrap_or_default()
        .len();
    if comment_indentation == preceding_indentation {
        return CommentPlacement::Default(comment);
    }

    let mut parent = None;
    let mut last_child_in_parent = last_child;

    loop {
        let child_indentation = indentation(locator, &last_child_in_parent)
            .unwrap_or_default()
            .len();

        // There a three cases:
        // ```python
        // if parent_body:
        //     if current_body:
        //         child_in_body()
        //         last_child_in_current_body  # may or may not have children on its own
        // # less: Comment belongs to the parent block.
        //   # less: Comment belongs to the parent block.
        //     # equal: The comment belongs to this block.
        //       # greater (but less in the next iteration)
        //         # greater: The comment belongs to the inner block.
        // ```
        match comment_indentation.cmp(&child_indentation) {
            Ordering::Less => {
                return if let Some(parent_block) = parent {
                    // Comment belongs to the parent block.
                    CommentPlacement::trailing(parent_block, comment)
                } else {
                    // The comment does not belong to this block.
                    // ```python
                    // if test:
                    //     pass
                    // # comment
                    // ```
                    CommentPlacement::Default(comment)
                };
            }
            Ordering::Equal => {
                // The comment belongs to this block.
                return CommentPlacement::trailing(last_child_in_parent, comment);
            }
            Ordering::Greater => {
                if let Some(nested_child) = last_child_in_body(last_child_in_parent) {
                    // The comment belongs to the inner block.
                    parent = Some(last_child_in_parent);
                    last_child_in_parent = nested_child;
                } else {
                    // The comment is overindented, we assign it to the most indented child we have.
                    // ```python
                    // if test:
                    //     pass
                    //       # comment
                    // ```
                    return CommentPlacement::trailing(last_child_in_parent, comment);
                }
            }
        }
    }
}

/// Attaches comments for the positional-only parameters separator `/` or the keywords-only
/// parameters separator `*` as dangling comments to the enclosing [`Parameters`] node.
///
/// See [`assign_argument_separator_comment_placement`]
fn handle_parameters_separator_comment<'a>(
    comment: DecoratedComment<'a>,
    parameters: &Parameters,
    locator: &Locator,
) -> CommentPlacement<'a> {
    let (slash, star) = find_parameter_separators(locator.contents(), parameters);
    let placement = assign_argument_separator_comment_placement(
        slash.as_ref(),
        star.as_ref(),
        comment.range(),
        comment.line_position(),
    );
    if placement.is_some() {
        return CommentPlacement::dangling(comment.enclosing_node(), comment);
    }

    CommentPlacement::Default(comment)
}

/// Handles comments between the left side and the operator of a binary expression (trailing comments of the left),
/// and trailing end-of-line comments that are on the same line as the operator.
///
/// ```python
/// a = (
///     5 # trailing left comment
///     + # trailing operator comment
///     # leading right comment
///     3
/// )
/// ```
fn handle_trailing_binary_expression_left_or_operator_comment<'a>(
    comment: DecoratedComment<'a>,
    binary_expression: &'a ast::ExprBinOp,
    locator: &Locator,
) -> CommentPlacement<'a> {
    // Only if there's a preceding node (in which case, the preceding node is `left`).
    if comment.preceding_node().is_none() || comment.following_node().is_none() {
        return CommentPlacement::Default(comment);
    }

    let between_operands_range = TextRange::new(
        binary_expression.left.end(),
        binary_expression.right.start(),
    );

    let mut tokens = SimpleTokenizer::new(locator.contents(), between_operands_range)
        .skip_trivia()
        .skip_while(|token| token.kind == SimpleTokenKind::RParen);
    let operator_offset = tokens
        .next()
        .expect("Expected a token for the operator")
        .start();

    if comment.end() < operator_offset {
        // ```python
        // a = (
        //      5
        //      # comment
        //      +
        //      3
        // )
        // ```
        CommentPlacement::trailing(binary_expression.left.as_ref(), comment)
    } else if comment.line_position().is_end_of_line() {
        // Is the operator on its own line.
        if locator.contains_line_break(TextRange::new(
            binary_expression.left.end(),
            operator_offset,
        )) && locator.contains_line_break(TextRange::new(
            operator_offset,
            binary_expression.right.start(),
        )) {
            // ```python
            // a = (
            //      5
            //      + # comment
            //      3
            // )
            // ```
            CommentPlacement::dangling(binary_expression, comment)
        } else {
            // ```python
            // a = (
            //      5
            //      +
            //      3 # comment
            // )
            // ```
            // OR
            // ```python
            // a = (
            //      5 # comment
            //      +
            //      3
            // )
            // ```
            CommentPlacement::Default(comment)
        }
    } else {
        // ```python
        // a = (
        //      5
        //      +
        //      # comment
        //      3
        // )
        // ```
        CommentPlacement::Default(comment)
    }
}

/// Handles own line comments on the module level before a class or function statement.
/// A comment only becomes the leading comment of a class or function if it isn't separated by an empty
/// line from the class. Comments that are separated by at least one empty line from the header of the
/// class are considered trailing comments of the previous statement.
///
/// This handling is necessary because Ruff inserts two empty lines before each class or function.
/// Let's take this example:
///
/// ```python
/// some = statement
/// # This should be stick to the statement above
///
///
/// # This should be split from the above by two lines
/// class MyClassWithComplexLeadingComments:
///     pass
/// ```
///
/// By default, the `# This should be stick to the statement above` would become a leading comment
/// of the `class` AND the `Suite` formatting separates the comment by two empty lines from the
/// previous statement, so that the result becomes:
///
/// ```python
/// some = statement
///
///
/// # This should be stick to the statement above
///
///
/// # This should be split from the above by two lines
/// class MyClassWithComplexLeadingComments:
///     pass
/// ```
///
/// Which is not what we want. The work around is to make the `# This should be stick to the statement above`
/// a trailing comment of the previous statement.
fn handle_module_level_own_line_comment_before_class_or_function_comment<'a>(
    comment: DecoratedComment<'a>,
    locator: &Locator,
) -> CommentPlacement<'a> {
    debug_assert!(comment.enclosing_node().is_module());
    // Only applies for own line comments on the module level...
    if comment.line_position().is_end_of_line() {
        return CommentPlacement::Default(comment);
    }

    // ... for comments with a preceding and following node,
    let (Some(preceding), Some(following)) = (comment.preceding_node(), comment.following_node())
    else {
        return CommentPlacement::Default(comment);
    };

    // ... where the following is a function or class statement.
    if !matches!(
        following,
        AnyNodeRef::StmtFunctionDef(_) | AnyNodeRef::StmtClassDef(_)
    ) {
        return CommentPlacement::Default(comment);
    }

    // Make the comment a leading comment if there's no empty line between the comment and the function / class header
    if max_empty_lines(locator.slice(TextRange::new(comment.end(), following.start()))) == 0 {
        CommentPlacement::leading(following, comment)
    } else {
        // Otherwise attach the comment as trailing comment to the previous statement
        CommentPlacement::trailing(preceding, comment)
    }
}

/// Handles the attaching comments left or right of the colon in a slice as trailing comment of the
/// preceding node or leading comment of the following node respectively.
/// ```python
/// a = "input"[
///     1 # c
///     # d
///     :2
/// ]
/// ```
fn handle_slice_comments<'a>(
    comment: DecoratedComment<'a>,
    expr_slice: &'a ast::ExprSlice,
    locator: &Locator,
) -> CommentPlacement<'a> {
    let ast::ExprSlice {
        range: _,
        lower,
        upper,
        step,
    } = expr_slice;

    // Check for `foo[ # comment`, but only if they are on the same line
    let after_lbracket = matches!(
        SimpleTokenizer::up_to_without_back_comment(comment.start(), locator.contents())
            .skip_trivia()
            .next_back(),
        Some(SimpleToken {
            kind: SimpleTokenKind::LBracket,
            ..
        })
    );
    if comment.line_position().is_end_of_line() && after_lbracket {
        // Keep comments after the opening bracket there by formatting them outside the
        // soft block indent
        // ```python
        // "a"[ # comment
        //     1:
        // ]
        // ```
        debug_assert!(
            matches!(comment.enclosing_node(), AnyNodeRef::ExprSubscript(_)),
            "{:?}",
            comment.enclosing_node()
        );
        return CommentPlacement::dangling(comment.enclosing_node(), comment);
    }

    let assignment = assign_comment_in_slice(comment.range(), locator.contents(), expr_slice);
    let node = match assignment {
        ExprSliceCommentSection::Lower => lower,
        ExprSliceCommentSection::Upper => upper,
        ExprSliceCommentSection::Step => step,
    };

    if let Some(node) = node {
        if comment.start() < node.start() {
            CommentPlacement::leading(node.as_ref(), comment)
        } else {
            // If a trailing comment is an end of line comment that's fine because we have a node
            // ahead of it
            CommentPlacement::trailing(node.as_ref(), comment)
        }
    } else {
        CommentPlacement::dangling(expr_slice, comment)
    }
}

/// Handles own line comments between the last function decorator and the *header* of the function.
/// It attaches these comments as dangling comments to the function instead of making them
/// leading argument comments.
///
/// ```python
/// @decorator
/// # leading function comment
/// def test():
///      ...
/// ```
fn handle_leading_function_with_decorators_comment(comment: DecoratedComment) -> CommentPlacement {
    let is_preceding_decorator = comment
        .preceding_node()
        .is_some_and(|node| node.is_decorator());

    let is_following_parameters = comment
        .following_node()
        .is_some_and(|node| node.is_parameters());

    if comment.line_position().is_own_line() && is_preceding_decorator && is_following_parameters {
        CommentPlacement::dangling(comment.enclosing_node(), comment)
    } else {
        CommentPlacement::Default(comment)
    }
}

/// Handle comments between decorators and the decorated node.
///
/// For example, given:
/// ```python
/// @dataclass
/// # comment
/// class Foo(Bar):
///     ...
/// ```
///
/// The comment should be attached to the enclosing [`ast::StmtClassDef`] as a dangling node,
/// as opposed to being treated as a leading comment on `Bar` or similar.
fn handle_leading_class_with_decorators_comment<'a>(
    comment: DecoratedComment<'a>,
    class_def: &'a ast::StmtClassDef,
) -> CommentPlacement<'a> {
    if comment.line_position().is_own_line() && comment.start() < class_def.name.start() {
        if let Some(decorator) = class_def.decorator_list.last() {
            if decorator.end() < comment.start() {
                return CommentPlacement::dangling(class_def, comment);
            }
        }
    }
    CommentPlacement::Default(comment)
}

/// Handles comments between a keyword's identifier and value:
/// ```python
/// func(
///     x  # dangling
///     =  # dangling
///     # dangling
///     1,
///     **  # dangling
///     y
/// )
/// ```
fn handle_keyword_comment<'a>(
    comment: DecoratedComment<'a>,
    keyword: &'a ast::Keyword,
    locator: &Locator,
) -> CommentPlacement<'a> {
    let start = keyword.arg.as_ref().map_or(keyword.start(), Ranged::end);

    // If the comment is parenthesized, it should be attached to the value:
    // ```python
    // func(
    //     x=(  # comment
    //         1
    //     )
    // )
    // ```
    let mut tokenizer =
        SimpleTokenizer::new(locator.contents(), TextRange::new(start, comment.start()));
    if tokenizer.any(|token| token.kind == SimpleTokenKind::LParen) {
        return CommentPlacement::Default(comment);
    }

    CommentPlacement::leading(comment.enclosing_node(), comment)
}

/// Handles comments between a pattern keyword's identifier and value:
/// ```python
/// case Point2D(
///     x  # dangling
///     =  # dangling
///     # dangling
///     1
/// )
/// ```
fn handle_pattern_keyword_comment<'a>(
    comment: DecoratedComment<'a>,
    pattern_keyword: &'a ast::PatternKeyword,
    locator: &Locator,
) -> CommentPlacement<'a> {
    // If the comment is parenthesized, it should be attached to the value:
    // ```python
    // case Point2D(
    //     x=(  # comment
    //         1
    //     )
    // )
    // ```
    let mut tokenizer = SimpleTokenizer::new(
        locator.contents(),
        TextRange::new(pattern_keyword.attr.end(), comment.start()),
    );
    if tokenizer.any(|token| token.kind == SimpleTokenKind::LParen) {
        return CommentPlacement::Default(comment);
    }

    CommentPlacement::leading(comment.enclosing_node(), comment)
}

/// Handles comments between `**` and the variable name in dict unpacking
/// It attaches these to the appropriate value node.
///
/// ```python
/// {
///     **  # comment between `**` and the variable name
///     value
///     ...
/// }
/// ```
fn handle_dict_unpacking_comment<'a>(
    comment: DecoratedComment<'a>,
    locator: &Locator,
) -> CommentPlacement<'a> {
    debug_assert!(matches!(comment.enclosing_node(), AnyNodeRef::ExprDict(_)));

    // no node after our comment so we can't be between `**` and the name (node)
    let Some(following) = comment.following_node() else {
        return CommentPlacement::Default(comment);
    };

    // we look at tokens between the previous node (or the start of the dict)
    // and the comment
    let preceding_end = match comment.preceding_node() {
        Some(preceding) => preceding.end(),
        None => comment.enclosing_node().start(),
    };
    let mut tokens = SimpleTokenizer::new(
        locator.contents(),
        TextRange::new(preceding_end, comment.start()),
    )
    .skip_trivia()
    .skip_while(|token| token.kind == SimpleTokenKind::RParen);

    // if the remaining tokens from the previous node are exactly `**`,
    // re-assign the comment to the one that follows the stars
    if tokens.any(|token| token.kind == SimpleTokenKind::DoubleStar) {
        CommentPlacement::leading(following, comment)
    } else {
        CommentPlacement::Default(comment)
    }
}

/// Handle comments between a function call and its arguments. For example, attach the following as
/// dangling on the call:
/// ```python
/// (
///   func
///   # dangling
///   ()
/// )
/// ```
fn handle_call_comment(comment: DecoratedComment) -> CommentPlacement {
    if comment.line_position().is_own_line() {
        if comment.preceding_node().is_some_and(|preceding| {
            comment.following_node().is_some_and(|following| {
                preceding.end() < comment.start() && comment.end() < following.start()
            })
        }) {
            return CommentPlacement::dangling(comment.enclosing_node(), comment);
        }
    }

    CommentPlacement::Default(comment)
}

/// Own line comments coming after the node are always dangling comments
/// ```python
/// (
///      a  # trailing comment on `a`
///      # dangling comment on the attribute
///      . # dangling comment on the attribute
///      # dangling comment on the attribute
///      b
/// )
/// ```
fn handle_attribute_comment<'a>(
    comment: DecoratedComment<'a>,
    attribute: &'a ast::ExprAttribute,
    locator: &Locator,
) -> CommentPlacement<'a> {
    if comment.preceding_node().is_none() {
        // ```text
        // (    value)   .   attr
        //  ^^^^ we're in this range
        // ```
        return CommentPlacement::leading(attribute.value.as_ref(), comment);
    }

    // If the comment is parenthesized, use the parentheses to either attach it as a trailing
    // comment on the value or a dangling comment on the attribute.
    // For example, treat this as trailing:
    // ```python
    // (
    //     (
    //         value
    //         # comment
    //     )
    //     .attribute
    // )
    // ```
    //
    // However, treat this as dangling:
    // ```python
    // (
    //     (value)
    //     # comment
    //     .attribute
    // )
    // ```
    if let Some(right_paren) = SimpleTokenizer::starts_at(attribute.value.end(), locator.contents())
        .skip_trivia()
        .take_while(|token| token.kind == SimpleTokenKind::RParen)
        .last()
    {
        return if comment.start() < right_paren.start() {
            CommentPlacement::trailing(attribute.value.as_ref(), comment)
        } else {
            CommentPlacement::dangling(comment.enclosing_node(), comment)
        };
    }

    // If the comment precedes the `.`, treat it as trailing _if_ it's on the same line as the
    // value. For example, treat this as trailing:
    // ```python
    // (
    //     value  # comment
    //     .attribute
    // )
    // ```
    //
    // However, treat this as dangling:
    // ```python
    // (
    //     value
    //     # comment
    //     .attribute
    // )
    // ```
    if comment.line_position().is_end_of_line() {
        let dot_token = find_only_token_in_range(
            TextRange::new(attribute.value.end(), attribute.attr.start()),
            SimpleTokenKind::Dot,
            locator.contents(),
        );
        if comment.end() < dot_token.start() {
            return CommentPlacement::trailing(attribute.value.as_ref(), comment);
        }
    }

    CommentPlacement::dangling(comment.enclosing_node(), comment)
}

/// Assign comments between `if` and `test` and `else` and `orelse` as leading to the respective
/// node.
///
/// ```python
/// x = (
///     "a"
///     if # leading comment of `True`
///     True
///     else # leading comment of `"b"`
///     "b"
/// )
/// ```
///
/// This placement ensures comments remain in their previous order. This an edge case that only
/// happens if the comments are in a weird position but it also doesn't hurt handling it.
fn handle_expr_if_comment<'a>(
    comment: DecoratedComment<'a>,
    expr_if: &'a ast::ExprIfExp,
    locator: &Locator,
) -> CommentPlacement<'a> {
    let ast::ExprIfExp {
        range: _,
        test,
        body,
        orelse,
    } = expr_if;

    if comment.line_position().is_own_line() {
        return CommentPlacement::Default(comment);
    }

    let if_token = find_only_token_in_range(
        TextRange::new(body.end(), test.start()),
        SimpleTokenKind::If,
        locator.contents(),
    );
    // Between `if` and `test`
    if if_token.start() < comment.start() && comment.start() < test.start() {
        return CommentPlacement::leading(test.as_ref(), comment);
    }

    let else_token = find_only_token_in_range(
        TextRange::new(test.end(), orelse.start()),
        SimpleTokenKind::Else,
        locator.contents(),
    );
    // Between `else` and `orelse`
    if else_token.start() < comment.start() && comment.start() < orelse.start() {
        return CommentPlacement::leading(orelse.as_ref(), comment);
    }

    CommentPlacement::Default(comment)
}

/// Handles trailing comments on between the `*` of a starred expression and the
/// expression itself. For example, attaches the first two comments here as leading
/// comments on the enclosing node, and the third to the `True` node.
/// ``` python
/// call(
///     *  # dangling end-of-line comment
///     # dangling own line comment
///     (  # leading comment on the expression
///        True
///     )
/// )
/// ```
fn handle_trailing_expression_starred_star_end_of_line_comment<'a>(
    comment: DecoratedComment<'a>,
    starred: &'a ast::ExprStarred,
    locator: &Locator,
) -> CommentPlacement<'a> {
    if comment.following_node().is_some() {
        let tokenizer = SimpleTokenizer::new(
            locator.contents(),
            TextRange::new(starred.start(), comment.start()),
        );
        if !tokenizer
            .skip_trivia()
            .any(|token| token.kind() == SimpleTokenKind::LParen)
        {
            return CommentPlacement::leading(starred, comment);
        }
    }

    CommentPlacement::Default(comment)
}

/// Handles trailing own line comments before the `as` keyword of a with item and
/// end of line comments that are on the same line as the `as` keyword:
///
/// ```python
/// with (
///     a
///     # trailing a own line comment
///     as # trailing as same line comment
///     b
/// ): ...
/// ```
fn handle_with_item_comment<'a>(
    comment: DecoratedComment<'a>,
    locator: &Locator,
) -> CommentPlacement<'a> {
    debug_assert!(comment.enclosing_node().is_with_item());

    // Needs to be a with item with an `as` expression.
    let (Some(context_expr), Some(optional_vars)) =
        (comment.preceding_node(), comment.following_node())
    else {
        return CommentPlacement::Default(comment);
    };

    let as_token = find_only_token_in_range(
        TextRange::new(context_expr.end(), optional_vars.start()),
        SimpleTokenKind::As,
        locator.contents(),
    );

    if comment.end() < as_token.start() {
        // If before the `as` keyword, then it must be a trailing comment of the context expression.
        CommentPlacement::trailing(context_expr, comment)
    } else if comment.line_position().is_end_of_line() {
        // Trailing end of line comment coming after the `as` keyword`.
        CommentPlacement::dangling(comment.enclosing_node(), comment)
    } else {
        CommentPlacement::leading(optional_vars, comment)
    }
}

/// Handles trailing comments between the class name and its arguments in:
/// ```python
/// case (
///     Pattern
///     # dangling
///     (...)
/// ): ...
/// ```
fn handle_pattern_match_class_comment<'a>(
    comment: DecoratedComment<'a>,
    class: &'a ast::PatternMatchClass,
) -> CommentPlacement<'a> {
    if class.cls.end() < comment.start() && comment.end() < class.arguments.start() {
        CommentPlacement::dangling(comment.enclosing_node(), comment)
    } else {
        CommentPlacement::Default(comment)
    }
}

/// Handles trailing comments after the `as` keyword of a pattern match item:
///
/// ```python
/// case (
///     pattern
///     as # dangling end of line comment
///     # dangling own line comment
///     name
/// ): ...
/// ```
fn handle_pattern_match_as_comment<'a>(
    comment: DecoratedComment<'a>,
    locator: &Locator,
) -> CommentPlacement<'a> {
    debug_assert!(comment.enclosing_node().is_pattern_match_as());

    let Some(pattern) = comment.preceding_node() else {
        return CommentPlacement::Default(comment);
    };

    let mut tokens = SimpleTokenizer::starts_at(pattern.end(), locator.contents())
        .skip_trivia()
        .skip_while(|token| token.kind == SimpleTokenKind::RParen);

    let Some(as_token) = tokens
        .next()
        .filter(|token| token.kind == SimpleTokenKind::As)
    else {
        return CommentPlacement::Default(comment);
    };

    if comment.end() < as_token.start() {
        // If before the `as` keyword, then it must be a trailing comment of the pattern.
        CommentPlacement::trailing(pattern, comment)
    } else {
        // Otherwise, must be a dangling comment. (Any comments that follow the name will be
        // trailing comments on the pattern match item, rather than enclosed by it.)
        CommentPlacement::dangling(comment.enclosing_node(), comment)
    }
}

/// Handles dangling comments between the `*` token and identifier of a pattern match star:
///
/// ```python
/// case [
///   ...,
///   *  # dangling end of line comment
///   # dangling end of line comment
///   rest,
/// ]: ...
/// ```
fn handle_pattern_match_star_comment(comment: DecoratedComment) -> CommentPlacement {
    CommentPlacement::dangling(comment.enclosing_node(), comment)
}

/// Handles trailing comments after the `**` in a pattern match item. The comments can either
/// appear between the `**` and the identifier, or after the identifier (which is just an
/// identifier, not a node).
///
/// ```python
/// case {
///     **  # dangling end of line comment
///     # dangling own line comment
///     rest  # dangling end of line comment
///     # dangling own line comment
/// ): ...
/// ```
fn handle_pattern_match_mapping_comment<'a>(
    comment: DecoratedComment<'a>,
    pattern: &'a ast::PatternMatchMapping,
    locator: &Locator,
) -> CommentPlacement<'a> {
    // The `**` has to come at the end, so there can't be another node after it. (The identifier,
    // like `rest` above, isn't a node.)
    if comment.following_node().is_some() {
        return CommentPlacement::Default(comment);
    };

    // If there's no rest pattern, no need to do anything special.
    let Some(rest) = pattern.rest.as_ref() else {
        return CommentPlacement::Default(comment);
    };

    // If the comment falls after the `**rest` entirely, treat it as dangling on the enclosing
    // node.
    if comment.start() > rest.end() {
        return CommentPlacement::dangling(comment.enclosing_node(), comment);
    }

    // Look at the tokens between the previous node (or the start of the pattern) and the comment.
    let preceding_end = match comment.preceding_node() {
        Some(preceding) => preceding.end(),
        None => comment.enclosing_node().start(),
    };
    let mut tokens = SimpleTokenizer::new(
        locator.contents(),
        TextRange::new(preceding_end, comment.start()),
    )
    .skip_trivia();

    // If the remaining tokens from the previous node include `**`, mark as a dangling comment.
    if tokens.any(|token| token.kind == SimpleTokenKind::DoubleStar) {
        CommentPlacement::dangling(comment.enclosing_node(), comment)
    } else {
        CommentPlacement::Default(comment)
    }
}

/// Handles comments around the `:=` token in a named expression (walrus operator).
///
/// For example, here, `# 1` and `# 2` will be marked as dangling comments on the named expression,
/// while `# 3` and `4` will be attached `y` (via our general parenthesized comment handling), and
/// `# 5` will be a trailing comment on the named expression.
///
/// ```python
/// if (
///     x
///     :=  # 1
///     # 2
///     (  # 3
///         y  # 4
///     ) # 5
/// ):
///     pass
/// ```
fn handle_named_expr_comment<'a>(
    comment: DecoratedComment<'a>,
    locator: &Locator,
) -> CommentPlacement<'a> {
    debug_assert!(comment.enclosing_node().is_expr_named_expr());

    let (Some(target), Some(value)) = (comment.preceding_node(), comment.following_node()) else {
        return CommentPlacement::Default(comment);
    };

    let colon_equal = find_only_token_in_range(
        TextRange::new(target.end(), value.start()),
        SimpleTokenKind::ColonEqual,
        locator.contents(),
    );

    if comment.end() < colon_equal.start() {
        // If the comment is before the `:=` token, then it must be a trailing comment of the
        // target.
        CommentPlacement::trailing(target, comment)
    } else {
        // Otherwise, treat it as dangling. We effectively treat it as a comment on the `:=` itself.
        CommentPlacement::dangling(comment.enclosing_node(), comment)
    }
}

/// Attach an end-of-line comment immediately following an open bracket as a dangling comment on
/// enclosing node.
///
/// For example, given  the following function call:
/// ```python
/// foo(  # comment
///    bar,
/// )
/// ```
///
/// The comment will be attached to the [`Arguments`] node as a dangling comment, to ensure
/// that it remains on the same line as open parenthesis.
///
/// Similarly, given:
/// ```python
/// type foo[  # comment
///    bar,
/// ] = ...
/// ```
///
/// The comment will be attached to the [`TypeParams`] node as a dangling comment, to ensure
/// that it remains on the same line as open bracket.
fn handle_bracketed_end_of_line_comment<'a>(
    comment: DecoratedComment<'a>,
    locator: &Locator,
) -> CommentPlacement<'a> {
    if comment.line_position().is_end_of_line() {
        // Ensure that there are no tokens between the open bracket and the comment.
        let mut lexer = SimpleTokenizer::new(
            locator.contents(),
            TextRange::new(comment.enclosing_node().start(), comment.start()),
        )
        .skip_trivia();

        // Skip the opening parenthesis.
        let Some(paren) = lexer.next() else {
            return CommentPlacement::Default(comment);
        };
        debug_assert!(matches!(
            paren.kind(),
            SimpleTokenKind::LParen | SimpleTokenKind::LBrace | SimpleTokenKind::LBracket
        ));

        // If there are no additional tokens between the open parenthesis and the comment, then
        // it should be attached as a dangling comment on the brackets, rather than a leading
        // comment on the first argument.
        if lexer.next().is_none() {
            return CommentPlacement::dangling(comment.enclosing_node(), comment);
        }
    }

    CommentPlacement::Default(comment)
}

/// Attach an enclosed end-of-line comment to a [`ast::StmtImportFrom`].
///
/// For example, given:
/// ```python
/// from foo import (  # comment
///    bar,
/// )
/// ```
///
/// The comment will be attached to the [`ast::StmtImportFrom`] node as a dangling comment, to
/// ensure that it remains on the same line as the [`ast::StmtImportFrom`] itself.
fn handle_import_from_comment<'a>(
    comment: DecoratedComment<'a>,
    import_from: &'a ast::StmtImportFrom,
) -> CommentPlacement<'a> {
    // The comment needs to be on the same line, but before the first member. For example, we want
    // to treat this as a dangling comment:
    // ```python
    // from foo import (  # comment
    //     bar,
    //     baz,
    //     qux,
    // )
    // ```
    // However, this should _not_ be treated as a dangling comment:
    // ```python
    // from foo import (bar,  # comment
    //     baz,
    //     qux,
    // )
    // ```
    // Thus, we check whether the comment is an end-of-line comment _between_ the start of the
    // statement and the first member. If so, the only possible position is immediately following
    // the open parenthesis.
    if comment.line_position().is_end_of_line()
        && import_from.names.first().is_some_and(|first_name| {
            import_from.start() < comment.start() && comment.start() < first_name.start()
        })
    {
        CommentPlacement::dangling(comment.enclosing_node(), comment)
    } else {
        CommentPlacement::Default(comment)
    }
}

/// Attach an enclosed end-of-line comment to a [`ast::StmtWith`].
///
/// For example, given:
/// ```python
/// with ( # foo
///     CtxManager1() as example1,
///     CtxManager2() as example2,
///     CtxManager3() as example3,
/// ):
///     ...
/// ```
///
/// The comment will be attached to the [`ast::StmtWith`] node as a dangling comment, to ensure
/// that it remains on the same line as the [`ast::StmtWith`] itself.
fn handle_with_comment<'a>(
    comment: DecoratedComment<'a>,
    with_statement: &'a ast::StmtWith,
) -> CommentPlacement<'a> {
    if comment.line_position().is_end_of_line()
        && with_statement.items.first().is_some_and(|with_item| {
            with_statement.start() < comment.start() && comment.start() < with_item.start()
        })
    {
        CommentPlacement::dangling(comment.enclosing_node(), comment)
    } else {
        CommentPlacement::Default(comment)
    }
}

/// Handle comments inside comprehensions, e.g.
///
/// ```python
/// [
///      a
///      for  # dangling on the comprehension
///      b
///      # dangling on the comprehension
///      in  # dangling on comprehension.iter
///      # leading on the iter
///      c
///      # dangling on comprehension.if.n
///      if  # dangling on comprehension.if.n
///      d
/// ]
/// ```
fn handle_comprehension_comment<'a>(
    comment: DecoratedComment<'a>,
    comprehension: &'a Comprehension,
    locator: &Locator,
) -> CommentPlacement<'a> {
    let is_own_line = comment.line_position().is_own_line();

    // Comments between the `for` and target
    // ```python
    // [
    //      a
    //      for  # attach as dangling on the comprehension
    //      b in c
    //  ]
    // ```
    if comment.end() < comprehension.target.start() {
        return if is_own_line {
            // own line comments are correctly assigned as leading the target
            CommentPlacement::Default(comment)
        } else {
            // after the `for`
            CommentPlacement::dangling(comment.enclosing_node(), comment)
        };
    }

    let in_token = find_only_token_in_range(
        TextRange::new(comprehension.target.end(), comprehension.iter.start()),
        SimpleTokenKind::In,
        locator.contents(),
    );

    // Comments between the target and the `in`
    // ```python
    // [
    //      a for b
    //      # attach as dangling on the target
    //      # (to be rendered as leading on the "in")
    //      in c
    //  ]
    // ```
    if comment.start() < in_token.start() {
        // attach as dangling comments on the target
        // (to be rendered as leading on the "in")
        return if is_own_line {
            CommentPlacement::dangling(comment.enclosing_node(), comment)
        } else {
            // correctly trailing on the target
            CommentPlacement::Default(comment)
        };
    }

    // Comments between the `in` and the iter
    // ```python
    // [
    //      a for b
    //      in  #  attach as dangling on the iter
    //      c
    //  ]
    // ```
    if comment.start() < comprehension.iter.start() {
        return if is_own_line {
            CommentPlacement::Default(comment)
        } else {
            // after the `in` but same line, turn into trailing on the `in` token
            CommentPlacement::dangling(&comprehension.iter, comment)
        };
    }

    let mut last_end = comprehension.iter.end();

    for if_node in &comprehension.ifs {
        // ```python
        // [
        //     a
        //     for
        //     c
        //     in
        //     e
        //     # above if   <-- find these own-line between previous and `if` token
        //     if  # if     <-- find these end-of-line between `if` and if node (`f`)
        //     # above f    <-- already correctly assigned as leading `f`
        //     f  # f       <-- already correctly assigned as trailing `f`
        //     # above if2
        //     if  # if2
        //     # above g
        //     g  # g
        // ]
        // ```
        let if_token = find_only_token_in_range(
            TextRange::new(last_end, if_node.start()),
            SimpleTokenKind::If,
            locator.contents(),
        );
        if is_own_line {
            if last_end < comment.start() && comment.start() < if_token.start() {
                return CommentPlacement::dangling(if_node, comment);
            }
        } else if if_token.start() < comment.start() && comment.start() < if_node.start() {
            return CommentPlacement::dangling(if_node, comment);
        }
        last_end = if_node.end();
    }

    CommentPlacement::Default(comment)
}

/// Returns `true` if `right` is `Some` and `left` and `right` are referentially equal.
fn are_same_optional<'a, T>(left: AnyNodeRef, right: Option<T>) -> bool
where
    T: Into<AnyNodeRef<'a>>,
{
    right.is_some_and(|right| left.ptr_eq(right.into()))
}

/// The last child of the last branch, if the node has multiple branches.
fn last_child_in_body(node: AnyNodeRef) -> Option<AnyNodeRef> {
    let body = match node {
        AnyNodeRef::StmtFunctionDef(ast::StmtFunctionDef { body, .. })
        | AnyNodeRef::StmtClassDef(ast::StmtClassDef { body, .. })
        | AnyNodeRef::StmtWith(ast::StmtWith { body, .. })
        | AnyNodeRef::MatchCase(MatchCase { body, .. })
        | AnyNodeRef::ExceptHandlerExceptHandler(ast::ExceptHandlerExceptHandler {
            body, ..
        })
        | AnyNodeRef::ElifElseClause(ast::ElifElseClause { body, .. }) => body,
        AnyNodeRef::StmtIf(ast::StmtIf {
            body,
            elif_else_clauses,
            ..
        }) => elif_else_clauses.last().map_or(body, |clause| &clause.body),

        AnyNodeRef::StmtFor(ast::StmtFor { body, orelse, .. })
        | AnyNodeRef::StmtWhile(ast::StmtWhile { body, orelse, .. }) => {
            if orelse.is_empty() {
                body
            } else {
                orelse
            }
        }

        AnyNodeRef::StmtMatch(ast::StmtMatch { cases, .. }) => {
            return cases.last().map(AnyNodeRef::from);
        }

        AnyNodeRef::StmtTry(ast::StmtTry {
            body,
            handlers,
            orelse,
            finalbody,
            ..
        }) => {
            if finalbody.is_empty() {
                if orelse.is_empty() {
                    if handlers.is_empty() {
                        body
                    } else {
                        return handlers.last().map(AnyNodeRef::from);
                    }
                } else {
                    orelse
                }
            } else {
                finalbody
            }
        }

        // Not a node that contains an indented child node.
        _ => return None,
    };

    body.last().map(AnyNodeRef::from)
}

/// Returns `true` if `statement` is the first statement in an alternate `body` (e.g. the else of an if statement)
fn is_first_statement_in_alternate_body(statement: AnyNodeRef, has_body: AnyNodeRef) -> bool {
    match has_body {
        AnyNodeRef::StmtFor(ast::StmtFor { orelse, .. })
        | AnyNodeRef::StmtWhile(ast::StmtWhile { orelse, .. }) => {
            are_same_optional(statement, orelse.first())
        }

        AnyNodeRef::StmtTry(ast::StmtTry {
            handlers,
            orelse,
            finalbody,
            ..
        }) => {
            are_same_optional(statement, handlers.first())
                || are_same_optional(statement, orelse.first())
                || are_same_optional(statement, finalbody.first())
        }

        AnyNodeRef::StmtIf(ast::StmtIf {
            elif_else_clauses, ..
        }) => are_same_optional(statement, elif_else_clauses.first()),
        _ => false,
    }
}

/// Returns `true` if the parameters are parenthesized (as in a function definition), or `false` if
/// not (as in a lambda).
fn are_parameters_parenthesized(parameters: &Parameters, contents: &str) -> bool {
    // A lambda never has parentheses around its parameters, but a function definition always does.
    contents[parameters.range()].starts_with('(')
}

/// Counts the number of empty lines in `contents`.
fn max_empty_lines(contents: &str) -> u32 {
    let mut newlines = 0u32;
    let mut max_new_lines = 0;

    for token in SimpleTokenizer::new(contents, TextRange::up_to(contents.text_len())) {
        match token.kind() {
            SimpleTokenKind::Newline => {
                newlines += 1;
            }

            SimpleTokenKind::Whitespace => {}

            SimpleTokenKind::Comment => {
                max_new_lines = newlines.max(max_new_lines);
                newlines = 0;
            }

            _ => {
                max_new_lines = newlines.max(max_new_lines);
                break;
            }
        }
    }

    max_new_lines.saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use crate::comments::placement::max_empty_lines;

    #[test]
    fn count_empty_lines_in_trivia() {
        assert_eq!(max_empty_lines(""), 0);
        assert_eq!(max_empty_lines("# trailing comment\n # other comment\n"), 0);
        assert_eq!(
            max_empty_lines("# trailing comment\n# own line comment\n"),
            0
        );
        assert_eq!(
            max_empty_lines("# trailing comment\n\n# own line comment\n"),
            1
        );

        assert_eq!(
            max_empty_lines(
                "# trailing comment\n\n# own line comment\n\n# an other own line comment"
            ),
            1
        );

        assert_eq!(
            max_empty_lines(
                "# trailing comment\n\n# own line comment\n\n# an other own line comment\n# block"
            ),
            1
        );

        assert_eq!(
            max_empty_lines("# trailing comment\n\n# own line comment\n\n\n# an other own line comment\n# block"),
            2
        );

        assert_eq!(
            max_empty_lines(
                r#"# This multiline comments section
# should be split from the statement
# above by two lines.
"#
            ),
            0
        );
    }
}
