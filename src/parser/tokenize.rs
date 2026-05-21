//! 词法层：把输入字符串切分为扁平 token 序列。
//!
//! 详见父模块 [`crate::parser`] 头注释中关于引号、转义与重定向操作符的语义说明。

use super::ParseError;

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
