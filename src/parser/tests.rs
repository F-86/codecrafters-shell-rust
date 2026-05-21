//! parser 模块单元测试集合。
//!
//! 本文件被 [`super`]（即 `parser/mod.rs`）以 `#[cfg(test)] mod tests;` 声明，
//! 仅在 `cargo test` 时编译；通过 `use super::*` 访问 `tokenize` / `parse` /
//! `ParsedCommand` / `ParseError` 等 parser 公开 API。

use super::*;
use std::collections::HashMap;

/// 构造空的 shell 变量视图，供绝大多数「与 `$VAR` 展开无关」的测试用例使用。
///
/// 设计取舍：tokenize / parse 系列签名引入 `vars` 参数后，137+ 处既有调用全部
/// 需要传第二参；用 helper 收口生成空 `HashMap` 比每处 inline `&HashMap::new()`
/// 更清晰，且未来若需统一改造（例如默认 PATH/HOME 注入）只需改这一处。
///
/// 「空表语义」：所有 `$NAME` 展开命中 `vars.get(NAME) == None`，按 q2 决策展开
/// 为空串。即 `tokenize("$X")` 返回 `[""]`、`tokenize("a$X b")` 返回 `["a", "b"]`
/// （首段 `a` 与展开空串拼接为 `"a"`），与「未 declare 任何变量」的 REPL 行为一致。
fn empty_vars() -> HashMap<String, String> {
    HashMap::new()
}

