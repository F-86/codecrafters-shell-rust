//! 词法层：把输入字符串切分为扁平 token 序列。
//!
//! 详见父模块 [`crate::parser`] 头注释中关于引号、转义、重定向操作符与
//! `$VAR` 变量展开的语义说明。

use super::ParseError;
use std::collections::HashMap;

/// NAME 首字符判定：`^[A-Za-z_]`，ASCII-only。
///
/// 与 `is_name_cont` 一起组成 bash POSIX valid-identifier
/// `^[A-Za-z_][A-Za-z0-9_]*$` 的字符级拆分，作为 `$VAR` 展开期 NAME
/// 扫描和 `declare` NAME 整串校验的**同源**判定逻辑——`builtins.rs`
/// 的 `is_valid_identifier` 也基于本对函数实现，跨 stage 100% 一致。
pub(crate) fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

/// NAME 后续字符判定：`[A-Za-z0-9_]`，ASCII-only。
///
/// 与 `is_name_start` 配对使用，参见后者文档。
pub(crate) fn is_name_cont(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// 词法分析器内部状态。
enum State {
    /// 引号外：空白作分隔符，遇到 `'` / `"` 进入对应引号态；
    /// 遇到 `$NAME` 触发变量展开（合法 NAME 替换为值，未命中替换为空串，
    /// `$` 后非合法首字符则按字面量保留 `$`）。
    Normal,
    /// 单引号内部：任何字符（除 `'`）都按字面量追加；`$` 不触发展开。
    InSingleQuote,
    /// 双引号内部：除 `"` 外大多数字符按字面量追加；`\` 仅对
    /// `"`、`\`、`$`、`` ` `` 这 4 个字符触发转义（吃掉自身），其他字符前
    /// `\` 按字面量保留。`$NAME` 与引号外语义一致触发变量展开；`\$` 路径
    /// 已在反斜杠分支一次性消费 `$` 并 push 字面，下一轮主循环看到的是
    /// `$` 之后的字符，绝不会进入 `$` 展开分支——天然「字面 `$` 不展开」。
    InDoubleQuote,
}

/// 将一行命令切分为 token 序列。
///
/// 返回的 `Vec<String>` 中每个元素对应最终传给命令的一个 argv；
/// 相邻引号 / 空引号 / 裸字符串拼接已在内部完成。
///
/// `vars` 为只读 shell 变量视图，用于 `$NAME` 展开：引号外与双引号内
/// 命中 NAME 替换为值、未命中替换为空串；单引号内 `$` 永远字面保留。
pub fn tokenize(
    input: &str,
    vars: &HashMap<String, String>,
) -> Result<Vec<String>, ParseError> {
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
                '$' => {
                    // 引号外 `$NAME` 变量展开：peek 下一字符判定是否为合法 NAME 首字符。
                    // - 合法：贪婪扫描 NAME（`is_name_cont`），用 NAME 查 vars——
                    //   命中 push 值、未命中 push 空串；in_token 置真（即使展开为空串
                    //   也开启 token，与 `""` 显式空 token 行为一致）。
                    // - 非法（数字 / `-` / 空白 / 引号 / EOF 等）：把 `$` 当字面字符
                    //   push，进入下一轮主循环按原规则处理后续字符（即 q4 决策的
                    //   「`$` 字面降级」：`$1abc` 字面输出、`$-` 字面输出、行尾 `$` 字面）。
                    //
                    // `chars.clone().next()` 是 O(1) 安全 peek（std `Chars: Clone` 仅克隆
                    // 内部 &[u8] 指针）。
                    if matches!(chars.clone().next(), Some(c) if is_name_start(c)) {
                        let mut name = String::new();
                        // 贪婪消费 NAME 字符：首字符已通过 peek 校验为 is_name_start，
                        // 直接 chars.next() 一次性收入；后续字符循环 peek + next。
                        while let Some(&_) = chars.clone().next().as_ref() {
                            // 双重 peek 模式：先 clone peek 拿 char，命中 is_name_cont
                            // 才真正 next 消费。比把 `next()` 结果存起来再判定更直观，
                            // 且对 NAME 末尾字符的非合法判定不会误吃。
                            if let Some(c) = chars.clone().next() {
                                if is_name_cont(c) {
                                    chars.next();
                                    name.push(c);
                                } else {
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                        // NAME 至少 1 字符（首字符已通过 peek 校验）
                        if let Some(value) = vars.get(&name) {
                            current.push_str(value);
                        }
                        // 未命中：q2 决策展开为空串（current 不追加任何字符）
                        in_token = true;
                    } else {
                        // q4 决策：`$` 后非合法首字符 → `$` 按字面量降级
                        current.push('$');
                        in_token = true;
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
                    // 重定向操作符 `>` / `>>`：作为独立 token 切出。
                    //
                    // 第一步：peek 下一字符以判定是否为 `>>` 追加形式。
                    // `Chars::clone()` 在 std 中是 O(1)（仅克隆 &[u8] 迭代器内部指针），
                    // 故 peek 不影响词法分析整体复杂度。若下一字符也是 `>`，则消费它
                    // 并产出 `">>"` 系列 token；否则按既有 `>` 系列产出。
                    //
                    // 第二步：根据当前累积 token 是否恰好为裸字符 `"1"` / `"2"` 决定
                    // 是否合并为 `"1>"` / `"2>"` / `"1>>"` / `"2>>"`——`1` / `2` 与 `>` 之间
                    // 任何空白、引号、转义都会先 flush 出 current 或改变其值，使合并
                    // 条件天然不满足。
                    let is_append = matches!(chars.clone().next(), Some('>'));
                    if is_append {
                        chars.next(); // 消费第二个 `>`
                    }
                    let (op_one, op_two) = if is_append {
                        ("1>>", "2>>")
                    } else {
                        ("1>", "2>")
                    };
                    let op_plain = if is_append { ">>" } else { ">" };
                    if in_token && current == "1" {
                        current.clear();
                        tokens.push(op_one.to_string());
                    } else if in_token && current == "2" {
                        current.clear();
                        tokens.push(op_two.to_string());
                    } else if in_token {
                        tokens.push(std::mem::take(&mut current));
                        tokens.push(op_plain.to_string());
                    } else {
                        tokens.push(op_plain.to_string());
                    }
                    in_token = false;
                }
                '|' => {
                    // Pipeline 分隔符 `|`：在引号外作为独立 token 切出，
                    // 与 `>` / `&` 切分规则对称（无论前是否有空白）。
                    //
                    // 设计要点：
                    // - 引号外 `|` 永远独立成 token，使 parse 层只需按 `"|"` 切分子序列，
                    //   即可识别 N 段 pipeline，逻辑最简、无歧义。
                    // - 引号内（InSingleQuote / InDoubleQuote）的 `|` 不会进入这里——
                    //   那两个分支对一切非闭合字符按字面量保留——故 `echo "|"` / `echo '|'`
                    //   仍输出字面 `|`，不触发 pipeline 语义。
                    // - 引号外反斜杠转义 `\|` 同样在 Normal 态 `\\` 分支先于此分支处理，
                    //   下一字符被消费为字面量，绕过本分支。
                    // - 本阶段不实现 `||`（逻辑 OR）：连续两个 `|` 切成两个独立 `"|"` token，
                    //   由 parse 层经空 stage 检测命中 `EmptyPipelineSegment` 错误——这与 bash
                    //   缺少 `||` 实现时最近似行为（用户能立即得知语法错误）。
                    if in_token {
                        tokens.push(std::mem::take(&mut current));
                    }
                    tokens.push("|".to_string());
                    in_token = false;
                }
                '&' => {
                    // 后台执行操作符 `&`：在引号外作为独立 token 切出，
                    // 与 `>` 切分规则对称（无论前是否有空白）。
                    //
                    // 设计理由：若把 `&` 留给主 ch 分支当字面量，则 `sleep 30&` 会切成
                    // `["sleep", "30&"]`，迫使 parse 层做「token 末尾字符串切分」二次解析，
                    // 引入双重数据源。让词法层统一切出独立 `"&"` token，使 parse 层只需
                    // 「检查并 pop 最后一个 token」即可识别后台标记，逻辑最简、无歧义。
                    //
                    // 引号内（InSingleQuote / InDoubleQuote）的 `&` 不会进入这里——
                    // 那两个分支对一切非闭合字符都按字面量保留——故 `echo "&"` / `echo '&'`
                    // 仍输出字面 `&`，不触发后台语义。引号外反斜杠转义 `\&` 同样在 Normal 态
                    // `\\` 分支先于此分支处理，下一字符被消费为字面量，绕过本分支。
                    //
                    // 本阶段仅 single `&` 语义；未来扩展 `&&`（逻辑 AND）时可在此 peek
                    // 下一字符按 `>>` 模板升级，目前保持单字符 token。
                    if in_token {
                        tokens.push(std::mem::take(&mut current));
                    }
                    tokens.push("&".to_string());
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
                    // 引号内一切字符（含空白与特殊字符，包括 `\` 与 `$`）按字面量保留
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
                    // 注：`\$` 提前消费 `$` 作为字面 push，下一轮主循环看到的是 `$`
                    // 之后的字符（绝不再进入下方 `$` 展开分支），天然实现 bash 的
                    // 「`\$VAR` 在双引号内不展开」语义，无需额外标记位。
                    // `std::str::Chars` 实现 `Clone`，`chars.clone().next()` 是 O(1) 安全 peek。
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
                '$' => {
                    // 双引号内 `$NAME` 展开：与 Normal 态分支语义完全一致，
                    // 仅状态机所属分支不同。重复实现而非抽函数：
                    // - 共享 `chars` / `current` / `in_token` 多个可变借用，抽函数
                    //   会引入 5 参签名，可读性反而下降；
                    // - 两态分支的展开行为契约对称，两份小代码块比一份带状态参数
                    //   的函数更易跟踪与维护。
                    if matches!(chars.clone().next(), Some(c) if is_name_start(c)) {
                        let mut name = String::new();
                        while let Some(c) = chars.clone().next() {
                            if is_name_cont(c) {
                                chars.next();
                                name.push(c);
                            } else {
                                break;
                            }
                        }
                        if let Some(value) = vars.get(&name) {
                            current.push_str(value);
                        }
                        // 未命中：展开为空串（双引号内 token 已经 in_token=true，
                        // 这里无需再设置 in_token——双引号开启时已置真）。
                    } else {
                        // 双引号内 `$` 后非合法首字符 → 字面保留 `$`
                        current.push('$');
                    }
                }
                c => {
                    // 引号内其他字符（含空白、单引号、`*`、`;` 等）按字面量保留
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
