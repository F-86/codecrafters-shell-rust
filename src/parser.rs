//! 命令行词法分析器（tokenizer）+ 命令结构化解析器（parser）。
//!
//! 当前支持单引号、双引号、「引号外反斜杠转义」与 `>` / `1>` 重定向操作符语义：
//! - 单引号内任何字符（含空格、`$`、`*`、`~`、`"`、`\`、`>`、Tab 等）按字面量保留；
//! - 双引号内大部分字符按字面量保留（含空格、单引号、`*`、`;`、`>` 等）；`\` 仅对
//!   `"`、`\`、`$`、`` ` `` 这 4 个字符触发转义并吃掉自身，其他字符前 `\` 按字面量
//!   保留；`$` 仍按字面量（变量展开留待后续阶段）；
//! - 引号外 `\X` 移除 `X` 的特殊含义并按字面量保留 `X`，反斜杠本身被丢弃，
//!   适用于任意下一字符（含空白、`'`、`"`、`$`、`*`、`>` 等及普通字母）；行尾孤立 `\`
//!   视为语法错误（`TrailingBackslash`）；
//! - 引号外连续空白作为 token 分隔符并被折叠；
//! - 任意相邻的引号串 / 空引号 / 裸字符串 / 转义字符可无缝拼接成同一个 argument；
//! - 引号外 `>` 与 `1>` 被识别为独立 token：当且仅当 `>` 紧贴在裸字符 `1` 之后（中间无
//!   空白、无引号、无转义）时合并为单 token `"1>"`，其余情形 `>` 单独成 token；引号
//!   内 `>` 仍按字面量。
//!
//! 上层 `parse` 函数在 `tokenize` 输出基础上识别 `>` / `1>` 操作符（两者等价），
//! 把其后第一个 token 作为 stdout 重定向目标，剩余 token 作为 argv，组装出
//! [`ParsedCommand`]。

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
    /// `>` / `1>` 后没有目标文件 token（例如 `echo hello >`）。
    MissingRedirectTarget,
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
            ParseError::MissingRedirectTarget => {
                write!(f, "syntax error: missing redirect target")
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
                '>' => {
                    // 重定向操作符 `>`：作为独立 token 切出。
                    // 特例：若当前正在累积的 token 恰好是裸字符 `1`（隐含 `1` 与 `>`
                    // 之间无空白、无引号、无转义——任何此类间隔都会先 flush 出 `1`
                    // 或在 current 中混入其他字符，使 `current != "1"`），则把 token
                    // 升级为 `"1>"`，与 bash 的 `1>` 重定向语义对齐。其余情况下，
                    // 先 flush 当前 token（若有），再单独 push `">"` 作为操作符 token。
                    if in_token && current == "1" {
                        current.clear();
                        tokens.push("1>".to_string());
                    } else if in_token {
                        tokens.push(std::mem::take(&mut current));
                        tokens.push(">".to_string());
                    } else {
                        tokens.push(">".to_string());
                    }
                    in_token = false;
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

/// 结构化的一条命令：去除重定向元信息后的 argv，加上可选的 stdout 重定向目标。
///
/// 当前阶段仅支持 stdout 重定向（`>` / `1>`）；后续阶段可在此扩展 stderr / 追加 /
/// stdin 等字段而不需调整 [`tokenize`] 与上层 REPL 的契约。
#[derive(Debug, PartialEq, Eq)]
pub struct ParsedCommand {
    /// 命令名 + 参数（不含任何重定向操作符或目标文件 token）。
    /// 当输入是仅含空白或仅含重定向（如 `> out`）时，`argv` 可能为空——由上层 REPL 决定如何处理。
    pub argv: Vec<String>,
    /// stdout 重定向目标文件路径；`None` 表示未指定重定向。
    /// `>` 与 `1>` 完全等价，归一化在本函数内完成。
    pub stdout_redirect: Option<String>,
}

/// 把一行输入解析为 [`ParsedCommand`]。
///
/// 内部先调用 [`tokenize`] 得到扁平 token 序列，再单次线性扫描识别 `>` / `1>`：
/// 把紧随其后的 token 作为 `stdout_redirect`，其余按顺序追加进 `argv`。
///
/// 错误传播：tokenize 阶段的语法错误原样返回；若 `>` / `1>` 后无下一 token，
/// 返回 [`ParseError::MissingRedirectTarget`]。重复出现的重定向（如 `> a > b`）
/// 取**最后一次**为准，与 bash 行为一致（前面的目标文件依然被创建/截断，但本阶段
/// spec 不要求该副作用，故仅记录最后一次即可）。
pub fn parse(input: &str) -> Result<ParsedCommand, ParseError> {
    let tokens = tokenize(input)?;
    let mut argv: Vec<String> = Vec::with_capacity(tokens.len());
    let mut stdout_redirect: Option<String> = None;
    let mut iter = tokens.into_iter();
    while let Some(tok) = iter.next() {
        // 归一化：`>` 与 `1>` 都被识别为「stdout 重定向」操作符
        if tok == ">" || tok == "1>" {
            match iter.next() {
                Some(target) => stdout_redirect = Some(target),
                None => return Err(ParseError::MissingRedirectTarget),
            }
        } else {
            argv.push(tok);
        }
    }
    Ok(ParsedCommand {
        argv,
        stdout_redirect,
    })
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

    // ===== 引号包裹的可执行文件名（quoted executable names） =====
    // 目的：守护 tokens[0] 与 args 共享同一套引号/转义解析的契约，
    //       防止后续重构在「第一个 token」上引入特殊路径而破坏 spec 行为。

    #[test]
    fn quoted_executable_single_quoted_with_space() {
        // spec 入门样例：`'my program' argument1` → 可执行名 `my program`，参数 `argument1`
        assert_eq!(
            tokenize("'my program' argument1").unwrap(),
            vec!["my program", "argument1"]
        );
    }

    #[test]
    fn quoted_executable_double_quoted_with_space() {
        // spec 入门样例：`"exe with spaces" file.txt` → 可执行名 `exe with spaces`，参数 `file.txt`
        assert_eq!(
            tokenize(r#""exe with spaces" file.txt"#).unwrap(),
            vec!["exe with spaces", "file.txt"]
        );
    }

    #[test]
    fn quoted_executable_double_quoted_contains_single_quote() {
        // spec 测试样例：`"exe with 'single quotes'" file`
        // 双引号内单引号是字面量，可执行名最终为 `exe with 'single quotes'`
        assert_eq!(
            tokenize(r#""exe with 'single quotes'" file"#).unwrap(),
            vec!["exe with 'single quotes'", "file"]
        );
    }

    #[test]
    fn quoted_executable_single_quoted_contains_double_quote() {
        // spec 测试样例：`'exe with "quotes"' file`
        // 单引号内双引号是字面量，可执行名最终为 `exe with "quotes"`
        assert_eq!(
            tokenize(r#"'exe with "quotes"' file"#).unwrap(),
            vec![r#"exe with "quotes""#, "file"]
        );
    }

    // ===== 重定向操作符（`>` / `1>`）切分 =====
    // 目的：确保 `>` / `1>` 在引号外被切为独立 token；引号内仍按字面量；
    //       既有 32 个测试用例均不含 `>` 字符，故新增切分逻辑不引入回归。

    #[test]
    fn redirect_gt_is_standalone_token_when_spaced() {
        // 空格分隔形态：`echo hello > out` → `>` 独立 token
        assert_eq!(
            tokenize("echo hello > out").unwrap(),
            vec!["echo", "hello", ">", "out"]
        );
    }

    #[test]
    fn redirect_gt_splits_adjacent_token() {
        // 紧贴形态：`echo hello>out` → 仍切出 `>` 独立 token
        assert_eq!(
            tokenize("echo hello>out").unwrap(),
            vec!["echo", "hello", ">", "out"]
        );
    }

    #[test]
    fn redirect_1gt_merges_only_when_adjacent() {
        // 紧贴 `1>` 合并：`echo hi 1>out` → `1>` 单 token
        assert_eq!(
            tokenize("echo hi 1>out").unwrap(),
            vec!["echo", "hi", "1>", "out"]
        );
        // 空格分隔形态：`echo hi 1> out` → 同样合并（因为 `1` 与 `>` 之间无空白）
        assert_eq!(
            tokenize("echo hi 1> out").unwrap(),
            vec!["echo", "hi", "1>", "out"]
        );
        // 关键负样例：`1` 与 `>` 之间有空白 → 不合并，`1` 是普通 arg，`>` 是独立操作符
        assert_eq!(
            tokenize("echo 1 > out").unwrap(),
            vec!["echo", "1", ">", "out"]
        );
        // 关键负样例：`a1>out` 中 `1` 是字符串后缀而非孤立 token → 不合并
        assert_eq!(
            tokenize("echo a1>out").unwrap(),
            vec!["echo", "a1", ">", "out"]
        );
    }

    #[test]
    fn redirect_gt_inside_quotes_is_literal() {
        // 单引号内 `>` 是字面量
        assert_eq!(
            tokenize("echo '> not redirect'").unwrap(),
            vec!["echo", "> not redirect"]
        );
        // 双引号内 `>` 与 `1>` 都是字面量
        assert_eq!(
            tokenize(r#"echo "a > b" "1> c""#).unwrap(),
            vec!["echo", "a > b", "1> c"]
        );
        // 引号外反斜杠转义 `>` → 按字面量保留，不再被识别为操作符
        assert_eq!(
            tokenize(r"echo a\>b").unwrap(),
            vec!["echo", "a>b"]
        );
    }

    // ===== parse 层：ParsedCommand 结构化 =====

    #[test]
    fn parse_without_redirect_returns_argv_only() {
        let p = parse("echo hello world").unwrap();
        assert_eq!(p.argv, vec!["echo", "hello", "world"]);
        assert_eq!(p.stdout_redirect, None);
    }

    #[test]
    fn parse_gt_extracts_redirect_target() {
        // 空格分隔形态
        let p = parse("echo hello > out.txt").unwrap();
        assert_eq!(p.argv, vec!["echo", "hello"]);
        assert_eq!(p.stdout_redirect, Some("out.txt".to_string()));
        // 紧贴形态
        let p = parse("echo hello>out.txt").unwrap();
        assert_eq!(p.argv, vec!["echo", "hello"]);
        assert_eq!(p.stdout_redirect, Some("out.txt".to_string()));
    }

    #[test]
    fn parse_1gt_is_equivalent_to_gt() {
        // `1>` 与 `>` 归一化为同一语义：stdout 重定向
        let p = parse("echo Hello James 1> /tmp/foo/foo.md").unwrap();
        assert_eq!(p.argv, vec!["echo", "Hello", "James"]);
        assert_eq!(p.stdout_redirect, Some("/tmp/foo/foo.md".to_string()));
        // 紧贴 1> 形态同样生效
        let p = parse("echo a 1>out").unwrap();
        assert_eq!(p.argv, vec!["echo", "a"]);
        assert_eq!(p.stdout_redirect, Some("out".to_string()));
    }

    #[test]
    fn parse_missing_redirect_target_errors() {
        // `>` 后无 token → 报 MissingRedirectTarget
        assert_eq!(
            parse("echo hello >"),
            Err(ParseError::MissingRedirectTarget)
        );
        // `1>` 后无 token 同理
        assert_eq!(
            parse("echo hello 1>"),
            Err(ParseError::MissingRedirectTarget)
        );
    }

    #[test]
    fn parse_redirect_target_preserves_quoting() {
        // 目标文件名可被引号包裹（含空格）；parse 不应丢失内容
        let p = parse(r#"echo hi > "/tmp/with space.txt""#).unwrap();
        assert_eq!(p.argv, vec!["echo", "hi"]);
        assert_eq!(p.stdout_redirect, Some("/tmp/with space.txt".to_string()));
    }
}