/// 构造含若干预设变量的 shell 变量视图，供 `$VAR` 展开测试组使用。
fn vars_with(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn empty_input_yields_empty_vec() {
    assert!(tokenize("", &empty_vars()).unwrap().is_empty());
    assert!(tokenize("   \t  ", &empty_vars()).unwrap().is_empty());
}

#[test]
fn plain_whitespace_split() {
    assert_eq!(tokenize("echo a b", &empty_vars()).unwrap(), vec!["echo", "a", "b"]);
    // 连续空白折叠
    assert_eq!(tokenize("echo   a    b", &empty_vars()).unwrap(), vec!["echo", "a", "b"]);
}

#[test]
fn single_quote_preserves_spaces() {
    assert_eq!(
        tokenize("echo 'hello    world'", &empty_vars()).unwrap(),
        vec!["echo", "hello    world"]
    );
}

#[test]
fn adjacent_quoted_concatenation() {
    assert_eq!(tokenize("'hello''world'", &empty_vars()).unwrap(), vec!["helloworld"]);
    assert_eq!(
        tokenize("echo 'hello''world'", &empty_vars()).unwrap(),
        vec!["echo", "helloworld"]
    );
}

#[test]
fn empty_quotes_concatenate_with_bare() {
    // hello''world → helloworld
    assert_eq!(tokenize("hello''world", &empty_vars()).unwrap(), vec!["helloworld"]);
    // ''abc → abc（空引号在前）
    assert_eq!(tokenize("''abc", &empty_vars()).unwrap(), vec!["abc"]);
    // 单独 '' → 一个空字符串 token
    assert_eq!(tokenize("''", &empty_vars()).unwrap(), vec![""]);
}

#[test]
fn special_chars_inside_quotes_are_literal() {
    assert_eq!(
        tokenize("echo '$HOME *.rs ~user'", &empty_vars()).unwrap(),
        vec!["echo", "$HOME *.rs ~user"]
    );
}

#[test]
fn multiple_quoted_paths() {
    assert_eq!(
        tokenize("cat '/tmp/file name' '/tmp/file name with spaces'", &empty_vars()).unwrap(),
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
        tokenize("echo 'abc", &empty_vars()),
        Err(ParseError::UnterminatedSingleQuote)
    );
}

// ===== 双引号语义 =====

#[test]
fn double_quote_preserves_spaces() {
    assert_eq!(
        tokenize(r#"echo "hello    world""#, &empty_vars()).unwrap(),
        vec!["echo", "hello    world"]
    );
}

#[test]
fn double_quote_adjacent_concatenation() {
    // 双双
    assert_eq!(
        tokenize(r#"echo "hello""world""#, &empty_vars()).unwrap(),
        vec!["echo", "helloworld"]
    );
    // 双裸（双在前）
    assert_eq!(
        tokenize(r#"echo "hello"world"#, &empty_vars()).unwrap(),
        vec!["echo", "helloworld"]
    );
    // 裸双（双在后）
    assert_eq!(
        tokenize(r#"echo hello"world""#, &empty_vars()).unwrap(),
        vec!["echo", "helloworld"]
    );
}

#[test]
fn double_and_single_quote_concatenation() {
    // 双 + 单
    assert_eq!(tokenize(r#""a"'b'"#, &empty_vars()).unwrap(), vec!["ab"]);
    // 单 + 双
    assert_eq!(tokenize(r#"'a'"b""#, &empty_vars()).unwrap(), vec!["ab"]);
    // 双 + 单 + 裸
    assert_eq!(tokenize(r#""a"'b'c"#, &empty_vars()).unwrap(), vec!["abc"]);
}

#[test]
fn single_quote_inside_double_is_literal() {
    // spec 给出的关键示例：双引号内的单引号是字面量
    assert_eq!(
        tokenize(r#"echo "shell's test""#, &empty_vars()).unwrap(),
        vec!["echo", "shell's test"]
    );
}

#[test]
fn double_quote_separate_args() {
    // 双引号外空白仍作分隔符
    assert_eq!(
        tokenize(r#"echo "quz  hello"  "bar""#, &empty_vars()).unwrap(),
        vec!["echo", "quz  hello", "bar"]
    );
    assert_eq!(
        tokenize(r#"echo "bar"  "shell's"  "foo""#, &empty_vars()).unwrap(),
        vec!["echo", "bar", "shell's", "foo"]
    );
}

#[test]
fn double_quote_paths_for_cat() {
    // 含空格 + 内嵌单引号的路径，模拟测试中的 cat 用例
    assert_eq!(
        tokenize(r#"cat "/tmp/file name" "/tmp/'file name' with spaces""#, &empty_vars()).unwrap(),
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
        tokenize(r#"echo "abc"#, &empty_vars()),
        Err(ParseError::UnterminatedDoubleQuote)
    );
}

// ===== 引号外反斜杠转义 =====

#[test]
fn escaped_space_keeps_token() {
    // spec 关键示例：每个 `\ ` 都是字面空格，整体合成单 token
    assert_eq!(
        tokenize(r"echo three\ \ \ spaces", &empty_vars()).unwrap(),
        vec!["echo", "three   spaces"]
    );
    // 测试样例：4 个 `\ ` → 字面 4 空格
    assert_eq!(
        tokenize(r"echo multiple\ \ \ \ spaces", &empty_vars()).unwrap(),
        vec!["echo", "multiple    spaces"]
    );
}

#[test]
fn escaped_then_unescaped_space_splits() {
    // `\ ` 保留首个字面空格延续 `before`；随后未转义连续空白折叠为单分隔
    // tokenize 结果应是两个 arg：`before ` 与 `after`
    assert_eq!(
        tokenize(r"echo before\     after", &empty_vars()).unwrap(),
        vec!["echo", "before ", "after"]
    );
}

#[test]
fn escaped_letter_drops_backslash() {
    // `\n` 仅为字面 n，不做 C 风格转义
    assert_eq!(
        tokenize(r"echo test\nexample", &empty_vars()).unwrap(),
        vec!["echo", "testnexample"]
    );
    // `\_` 等普通字符同理
    assert_eq!(
        tokenize(r"echo ignore\_backslash", &empty_vars()).unwrap(),
        vec!["echo", "ignore_backslash"]
    );
}

#[test]
fn escaped_backslash_yields_single_backslash() {
    // 第一个 `\` 转义第二个 `\`，结果一个字面反斜杠
    assert_eq!(
        tokenize(r"echo hello\\world", &empty_vars()).unwrap(),
        vec!["echo", r"hello\world"]
    );
}

#[test]
fn escaped_quotes_are_literal() {
    // `\'` 与 `\"` 不进入引号态，按字面量
    assert_eq!(
        tokenize(r"echo \'hello\'", &empty_vars()).unwrap(),
        vec!["echo", "'hello'"]
    );
    assert_eq!(
        tokenize(r#"echo \'\"literal quotes\"\'"#, &empty_vars()).unwrap(),
        vec!["echo", r#"'"literal"#, r#"quotes"'"#]
    );
}

#[test]
fn escaped_filenames_for_cat() {
    // 测试样例：3 个含转义的文件名参数
    assert_eq!(
        tokenize(r"cat /tmp/\_ignored_1 /tmp/ignore_\2 /tmp/just_one_\\_3", &empty_vars()).unwrap(),
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
    assert_eq!(tokenize(r"'a\b'", &empty_vars()).unwrap(), vec![r"a\b"]);
}

#[test]
fn trailing_backslash_errors() {
    assert_eq!(tokenize(r"echo abc\", &empty_vars()), Err(ParseError::TrailingBackslash));
}

// ===== 双引号内反斜杠转义 =====

#[test]
fn double_quote_escapes_backslash() {
    // spec 示例：`"A \\ escapes itself"` → `A \ escapes itself`
    // 双反斜杠 `\\` 在双引号内被吃掉一个，留下单字面反斜杠
    assert_eq!(
        tokenize(r#"echo "A \\ escapes itself""#, &empty_vars()).unwrap(),
        vec!["echo", r"A \ escapes itself"]
    );
}

#[test]
fn double_quote_escapes_double_quote() {
    // spec 示例：`"A \" inside double quotes"` → `A " inside double quotes`
    // `\"` 在双引号内是字面双引号，不闭合
    assert_eq!(
        tokenize(r#"echo "A \" inside double quotes""#, &empty_vars()).unwrap(),
        vec!["echo", r#"A " inside double quotes"#]
    );
}

#[test]
fn double_quote_preserves_backslash_before_letter() {
    // spec 关键示例：`"just'one'\\n'backslash"` → `just'one'\n'backslash`
    // `\\` → `\`，紧接的 `n` 是字面量；最终是反斜杠+n 两字符，不是换行符
    assert_eq!(
        tokenize(r#"echo "just'one'\\n'backslash""#, &empty_vars()).unwrap(),
        vec!["echo", r"just'one'\n'backslash"]
    );
}

#[test]
fn double_quote_concatenation_with_escaped_quote() {
    // spec 关键示例：`"inside\"literal_quote."outside\"` → `inside"literal_quote.outside"`
    // 三段拼接：双引号段 `inside"literal_quote.` + Normal 续接 `outside` +
    // 引号外 `\"` 转义为字面 `"`，全程单 token
    assert_eq!(
        tokenize(r#""inside\"literal_quote."outside\""#, &empty_vars()).unwrap(),
        vec![r#"inside"literal_quote.outside""#]
    );
}

#[test]
fn double_quote_paths_for_cat_with_escapes() {
    // spec 测试样例：cat 三个含转义的双引号路径
    assert_eq!(
        tokenize(r#"cat "/tmp/number 1" "/tmp/doublequote \" 2" "/tmp/backslash \\ 3""#, &empty_vars())
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
        tokenize(r#"echo "price \$5""#, &empty_vars()).unwrap(),
        vec!["echo", "price $5"]
    );
}

#[test]
fn double_quote_escapes_backtick() {
    // 提前到位：`` \` `` 在双引号内吃掉反斜杠，仅留字面反引号
    assert_eq!(
        tokenize(r#"echo "a \` b""#, &empty_vars()).unwrap(),
        vec!["echo", "a ` b"]
    );
}

#[test]
fn double_quote_backslash_before_other_chars_is_literal() {
    // 反斜杠后跟普通字符（n、a、空格等）时反斜杠按字面量保留
    assert_eq!(
        tokenize(r#"echo "\a\b\c""#, &empty_vars()).unwrap(),
        vec!["echo", r"\a\b\c"]
    );
    // 反斜杠后跟空格也保留（双引号内空格本就是字面量，反斜杠也保留）
    assert_eq!(
        tokenize(r#"echo "x\ y""#, &empty_vars()).unwrap(),
        vec!["echo", r"x\ y"]
    );
}

#[test]
fn backslash_inside_single_quote_unchanged() {
    // 回归守护：单引号内反斜杠仍按字面量（spec 范围之外，不应被本阶段改动影响）
    assert_eq!(tokenize(r"echo 'a\b\\c'", &empty_vars()).unwrap(), vec!["echo", r"a\b\\c"]);
}

// ===== 引号包裹的可执行文件名（quoted executable names） =====
// 目的：守护 tokens[0] 与 args 共享同一套引号/转义解析的契约，
//       防止后续重构在「第一个 token」上引入特殊路径而破坏 spec 行为。

#[test]
fn quoted_executable_single_quoted_with_space() {
    // spec 入门样例：`'my program' argument1` → 可执行名 `my program`，参数 `argument1`
    assert_eq!(
        tokenize("'my program' argument1", &empty_vars()).unwrap(),
        vec!["my program", "argument1"]
    );
}

#[test]
fn quoted_executable_double_quoted_with_space() {
    // spec 入门样例：`"exe with spaces" file.txt` → 可执行名 `exe with spaces`，参数 `file.txt`
    assert_eq!(
        tokenize(r#""exe with spaces" file.txt"#, &empty_vars()).unwrap(),
        vec!["exe with spaces", "file.txt"]
    );
}

#[test]
fn quoted_executable_double_quoted_contains_single_quote() {
    // spec 测试样例：`"exe with 'single quotes'" file`
    // 双引号内单引号是字面量，可执行名最终为 `exe with 'single quotes'`
    assert_eq!(
        tokenize(r#""exe with 'single quotes'" file"#, &empty_vars()).unwrap(),
        vec!["exe with 'single quotes'", "file"]
    );
}

#[test]
fn quoted_executable_single_quoted_contains_double_quote() {
    // spec 测试样例：`'exe with "quotes"' file`
    // 单引号内双引号是字面量，可执行名最终为 `exe with "quotes"`
    assert_eq!(
        tokenize(r#"'exe with "quotes"' file"#, &empty_vars()).unwrap(),
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
        tokenize("echo hello > out", &empty_vars()).unwrap(),
        vec!["echo", "hello", ">", "out"]
    );
}

#[test]
fn redirect_gt_splits_adjacent_token() {
    // 紧贴形态：`echo hello>out` → 仍切出 `>` 独立 token
    assert_eq!(
        tokenize("echo hello>out", &empty_vars()).unwrap(),
        vec!["echo", "hello", ">", "out"]
    );
}

#[test]
fn redirect_1gt_merges_only_when_adjacent() {
    // 紧贴 `1>` 合并：`echo hi 1>out` → `1>` 单 token
    assert_eq!(
        tokenize("echo hi 1>out", &empty_vars()).unwrap(),
        vec!["echo", "hi", "1>", "out"]
    );
    // 空格分隔形态：`echo hi 1> out` → 同样合并（因为 `1` 与 `>` 之间无空白）
    assert_eq!(
        tokenize("echo hi 1> out", &empty_vars()).unwrap(),
        vec!["echo", "hi", "1>", "out"]
    );
    // 关键负样例：`1` 与 `>` 之间有空白 → 不合并，`1` 是普通 arg，`>` 是独立操作符
    assert_eq!(
        tokenize("echo 1 > out", &empty_vars()).unwrap(),
        vec!["echo", "1", ">", "out"]
    );
    // 关键负样例：`a1>out` 中 `1` 是字符串后缀而非孤立 token → 不合并
    assert_eq!(
        tokenize("echo a1>out", &empty_vars()).unwrap(),
        vec!["echo", "a1", ">", "out"]
    );
}

#[test]
fn redirect_gt_inside_quotes_is_literal() {
    // 单引号内 `>` 是字面量
    assert_eq!(
        tokenize("echo '> not redirect'", &empty_vars()).unwrap(),
        vec!["echo", "> not redirect"]
    );
    // 双引号内 `>` 与 `1>` 都是字面量
    assert_eq!(
        tokenize(r#"echo "a > b" "1> c""#, &empty_vars()).unwrap(),
        vec!["echo", "a > b", "1> c"]
    );
    // 引号外反斜杠转义 `>` → 按字面量保留，不再被识别为操作符
    assert_eq!(
        tokenize(r"echo a\>b", &empty_vars()).unwrap(),
        vec!["echo", "a>b"]
    );
}

// ===== parse 层：ParsedCommand 结构化 =====

#[test]
fn parse_without_redirect_returns_argv_only() {
    let p = parse("echo hello world", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["echo", "hello", "world"]);
    assert_eq!(p.stdout_redirect, None);
}

#[test]
fn parse_gt_extracts_redirect_target() {
    // 空格分隔形态
    let p = parse("echo hello > out.txt", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["echo", "hello"]);
    assert_eq!(p.stdout_redirect, Some("out.txt".to_string()));
    // 紧贴形态
    let p = parse("echo hello>out.txt", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["echo", "hello"]);
    assert_eq!(p.stdout_redirect, Some("out.txt".to_string()));
}

#[test]
fn parse_1gt_is_equivalent_to_gt() {
    // `1>` 与 `>` 归一化为同一语义：stdout 重定向
    let p = parse("echo Hello James 1> /tmp/foo/foo.md", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["echo", "Hello", "James"]);
    assert_eq!(p.stdout_redirect, Some("/tmp/foo/foo.md".to_string()));
    // 紧贴 1> 形态同样生效
    let p = parse("echo a 1>out", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["echo", "a"]);
    assert_eq!(p.stdout_redirect, Some("out".to_string()));
}

#[test]
fn parse_missing_redirect_target_errors() {
    // `>` 后无 token → 报 MissingRedirectTarget
    assert_eq!(
        parse("echo hello >", &empty_vars()),
        Err(ParseError::MissingRedirectTarget)
    );
    // `1>` 后无 token 同理
    assert_eq!(
        parse("echo hello 1>", &empty_vars()),
        Err(ParseError::MissingRedirectTarget)
    );
}

#[test]
fn parse_redirect_target_preserves_quoting() {
    // 目标文件名可被引号包裹（含空格）；parse 不应丢失内容
    let p = parse(r#"echo hi > "/tmp/with space.txt""#, &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["echo", "hi"]);
    assert_eq!(p.stdout_redirect, Some("/tmp/with space.txt".to_string()));
}

// ===== `2>` 重定向：tokenize 层 =====
// 目的：确认 `2>` 在引号外被切为独立 token；引号内 / 转义后仍按字面量。
// 既有用例不含 `2>` 字符序列，零回归风险。

#[test]
fn redirect_2gt_merges_only_when_adjacent() {
    // 紧贴 `2>` 合并：`ls nodir 2>err` → `2>` 单 token
    assert_eq!(
        tokenize("ls nodir 2>err", &empty_vars()).unwrap(),
        vec!["ls", "nodir", "2>", "err"]
    );
    // 空格分隔形态：`ls nodir 2> err` → 同样合并（`2` 与 `>` 之间无空白）
    assert_eq!(
        tokenize("ls nodir 2> err", &empty_vars()).unwrap(),
        vec!["ls", "nodir", "2>", "err"]
    );
    // 关键负样例：`2` 与 `>` 之间有空白 → 不合并，`2` 是普通 arg，`>` 是独立操作符
    assert_eq!(
        tokenize("echo hi 2 > out", &empty_vars()).unwrap(),
        vec!["echo", "hi", "2", ">", "out"]
    );
    // 关键负样例：`a2>out` 中 `2` 是字符串后缀而非孤立 token → 不合并
    assert_eq!(
        tokenize("echo a2>out", &empty_vars()).unwrap(),
        vec!["echo", "a2", ">", "out"]
    );
}

#[test]
fn redirect_2gt_inside_quotes_is_literal() {
    // 单引号内 `2>` 是字面量
    assert_eq!(
        tokenize("echo '2> not redirect'", &empty_vars()).unwrap(),
        vec!["echo", "2> not redirect"]
    );
    // 双引号内 `2>` 是字面量
    assert_eq!(
        tokenize(r#"echo "x 2> y""#, &empty_vars()).unwrap(),
        vec!["echo", "x 2> y"]
    );
}

#[test]
fn redirect_2gt_escaped_is_literal() {
    // 引号外反斜杠转义 `>` 紧跟在 `2` 之后 → `2` 是裸字符 token，但 `>` 被转义，
    // 故不再触发 `2>` 合并，而是 `2` 与 `>` 拼接为字面 `2>` 字符串 token
    assert_eq!(
        tokenize(r"echo 2\>out", &empty_vars()).unwrap(),
        vec!["echo", "2>out"]
    );
}

// ===== `2>` 重定向：parse 层 =====

#[test]
fn parse_2gt_extracts_stderr_target() {
    // 空格分隔形态
    let p = parse("ls nonexistent 2> /tmp/quz/baz.md", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["ls", "nonexistent"]);
    assert_eq!(p.stdout_redirect, None);
    assert_eq!(p.stderr_redirect, Some("/tmp/quz/baz.md".to_string()));
    // 紧贴形态
    let p = parse("ls nonexistent 2>err", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["ls", "nonexistent"]);
    assert_eq!(p.stderr_redirect, Some("err".to_string()));
}

#[test]
fn parse_stdout_and_stderr_coexist() {
    // `> out 2> err`：两种重定向同时出现，互不干扰
    let p = parse("cmd a b > out.txt 2> err.txt", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["cmd", "a", "b"]);
    assert_eq!(p.stdout_redirect, Some("out.txt".to_string()));
    assert_eq!(p.stderr_redirect, Some("err.txt".to_string()));
    // 顺序反转同样生效：`2> err > out`
    let p = parse("cmd a b 2> err.txt > out.txt", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["cmd", "a", "b"]);
    assert_eq!(p.stdout_redirect, Some("out.txt".to_string()));
    assert_eq!(p.stderr_redirect, Some("err.txt".to_string()));
}

#[test]
fn parse_2gt_missing_target_errors() {
    // `2>` 后无 token → 报 MissingRedirectTarget
    assert_eq!(
        parse("ls nonexistent 2>", &empty_vars()),
        Err(ParseError::MissingRedirectTarget)
    );
}

#[test]
fn parse_2gt_target_preserves_quoting() {
    // stderr 目标文件名可被引号包裹（含空格）；parse 不应丢失内容
    let p = parse(r#"ls nodir 2> "/tmp/err log.md""#, &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["ls", "nodir"]);
    assert_eq!(p.stderr_redirect, Some("/tmp/err log.md".to_string()));
}

// ===== `>>` / `1>>` / `2>>` 追加重定向：tokenize 层 =====
// 目的：确认 `>>` 仅在两个 `>` 紧贴（无空白/引号/转义间隔）时合并；
//       与 `1` / `2` 紧贴时再分别升级为 `1>>` / `2>>`；引号内 / 转义后仍按字面量。

#[test]
fn redirect_append_plain_merges_only_adjacent_gt() {
    // 紧贴两个 `>` → 合并为 `>>` 单 token
    assert_eq!(
        tokenize("echo hi >> out", &empty_vars()).unwrap(),
        vec!["echo", "hi", ">>", "out"]
    );
    // 紧贴目标（无空格）：`>>out` 仍正确切出 `>>` 与目标
    assert_eq!(
        tokenize("echo hi >>out", &empty_vars()).unwrap(),
        vec!["echo", "hi", ">>", "out"]
    );
    // 关键负样例：两个 `>` 之间有空格 → 不合并，应是两个独立 `>` 操作符
    assert_eq!(
        tokenize("echo hi > > out", &empty_vars()).unwrap(),
        vec!["echo", "hi", ">", ">", "out"]
    );
}

#[test]
fn redirect_append_with_digit_prefix_merges() {
    // `1>>` 合并：紧贴 `1` 与连续 `>>`
    assert_eq!(
        tokenize("echo hi 1>> out", &empty_vars()).unwrap(),
        vec!["echo", "hi", "1>>", "out"]
    );
    assert_eq!(
        tokenize("echo hi 1>>out", &empty_vars()).unwrap(),
        vec!["echo", "hi", "1>>", "out"]
    );
    // `2>>` 合并
    assert_eq!(
        tokenize("ls nodir 2>> err", &empty_vars()).unwrap(),
        vec!["ls", "nodir", "2>>", "err"]
    );
    assert_eq!(
        tokenize("ls nodir 2>>err", &empty_vars()).unwrap(),
        vec!["ls", "nodir", "2>>", "err"]
    );
    // 关键负样例：`1` 与 `>>` 之间有空格 → `1` 是普通 arg，`>>` 独立成操作符
    assert_eq!(
        tokenize("echo hi 1 >> out", &empty_vars()).unwrap(),
        vec!["echo", "hi", "1", ">>", "out"]
    );
}

#[test]
fn redirect_append_inside_quotes_is_literal() {
    // 单引号内 `>>` 是字面量
    assert_eq!(
        tokenize("echo '>> not redirect'", &empty_vars()).unwrap(),
        vec!["echo", ">> not redirect"]
    );
    // 双引号内 `1>>` 是字面量
    assert_eq!(
        tokenize(r#"echo "x 1>> y""#, &empty_vars()).unwrap(),
        vec!["echo", "x 1>> y"]
    );
}

#[test]
fn redirect_append_escaped_breaks_merge() {
    // 第一个 `>` 后紧跟 `\>`：转义切断合并——首 `>` 单独成 token，
    // 然后 `\>` 在 Normal 态被转义为字面量 `>`，与下一字符拼接为字符串 token。
    // 当前累积 token 为空，故首 `>` 作为独立操作符 token push。
    assert_eq!(
        tokenize(r"echo hi >\>out", &empty_vars()).unwrap(),
        vec!["echo", "hi", ">", ">out"]
    );
}

// ===== `>>` / `1>>` / `2>>` 追加重定向：parse 层 =====

#[test]
fn parse_append_sets_stdout_append_flag() {
    // `>>` 与 `1>>` 都填 stdout_redirect 并置 stdout_append=true
    let p = parse("echo first >> out.txt", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["echo", "first"]);
    assert_eq!(p.stdout_redirect, Some("out.txt".to_string()));
    assert!(p.stdout_append);
    assert_eq!(p.stderr_redirect, None);
    assert!(!p.stderr_append);

    let p = parse("echo first 1>> out.txt", &empty_vars()).unwrap();
    assert_eq!(p.stdout_redirect, Some("out.txt".to_string()));
    assert!(p.stdout_append);
}

#[test]
fn parse_append_stderr_sets_flag() {
    // `2>>` 填 stderr_redirect 并置 stderr_append=true
    let p = parse("ls nodir 2>> err.txt", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["ls", "nodir"]);
    assert_eq!(p.stderr_redirect, Some("err.txt".to_string()));
    assert!(p.stderr_append);
    assert_eq!(p.stdout_redirect, None);
    assert!(!p.stdout_append);
}

#[test]
fn parse_truncate_keeps_append_false() {
    // 既有 `>` / `1>` / `2>` 路径 append 标志保持 false（回归保护）
    let p = parse("echo hi > out.txt", &empty_vars()).unwrap();
    assert_eq!(p.stdout_redirect, Some("out.txt".to_string()));
    assert!(!p.stdout_append);

    let p = parse("ls nodir 2> err.txt", &empty_vars()).unwrap();
    assert_eq!(p.stderr_redirect, Some("err.txt".to_string()));
    assert!(!p.stderr_append);
}

#[test]
fn parse_mixed_truncate_and_append_takes_last() {
    // `> a >> b`：最后一次是 `>>`，最终 append=true，目标 b
    let p = parse("cmd > a >> b", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["cmd"]);
    assert_eq!(p.stdout_redirect, Some("b".to_string()));
    assert!(p.stdout_append);

    // `>> a > b`：最后一次是 `>`，最终 append=false，目标 b
    let p = parse("cmd >> a > b", &empty_vars()).unwrap();
    assert_eq!(p.stdout_redirect, Some("b".to_string()));
    assert!(!p.stdout_append);

    // stderr 同理：`2> a 2>> b` 最终 append=true
    let p = parse("cmd 2> a 2>> b", &empty_vars()).unwrap();
    assert_eq!(p.stderr_redirect, Some("b".to_string()));
    assert!(p.stderr_append);
}

#[test]
fn parse_stdout_append_and_stderr_append_coexist() {
    // `>> out 2>> err`：两种追加重定向同时出现，互不干扰
    let p = parse("cmd a b >> out.txt 2>> err.txt", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["cmd", "a", "b"]);
    assert_eq!(p.stdout_redirect, Some("out.txt".to_string()));
    assert!(p.stdout_append);
    assert_eq!(p.stderr_redirect, Some("err.txt".to_string()));
    assert!(p.stderr_append);
}

#[test]
fn parse_append_missing_target_errors() {
    // `>>` / `1>>` / `2>>` 后无 token → 报 MissingRedirectTarget
    assert_eq!(
        parse("echo hi >>", &empty_vars()),
        Err(ParseError::MissingRedirectTarget)
    );
    assert_eq!(
        parse("echo hi 1>>", &empty_vars()),
        Err(ParseError::MissingRedirectTarget)
    );
    assert_eq!(
        parse("ls nodir 2>>", &empty_vars()),
        Err(ParseError::MissingRedirectTarget)
    );
}

#[test]
fn parse_stdout_append_with_stderr_inherit_split_stream() {
    // Split-stream 语义回归保护（codecrafters 2>> stage spec 第 1 条样例）：
    // `ls nonexistent >> /tmp/foo/baz.md` 仅追加重定向 stdout；stderr 字段保持
    // None / false，由上层 REPL 把外部命令的 stderr 设为 Stdio::inherit() 直通
    // 终端（这是 spec 中「ls: nonexistent: No such file or directory」仍在终端
    // 可见的根因）。一旦未来有人误把 `>>` 归类为「同时影响 stderr」，本测试
    // 立即报错。
    let p = parse("ls nonexistent >> /tmp/foo/baz.md", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["ls", "nonexistent"]);
    assert_eq!(p.stdout_redirect, Some("/tmp/foo/baz.md".to_string()));
    assert!(p.stdout_append);
    assert_eq!(p.stderr_redirect, None);
    assert!(!p.stderr_append);

    // 对称：`2>>` 仅作用于 stderr，stdout 保持 None / false（spec 第 2、4、5 条样例）。
    let p = parse("ls nonexistent 2>> /tmp/foo/qux.md", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["ls", "nonexistent"]);
    assert_eq!(p.stdout_redirect, None);
    assert!(!p.stdout_append);
    assert_eq!(p.stderr_redirect, Some("/tmp/foo/qux.md".to_string()));
    assert!(p.stderr_append);
}

// ===== 后台执行操作符 `&`：tokenize 层 =====
// 目的：确认 `&` 在引号外被切为独立 token（无论前是否有空白）；
//       引号内 / 转义后仍按字面量；既有用例不含 `&` 字符，零回归风险。

#[test]
fn redirect_amp_is_standalone_token_when_spaced() {
    // 空格分隔形态：`sleep 30 &` → `&` 独立 token
    assert_eq!(
        tokenize("sleep 30 &", &empty_vars()).unwrap(),
        vec!["sleep", "30", "&"]
    );
}

#[test]
fn redirect_amp_splits_adjacent_token() {
    // 紧贴形态：`sleep 30&` → 仍切出 `&` 独立 token
    assert_eq!(
        tokenize("sleep 30&", &empty_vars()).unwrap(),
        vec!["sleep", "30", "&"]
    );
}

#[test]
fn redirect_amp_inside_quotes_is_literal() {
    // 单引号内 `&` 是字面量
    assert_eq!(
        tokenize("echo '&'", &empty_vars()).unwrap(),
        vec!["echo", "&"]
    );
    // 双引号内 `&` 是字面量
    assert_eq!(
        tokenize(r#"echo "& not bg""#, &empty_vars()).unwrap(),
        vec!["echo", "& not bg"]
    );
    // 引号外反斜杠转义 `\&` → 字面 `&`，与前一字符拼接为同一 token
    assert_eq!(
        tokenize(r"echo a\&b", &empty_vars()).unwrap(),
        vec!["echo", "a&b"]
    );
}

// ===== 后台执行：parse 层 `background` 字段 =====

#[test]
fn parse_trailing_amp_sets_background() {
    // 空格分隔形态：argv 不含 `&`，background=true
    let p = parse("sleep 30 &", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["sleep", "30"]);
    assert!(p.background);
    assert_eq!(p.stdout_redirect, None);
    assert_eq!(p.stderr_redirect, None);
}

#[test]
fn parse_trailing_amp_no_space_sets_background() {
    // 紧贴形态：`sleep 30&` 同样剥离 `&` 并置位
    let p = parse("sleep 30&", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["sleep", "30"]);
    assert!(p.background);
}

#[test]
fn parse_no_amp_keeps_background_false() {
    // 默认情形：未带 `&` 时 background=false（回归保护）
    let p = parse("echo hi", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["echo", "hi"]);
    assert!(!p.background);
}

#[test]
fn parse_quoted_amp_known_limitation() {
    // 已知限制：tokenizer 输出扁平 `Vec<String>` 不携带 token 类型标签，
    // 故引号内字面量 `&`（`echo "&"` / `echo '&'`）与引号外操作符 `&` 在
    // token 层均表现为孤立字符串 `"&"`，parse 层会把末尾 `"&"` 一律当作
    // 后台标记剥离。该边界与 bash 真实行为偏离，但 codecrafters 测试不覆盖，
    // 作为已知简化保留——未来若引入 `enum Token { Word(String), Op(String) }`
    // 元数据，本测试应改为「argv 包含字面量 `&` 且 background=false」。
    //
    // 当前观察行为（用于回归锁定，并非期望行为）：
    let p = parse(r#"echo "&""#, &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["echo"]);
    assert!(p.background);

    let p = parse("echo '&'", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["echo"]);
    assert!(p.background);
}

#[test]
fn parse_redirect_and_background_coexist() {
    // `sleep 30 > out &`：先剥末尾 `&` 置 background=true，再扫描重定向
    let p = parse("sleep 30 > /tmp/out.log &", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["sleep", "30"]);
    assert_eq!(p.stdout_redirect, Some("/tmp/out.log".to_string()));
    assert!(!p.stdout_append);
    assert!(p.background);
}

#[test]
fn parse_amp_in_middle_is_literal_argv() {
    // 非末尾位置的 `&` token 按普通字面量参数留在 argv，不触发后台
    // （本阶段简化：未来支持 `cmd1 & cmd2` 复合形式时再升级）
    let p = parse("echo & hi", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["echo", "&", "hi"]);
    assert!(!p.background);
}

// ===== Pipeline 分隔符 `|`：tokenize 层 =====
// 目的：确认 `|` 在引号外被切为独立 token（无论前是否有空白）；
//       引号内 / 转义后仍按字面量；连续 `||` 切成两个独立 `|`，
//       由 parse 层经空 stage 检测命中错误。

#[test]
fn tokenize_pipe_unquoted_splits() {
    // 紧贴形态：`a|b` → `|` 独立 token
    assert_eq!(
        tokenize("a|b", &empty_vars()).unwrap(),
        vec!["a", "|", "b"]
    );
}

#[test]
fn tokenize_pipe_with_spaces() {
    // 空格分隔形态：`a | b` → 三 token
    assert_eq!(
        tokenize("a | b", &empty_vars()).unwrap(),
        vec!["a", "|", "b"]
    );
    // 三段 pipeline：`cat f | head -n 2 | wc -l`
    assert_eq!(
        tokenize("cat f | head -n 2 | wc -l", &empty_vars()).unwrap(),
        vec!["cat", "f", "|", "head", "-n", "2", "|", "wc", "-l"]
    );
}

#[test]
fn tokenize_pipe_quoted_literal() {
    // 单引号内 `|` 是字面量
    assert_eq!(
        tokenize("echo '|'", &empty_vars()).unwrap(),
        vec!["echo", "|"]
    );
    assert_eq!(
        tokenize("echo 'a|b'", &empty_vars()).unwrap(),
        vec!["echo", "a|b"]
    );
    // 双引号内 `|` 是字面量
    assert_eq!(
        tokenize(r#"echo "a|b""#, &empty_vars()).unwrap(),
        vec!["echo", "a|b"]
    );
    // 引号外反斜杠转义 `\|` → 字面 `|`，与前一字符拼接为同一 token
    assert_eq!(
        tokenize(r"echo a\|b", &empty_vars()).unwrap(),
        vec!["echo", "a|b"]
    );
}

#[test]
fn tokenize_double_pipe_two_independent_tokens() {
    // `a||b` → 两个独立 `|` token 中间夹空 stage；parse 层将报 EmptyPipelineSegment
    assert_eq!(
        tokenize("a||b", &empty_vars()).unwrap(),
        vec!["a", "|", "|", "b"]
    );
    assert_eq!(
        tokenize("a || b", &empty_vars()).unwrap(),
        vec!["a", "|", "|", "b"]
    );
}

// ===== Pipeline 结构：parse_pipeline 多段解析 =====

#[test]
fn parse_pipeline_two_stages() {
    let p = parse_pipeline("cat f | wc", &empty_vars()).unwrap();
    assert_eq!(p.stages.len(), 2);
    assert_eq!(p.stages[0].argv, vec!["cat", "f"]);
    assert_eq!(p.stages[1].argv, vec!["wc"]);
    assert!(!p.background);
}

#[test]
fn parse_pipeline_three_stages() {
    let p = parse_pipeline("a | b | c", &empty_vars()).unwrap();
    assert_eq!(p.stages.len(), 3);
    assert_eq!(p.stages[0].argv, vec!["a"]);
    assert_eq!(p.stages[1].argv, vec!["b"]);
    assert_eq!(p.stages[2].argv, vec!["c"]);
    assert!(!p.background);
}

#[test]
fn parse_pipeline_single_stage_no_pipe() {
    // 无 `|` → 单段 pipeline；background 跟随末尾 `&`
    let p = parse_pipeline("echo hi", &empty_vars()).unwrap();
    assert_eq!(p.stages.len(), 1);
    assert_eq!(p.stages[0].argv, vec!["echo", "hi"]);
    assert!(!p.background);
}

#[test]
fn parse_pipeline_with_background() {
    let p = parse_pipeline("a | b &", &empty_vars()).unwrap();
    assert_eq!(p.stages.len(), 2);
    assert_eq!(p.stages[0].argv, vec!["a"]);
    assert_eq!(p.stages[1].argv, vec!["b"]);
    assert!(p.background);
    // 子段 background 字段始终 false——后台是 pipeline 级别属性
    assert!(!p.stages[0].background);
    assert!(!p.stages[1].background);
}

#[test]
fn parse_pipeline_with_redirect_each_stage() {
    let p = parse_pipeline("cat f > out | wc 2> err", &empty_vars()).unwrap();
    assert_eq!(p.stages.len(), 2);
    assert_eq!(p.stages[0].argv, vec!["cat", "f"]);
    assert_eq!(p.stages[0].stdout_redirect, Some("out".to_string()));
    assert_eq!(p.stages[0].stderr_redirect, None);
    assert_eq!(p.stages[1].argv, vec!["wc"]);
    assert_eq!(p.stages[1].stdout_redirect, None);
    assert_eq!(p.stages[1].stderr_redirect, Some("err".to_string()));
}

#[test]
fn parse_pipeline_empty_first_segment_errors() {
    assert_eq!(
        parse_pipeline("| ls", &empty_vars()),
        Err(ParseError::EmptyPipelineSegment)
    );
}

#[test]
fn parse_pipeline_empty_last_segment_errors() {
    assert_eq!(
        parse_pipeline("ls |", &empty_vars()),
        Err(ParseError::EmptyPipelineSegment)
    );
}

#[test]
fn parse_pipeline_empty_middle_segment_errors() {
    assert_eq!(
        parse_pipeline("ls | | cat", &empty_vars()),
        Err(ParseError::EmptyPipelineSegment)
    );
}

#[test]
fn parse_pipeline_double_pipe_errors() {
    // `a || b` → 中间空 stage 触发 EmptyPipelineSegment（本阶段不实现逻辑 OR）
    assert_eq!(
        parse_pipeline("a || b", &empty_vars()),
        Err(ParseError::EmptyPipelineSegment)
    );
}

#[test]
fn parse_single_command_still_works_via_compat_wrapper() {
    // parse 兼容 wrapper：单段输入仍返回 ParsedCommand，与既有调用方零回归
    let p = parse("echo hello world", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["echo", "hello", "world"]);
    assert!(!p.background);

    let p = parse("sleep 30 &", &empty_vars()).unwrap();
    assert_eq!(p.argv, vec!["sleep", "30"]);
    assert!(p.background);
}

// ========================================================================
// $VAR 参数展开（parameter expansion）测试组
//
// 语义契约（与 plan 决策对齐）：
// - 引号外与双引号内：`$NAME` 按字符集 `[A-Za-z_][A-Za-z0-9_]*` 贪婪匹配并查表展开。
// - 单引号内：`$` 始终字面，不展开。
// - 未定义变量：展开为空串（与 bash 默认 LANG=C 一致；引号外保留为空 token）。
// - `$` 后非合法 NAME 首字符（数字 / `-` / 空白 / EOF / 引号等）：`$` 字面降级。
// - 反斜杠路径优先：`\$` 在 Normal / 双引号态先被反斜杠分支吃成字面 `$`，
//   不会触发展开分支（已由 double_quote_escapes_dollar 间接覆盖）。
// ========================================================================

#[test]
fn dollar_expansion_unquoted_hit() {
    // 题面 verbatim：echo $Variable_1 $Variable_2 → 展开后两个独立 argv 词
    let vars = vars_with(&[("Variable_1", "Value_1"), ("Variable_2", "Value2")]);
    assert_eq!(
        tokenize("echo $Variable_1 $Variable_2", &vars).unwrap(),
        vec!["echo", "Value_1", "Value2"]
    );
}

#[test]
fn dollar_expansion_unquoted_miss_is_empty_token() {
    // 未定义变量在引号外展开为空串，token 保留（不被过滤）
    assert_eq!(
        tokenize("echo $UNSET", &empty_vars()).unwrap(),
        vec!["echo", ""]
    );
}

#[test]
fn dollar_expansion_double_quoted_hit() {
    // 双引号内 `$NAME` 展开
    let vars = vars_with(&[("X", "hello")]);
    assert_eq!(
        tokenize(r#"echo "$X""#, &vars).unwrap(),
        vec!["echo", "hello"]
    );
}

#[test]
fn dollar_expansion_double_quoted_concat() {
    // 双引号内字面与 $VAR 拼接为同一个 token（对齐 bash word-splitting）
    let vars = vars_with(&[("Y", "VAL")]);
    assert_eq!(
        tokenize(r#"echo "x$Y z""#, &vars).unwrap(),
        vec!["echo", "xVAL z"]
    );
}

#[test]
fn dollar_expansion_double_quoted_miss_keeps_token() {
    // 双引号内未定义变量 → 空串拼接，token 仍存在（即便整个 token 为空）
    assert_eq!(
        tokenize(r#"echo "$UNSET""#, &empty_vars()).unwrap(),
        vec!["echo", ""]
    );
}

#[test]
fn dollar_expansion_single_quoted_is_literal() {
    // 单引号内 `$NAME` 不展开（即便变量已定义）
    let vars = vars_with(&[("X", "hello")]);
    assert_eq!(
        tokenize("echo '$X'", &vars).unwrap(),
        vec!["echo", "$X"]
    );
}

#[test]
fn dollar_expansion_escaped_unquoted_is_literal() {
    // Normal 态 `\$X` → 反斜杠分支吃 `$` 为字面，不触发展开；后续 `X` 按普通字符
    let vars = vars_with(&[("X", "hello")]);
    assert_eq!(
        tokenize(r"echo \$X", &vars).unwrap(),
        vec!["echo", "$X"]
    );
}

#[test]
fn dollar_expansion_invalid_first_char_digit() {
    // `$1abc`：1 非合法 NAME 首字符 → `$` 字面降级，后续 `1abc` 按普通字符
    assert_eq!(
        tokenize("echo $1abc", &empty_vars()).unwrap(),
        vec!["echo", "$1abc"]
    );
}

#[test]
fn dollar_expansion_invalid_first_char_dash() {
    // `$-`：`-` 非合法 NAME 首字符 → `$` 字面降级
    assert_eq!(
        tokenize("echo $-", &empty_vars()).unwrap(),
        vec!["echo", "$-"]
    );
}

#[test]
fn dollar_expansion_dollar_then_space_is_literal() {
    // `$<空格>`：空格非合法 NAME 首字符 → `$` 字面，独立成词
    assert_eq!(
        tokenize("echo $ x", &empty_vars()).unwrap(),
        vec!["echo", "$", "x"]
    );
}

#[test]
fn dollar_expansion_trailing_dollar_is_literal() {
    // 行尾孤立 `$`：EOF 后无字符 → `$` 字面
    assert_eq!(
        tokenize("echo $", &empty_vars()).unwrap(),
        vec!["echo", "$"]
    );
}

#[test]
fn dollar_expansion_adjacent_vars_concat() {
    // `$A$B` 紧邻：两个展开值在同一 token 内拼接
    let vars = vars_with(&[("A", "foo"), ("B", "bar")]);
    assert_eq!(
        tokenize("echo $A$B", &vars).unwrap(),
        vec!["echo", "foobar"]
    );
}

#[test]
fn dollar_expansion_underscore_and_digits_in_name() {
    // NAME 字符集：`_underscore_1` 合法（`_` 首字符 + 后续含数字/下划线）
    let vars = vars_with(&[("_underscore_1", "ok")]);
    assert_eq!(
        tokenize("echo $_underscore_1", &vars).unwrap(),
        vec!["echo", "ok"]
    );
}

#[test]
fn dollar_expansion_name_stops_at_punctuation() {
    // NAME 贪婪扫描在第一个非合法字符停止：`$X.txt` → 展开 `X` 后追加 `.txt` 字面
    let vars = vars_with(&[("X", "file")]);
    assert_eq!(
        tokenize("echo $X.txt", &vars).unwrap(),
        vec!["echo", "file.txt"]
    );
}

#[test]
fn dollar_expansion_double_quoted_escaped_dollar_is_literal() {
    // 双引号内 `\$X` → 反斜杠分支吃 `$` 为字面，X 跟随字面 push（即便 X 已定义）
    let vars = vars_with(&[("X", "hello")]);
    assert_eq!(
        tokenize(r#"echo "\$X""#, &vars).unwrap(),
        vec!["echo", "$X"]
    );
}
