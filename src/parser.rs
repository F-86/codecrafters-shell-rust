//! 命令行词法分析器（tokenizer）。
//!
//! 本阶段仅支持单引号语义：
//! - 单引号内任何字符（含空格、`$`、`*`、`~`、Tab 等）按字面量保留；
//! - 引号外连续空白作为 token 分隔符并被折叠；
//! - 相邻引号串 / 空引号 `''` / 裸字符串可无缝拼接成同一个 argument。
//!
//! 后续阶段可在本模块叠加双引号、反斜杠转义、变量展开等规则。

use std::fmt;

/// 词法分析阶段可能产生的错误。
///
/// 本阶段仅一种变体；保留 enum 形式便于后续阶段（双引号、反斜杠等）扩展。
#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    /// 行末仍在单引号内部（缺少闭合引号）。
    UnterminatedSingleQuote,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::UnterminatedSingleQuote => {
                write!(f, "syntax error: unterminated single quote")
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// 词法分析器内部状态。
enum State {
    /// 引号外：空白作分隔符，遇到 `'` 进入引号态。
    Normal,
    /// 单引号内部：任何字符（除 `'`）都按字面量追加。
    InSingleQuote,
}

/// 将一行命令切分为 token 序列。
///
/// 返回的 `Vec<String>` 中每个元素对应最终传给命令的一个 argv；
/// 相邻引号 / 空引号 / 裸字符串拼接已在内部完成。
pub fn tokenize(input: &str) -> Result<Vec<String>, ParseError> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    // 标记「当前 token 是否已经开始」：
    // 用它而不是「碰到 `'` 就 push 空串」可天然支持
    // `''`、`hello''world`、`'a''b'` 这类相邻拼接，无需特殊分支。
    let mut in_token = false;
    let mut state = State::Normal;

    for ch in input.chars() {
        match state {
            State::Normal => match ch {
                '\'' => {
                    // 开启引号段；标记 token 已开始但不追加引号字符本身
                    state = State::InSingleQuote;
                    in_token = true;
                }
                c if c.is_whitespace() => {
                    // 引号外空白：若已有 token 则结束之；否则跳过连续空白
                    if in_token {
                        tokens.push(std::mem::take(&mut current));
                        in_token = false;
                    }
                }
                c => {
                    current.push(c);
                    in_token = true;
                }
            },
            State::InSingleQuote => match ch {
                '\'' => {
                    // 闭合引号：回到 Normal；保持 in_token 为真以便后续字符 / 引号继续拼接
                    state = State::Normal;
                }
                c => {
                    // 引号内一切字符（含空白与特殊字符）按字面量保留
                    current.push(c);
                }
            },
        }
    }

    // 行尾仍处于引号内 → 视为语法错误，由 REPL 决定如何提示
    if matches!(state, State::InSingleQuote) {
        return Err(ParseError::UnterminatedSingleQuote);
    }

    // flush 最后一个 token
    if in_token {
        tokens.push(current);
    }

    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_empty_vec() {
        assert!(tokenize("").unwrap().is_empty());
        assert!(tokenize("   \t  ").unwrap().is_empty());
    }

    #[test]
    fn plain_whitespace_split() {
        assert_eq!(tokenize("echo a b").unwrap(), vec!["echo", "a", "b"]);
        // 连续空白折叠
        assert_eq!(tokenize("echo   a    b").unwrap(), vec!["echo", "a", "b"]);
    }

    #[test]
    fn single_quote_preserves_spaces() {
        assert_eq!(
            tokenize("echo 'hello    world'").unwrap(),
            vec!["echo", "hello    world"]
        );
    }

    #[test]
    fn adjacent_quoted_concatenation() {
        assert_eq!(tokenize("'hello''world'").unwrap(), vec!["helloworld"]);
        assert_eq!(
            tokenize("echo 'hello''world'").unwrap(),
            vec!["echo", "helloworld"]
        );
    }

    #[test]
    fn empty_quotes_concatenate_with_bare() {
        // hello''world → helloworld
        assert_eq!(tokenize("hello''world").unwrap(), vec!["helloworld"]);
        // ''abc → abc（空引号在前）
        assert_eq!(tokenize("''abc").unwrap(), vec!["abc"]);
        // 单独 '' → 一个空字符串 token
        assert_eq!(tokenize("''").unwrap(), vec![""]);
    }

    #[test]
    fn special_chars_inside_quotes_are_literal() {
        assert_eq!(
            tokenize("echo '$HOME *.rs ~user'").unwrap(),
            vec!["echo", "$HOME *.rs ~user"]
        );
    }

    #[test]
    fn multiple_quoted_paths() {
        assert_eq!(
            tokenize("cat '/tmp/file name' '/tmp/file name with spaces'").unwrap(),
            vec![
                "cat",
                "/tmp/file name",
                "/tmp/file name with spaces"
            ]
        );
    }

    #[test]
    fn unterminated_single_quote_errors() {
        assert_eq!(
            tokenize("echo 'abc"),
            Err(ParseError::UnterminatedSingleQuote)
        );
    }
}
