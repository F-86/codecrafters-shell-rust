//! 命令行词法分析器（tokenizer）。
//!
//! 当前支持单引号、双引号与「引号外反斜杠转义」语义：
//! - 单引号内任何字符（含空格、`$`、`*`、`~`、`"`、`\`、Tab 等）按字面量保留；
//! - 双引号内大部分字符按字面量保留（含空格、单引号、`*`、`;` 等）；`\` 仅对
//!   `"`、`\`、`$`、`` ` `` 这 4 个字符触发转义并吃掉自身，其他字符前 `\` 按字面量
//!   保留；`$` 仍按字面量（变量展开留待后续阶段）；
//! - 引号外 `\X` 移除 `X` 的特殊含义并按字面量保留 `X`，反斜杠本身被丢弃，
//!   适用于任意下一字符（含空白、`'`、`"`、`$`、`*` 等及普通字母）；行尾孤立 `\`
//!   视为语法错误（`TrailingBackslash`）；
//! - 引号外连续空白作为 token 分隔符并被折叠；
//! - 任意相邻的引号串 / 空引号 / 裸字符串 / 转义字符可无缝拼接成同一个 argument。

use std::fmt;

/// 词法分析阶段可能产生的错误。
///
/// 保留 enum 形式便于后续阶段（双引号内部分转义、变量展开等）继续扩展。
#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    /// 行末仍在单引号内部（缺少闭合 `'`）。
    UnterminatedSingleQuote,
    /// 行末仍在双引号内部（缺少闭合 `"`）。
    UnterminatedDoubleQuote,
    /// 行末为孤立反斜杠（无字符可转义）。
    TrailingBackslash,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::UnterminatedSingleQuote => {
                write!(f, "syntax error: unterminated single quote")
            }
            ParseError::UnterminatedDoubleQuote => {
                write!(f, "syntax error: unterminated double quote")
            }
            ParseError::TrailingBackslash => {
                write!(f, "syntax error: trailing backslash")
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// 词法分析器内部状态。
enum State {
    /// 引号外：空白作分隔符，遇到 `'` / `"` 进入对应引号态。
    Normal,
    /// 单引号内部：任何字符（除 `'`）都按字面量追加。
    InSingleQuote,
    /// 双引号内部：除 `"` 外大多数字符按字面量追加；`\` 仅对
    /// `"`、`\`、`$`、`` ` `` 这 4 个字符触发转义（吃掉自身），其他字符前
    /// `\` 按字面量保留。`$` 仍按字面量，待后续阶段实现变量展开。
    InDoubleQuote,
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
    // 使用显式迭代器以便 Normal 态遇到 `\` 时主动消费下一字符
    let mut chars = input.chars();

    while let Some(ch) = chars.next() {
        match state {
            State::Normal => match ch {
                '\\' => {
                    // 引号外反斜杠转义：消费下一字符并按字面量追加；
                    // 关键：必须置 in_token = true，使 `\<空格>` 不分隔 token、
                    // 且 `\X` 可独立开启首参数（如 `\_ignored_1`）。
                    match chars.next() {
                        Some(c) => {
                            current.push(c);
                            in_token = true;
                        }
                        // 行尾孤立 `\`：无字符可转义，按语法错误处理
                        None => return Err(ParseError::TrailingBackslash),
                    }
                }
                '\'' => {
                    // 开启单引号段；标记 token 已开始但不追加引号字符本身
                    state = State::InSingleQuote;
                    in_token = true;
                }
                '"' => {
                    // 开启双引号段；与单引号路径完全对称
                    state = State::InDoubleQuote;
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
                    // 引号内一切字符（含空白与特殊字符，包括 `\`）按字面量保留
                    current.push(c);
                }
            },
            State::InDoubleQuote => match ch {
                '"' => {
                    // 闭合引号：回到 Normal；in_token 保持为真支持后续拼接
                    state = State::Normal;
                }
                '\\' => {
                    // 双引号内反斜杠：仅对 `"`、`\`、`$`、`` ` `` 这 4 个字符触发转义
                    // 并吃掉自身；其他字符前 `\` 按字面量保留（与 Normal 态「无条件
                    // 转义」的关键差异：双引号内 `\n` 是 2 字符字面量，不丢反斜杠）。
                    // 注：`\$` 与 `` \` `` 提前到位与 bash 真实行为一致，避免后续
                    // 变量展开阶段回头改测试。`std::str::Chars` 实现 `Clone`，
                    // `chars.clone().next()` 是 O(1) 安全 peek。
                    match chars.clone().next() {
                        Some(next) if matches!(next, '"' | '\\' | '$' | '`') => {
                            chars.next(); // 消费下一字符
                            current.push(next); // 仅 push 下一字符（反斜杠被吃掉）
                        }
                        _ => {
                            // 其他字符或行尾：保留 `\` 字面，不消费下一字符
                            // （让其在循环正常分支处理）。行尾孤立 `\` + EOF
                            // 仍由后续 `UnterminatedDoubleQuote` 兜底。
                            current.push('\\');
                        }
                    }
                }
                c => {
                    // 引号内其他字符（含空白、单引号、`$`、`*`、`;` 等）按字面量保留
                    // 注：`$` 的变量展开语义在后续阶段实现
                    current.push(c);
                }
            },
        }
    }

    // 行尾仍处于引号内 → 视为语法错误，由 REPL 决定如何提示
    match state {
        State::InSingleQuote => return Err(ParseError::UnterminatedSingleQuote),
        State::InDoubleQuote => return Err(ParseError::UnterminatedDoubleQuote),
        State::Normal => {}
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

    // ===== 双引号语义 =====

    #[test]
    fn double_quote_preserves_spaces() {
        assert_eq!(
            tokenize(r#"echo "hello    world""#).unwrap(),
            vec!["echo", "hello    world"]
        );
    }

    #[test]
    fn double_quote_adjacent_concatenation() {
        // 双双
        assert_eq!(
            tokenize(r#"echo "hello""world""#).unwrap(),
            vec!["echo", "helloworld"]
        );
        // 双裸（双在前）
        assert_eq!(
            tokenize(r#"echo "hello"world"#).unwrap(),
            vec!["echo", "helloworld"]
        );
        // 裸双（双在后）
        assert_eq!(
            tokenize(r#"echo hello"world""#).unwrap(),
            vec!["echo", "helloworld"]
        );
    }

    #[test]
    fn double_and_single_quote_concatenation() {
        // 双 + 单
        assert_eq!(tokenize(r#""a"'b'"#).unwrap(), vec!["ab"]);
        // 单 + 双
        assert_eq!(tokenize(r#"'a'"b""#).unwrap(), vec!["ab"]);
        // 双 + 单 + 裸
        assert_eq!(tokenize(r#""a"'b'c"#).unwrap(), vec!["abc"]);
    }

    #[test]
    fn single_quote_inside_double_is_literal() {
        // spec 给出的关键示例：双引号内的单引号是字面量
        assert_eq!(
            tokenize(r#"echo "shell's test""#).unwrap(),
            vec!["echo", "shell's test"]
        );
    }

    #[test]
    fn double_quote_separate_args() {
        // 双引号外空白仍作分隔符
        assert_eq!(
            tokenize(r#"echo "quz  hello"  "bar""#).unwrap(),
            vec!["echo", "quz  hello", "bar"]
        );
        assert_eq!(
            tokenize(r#"echo "bar"  "shell's"  "foo""#).unwrap(),
            vec!["echo", "bar", "shell's", "foo"]
        );
    }

    #[test]
    fn double_quote_paths_for_cat() {
        // 含空格 + 内嵌单引号的路径，模拟测试中的 cat 用例
        assert_eq!(
            tokenize(r#"cat "/tmp/file name" "/tmp/'file name' with spaces""#).unwrap(),
            vec![
                "cat",
                "/tmp/file name",
                "/tmp/'file name' with spaces"
            ]
        );
    }

    #[test]
    fn unterminated_double_quote_errors() {
        assert_eq!(
            tokenize(r#"echo "abc"#),
            Err(ParseError::UnterminatedDoubleQuote)
        );
    }

    // ===== 引号外反斜杠转义 =====

    #[test]
    fn escaped_space_keeps_token() {
        // spec 关键示例：每个 `\ ` 都是字面空格，整体合成单 token
        assert_eq!(
            tokenize(r"echo three\ \ \ spaces").unwrap(),
            vec!["echo", "three   spaces"]
        );
        // 测试样例：4 个 `\ ` → 字面 4 空格
        assert_eq!(
            tokenize(r"echo multiple\ \ \ \ spaces").unwrap(),
            vec!["echo", "multiple    spaces"]
        );
    }

    #[test]
    fn escaped_then_unescaped_space_splits() {
        // `\ ` 保留首个字面空格延续 `before`；随后未转义连续空白折叠为单分隔
        // tokenize 结果应是两个 arg：`before ` 与 `after`
        assert_eq!(
            tokenize(r"echo before\     after").unwrap(),
            vec!["echo", "before ", "after"]
        );
    }

    #[test]
    fn escaped_letter_drops_backslash() {
        // `\n` 仅为字面 n，不做 C 风格转义
        assert_eq!(
            tokenize(r"echo test\nexample").unwrap(),
            vec!["echo", "testnexample"]
        );
        // `\_` 等普通字符同理
        assert_eq!(
            tokenize(r"echo ignore\_backslash").unwrap(),
            vec!["echo", "ignore_backslash"]
        );
    }

    #[test]
    fn escaped_backslash_yields_single_backslash() {
        // 第一个 `\` 转义第二个 `\`，结果一个字面反斜杠
        assert_eq!(
            tokenize(r"echo hello\\world").unwrap(),
            vec!["echo", r"hello\world"]
        );
    }

    #[test]
    fn escaped_quotes_are_literal() {
        // `\'` 与 `\"` 不进入引号态，按字面量
        assert_eq!(
            tokenize(r"echo \'hello\'").unwrap(),
            vec!["echo", "'hello'"]
        );
        assert_eq!(
            tokenize(r#"echo \'\"literal quotes\"\'"#).unwrap(),
            vec!["echo", r#"'"literal"#, r#"quotes"'"#]
        );
    }

    #[test]
    fn escaped_filenames_for_cat() {
        // 测试样例：3 个含转义的文件名参数
        assert_eq!(
            tokenize(r"cat /tmp/\_ignored_1 /tmp/ignore_\2 /tmp/just_one_\\_3").unwrap(),
            vec![
                "cat",
                "/tmp/_ignored_1",
                "/tmp/ignore_2",
                r"/tmp/just_one_\_3",
            ]
        );
    }

    #[test]
    fn backslash_inside_single_quote_is_literal() {
        // 守护既有行为：单引号内 `\` 仍按字面量（spec 范围之外）
        assert_eq!(tokenize(r"'a\b'").unwrap(), vec![r"a\b"]);
    }

    #[test]
    fn trailing_backslash_errors() {
        assert_eq!(tokenize(r"echo abc\"), Err(ParseError::TrailingBackslash));
    }

    // ===== 双引号内反斜杠转义 =====

    #[test]
    fn double_quote_escapes_backslash() {
        // spec 示例：`"A \\ escapes itself"` → `A \ escapes itself`
        // 双反斜杠 `\\` 在双引号内被吃掉一个，留下单字面反斜杠
        assert_eq!(
            tokenize(r#"echo "A \\ escapes itself""#).unwrap(),
            vec!["echo", r"A \ escapes itself"]
        );
    }

    #[test]
    fn double_quote_escapes_double_quote() {
        // spec 示例：`"A \" inside double quotes"` → `A " inside double quotes`
        // `\"` 在双引号内是字面双引号，不闭合
        assert_eq!(
            tokenize(r#"echo "A \" inside double quotes""#).unwrap(),
            vec!["echo", r#"A " inside double quotes"#]
        );
    }

    #[test]
    fn double_quote_preserves_backslash_before_letter() {
        // spec 关键示例：`"just'one'\\n'backslash"` → `just'one'\n'backslash`
        // `\\` → `\`，紧接的 `n` 是字面量；最终是反斜杠+n 两字符，不是换行符
        assert_eq!(
            tokenize(r#"echo "just'one'\\n'backslash""#).unwrap(),
            vec!["echo", r"just'one'\n'backslash"]
        );
    }

    #[test]
    fn double_quote_concatenation_with_escaped_quote() {
        // spec 关键示例：`"inside\"literal_quote."outside\"` → `inside"literal_quote.outside"`
        // 三段拼接：双引号段 `inside"literal_quote.` + Normal 续接 `outside` +
        // 引号外 `\"` 转义为字面 `"`，全程单 token
        assert_eq!(
            tokenize(r#""inside\"literal_quote."outside\""#).unwrap(),
            vec![r#"inside"literal_quote.outside""#]
        );
    }

    #[test]
    fn double_quote_paths_for_cat_with_escapes() {
        // spec 测试样例：cat 三个含转义的双引号路径
        assert_eq!(
            tokenize(r#"cat "/tmp/number 1" "/tmp/doublequote \" 2" "/tmp/backslash \\ 3""#)
                .unwrap(),
            vec![
                "cat",
                "/tmp/number 1",
                r#"/tmp/doublequote " 2"#,
                r"/tmp/backslash \ 3",
            ]
        );
    }

    #[test]
    fn double_quote_escapes_dollar() {
        // 提前到位：`\$` 在双引号内吃掉反斜杠，仅留字面 `$`（与 bash 真实行为一致）
        assert_eq!(
            tokenize(r#"echo "price \$5""#).unwrap(),
            vec!["echo", "price $5"]
        );
    }

    #[test]
    fn double_quote_escapes_backtick() {
        // 提前到位：`` \` `` 在双引号内吃掉反斜杠，仅留字面反引号
        assert_eq!(
            tokenize(r#"echo "a \` b""#).unwrap(),
            vec!["echo", "a ` b"]
        );
    }

    #[test]
    fn double_quote_backslash_before_other_chars_is_literal() {
        // 反斜杠后跟普通字符（n、a、空格等）时反斜杠按字面量保留
        assert_eq!(
            tokenize(r#"echo "\a\b\c""#).unwrap(),
            vec!["echo", r"\a\b\c"]
        );
        // 反斜杠后跟空格也保留（双引号内空格本就是字面量，反斜杠也保留）
        assert_eq!(
            tokenize(r#"echo "x\ y""#).unwrap(),
            vec!["echo", r"x\ y"]
        );
    }

    #[test]
    fn backslash_inside_single_quote_unchanged() {
        // 回归守护：单引号内反斜杠仍按字面量（spec 范围之外，不应被本阶段改动影响）
        assert_eq!(tokenize(r"echo 'a\b\\c'").unwrap(), vec!["echo", r"a\b\\c"]);
    }
}
