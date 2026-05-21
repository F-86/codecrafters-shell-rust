---
name: brace-parameter-expansion
overview: 在已有 `$NAME` 展开能力之上，新增 `${NAME}` 大括号形式的参数展开：双引号内与引号外均支持，单引号内字面，与上一 stage 完全同源；同时新增 2 类语法错误以严格契合 bash 语义。
todos:
  - id: extend-parse-error
    content: 在 src/parser/mod.rs 的 ParseError 中新增 BadSubstitution 与 UnterminatedBraceExpansion 两个变体并补 Display 实现，更新模块头注释追加 ${NAME} 语义条款
    status: completed
  - id: impl-brace-expansion
    content: 在 src/parser/tokenize.rs 的 Normal 与 InDoubleQuote 两态 '$' 分支顶部插入大括号子分支，扫描到 '}' 后整串 NAME 校验，命中查 vars 替换为值，未命中空串，错误对应抛 BadSubstitution / UnterminatedBraceExpansion
    status: completed
    dependencies:
      - extend-parse-error
  - id: add-brace-tests
    content: 在 src/parser/tests.rs 末尾追加 12–15 个 ${VAR} 覆盖用例：题面 verbatim、双引号、单引号字面、\${X} 转义、紧邻拼接、边界字面拼接、各类非法形式与未闭合错误
    status: completed
    dependencies:
      - impl-brace-expansion
  - id: verify-and-e2e
    content: 运行 cargo build / cargo test 确认零警告零回归，端到端 printf 注入题面 verbatim 序列（declare Item=widget / declare Foo1=Bar2 / echo stock_${Item}_id ${Foo1}）并人工核验 stdout 与 argv 边界
    status: completed
    dependencies:
      - add-brace-tests
---

## Product Overview

为 codecrafters-shell-rust 添加 `${VAR}` 大括号参数展开能力，作为上一 stage `$VAR` 展开的延伸——让用户可以显式标记变量名边界，使变量值与紧邻字面字符无歧义拼接。

## Core Features

- **大括号展开**：在引号外与双引号内识别 `${NAME}` 形式，将整段替换为变量值，命中走 `vars` 表，未命中替换为空串（与 `$NAME` 语义对齐）。
- **边界明确化**：`${Var1}end` 展开后立即拼接字面 `end`，无需空白分隔，从而消除 `$Var1end` 被读作变量 `Var1end` 的歧义。
- **单引号字面**：`'${X}'` 内的  `与 `{}` 全部按字面量保留，单引号语义完全不变。
- **反斜杠转义**：`\${X}` 在 Normal 态与双引号内复用现有反斜杠分支—— `被先吃成字面 push，残余 `{X}` 按普通字面字符流入，结果为字面 `${X}`，零额外代码。
- **严格错误**：
- 内部 NAME 不合法（空 `${}`、首字符非法 `${1abc}`、含非 NAME 字符 `${X-Y}` 等）→ `ParseError::BadSubstitution`。
- 行尾未闭合（缺少 `}`）→ `ParseError::UnterminatedBraceExpansion`。
- **题面 verbatim 通过**：`declare Item=widget` / `declare Foo1=Bar2` / `./custom_exe_1234 stock_${Item}_id ${Foo1}` 严格切出 argv `["custom_exe_1234", "stock_widget_id", "Bar2"]`。

## Tech Stack

- 沿用现有 Rust 词法状态机（`src/parser/tokenize.rs`），不引入任何新依赖。
- 复用 `is_name_start` / `is_name_cont` 字符级 helper，跨 stage 同源。

## Implementation Approach

**策略**：在现有 `'` 分支顶部增加一层 peek 分流——若下一字符为 `{`，进入大括号子分支；否则按原 `$NAME` 路径不变。大括号子分支扫描到 `}` 闭合后，对内部字符串做整串 NAME 校验，命中查表 push 值（未命中空串），失败抛 `BadSubstitution`，未闭合抛 `UnterminatedBraceExpansion`。Normal 与 InDoubleQuote 两态对称实现，遵循上一 stage 注释中确立的「重复小代码块优于多参函数」原则（避免共享 `chars`/`current` 多重可变借用引发的签名膨胀）。

**关键决策**：

- **整串校验而非贪婪扫描**：`${...}` 内部一旦遇到非合法字符立即终止扫描会让 `${X-Y}` 这类输入静默截断，与 q2 严格语义不符。改为先收集到 `}` 再用 `is_name_start(first) && rest.all(is_name_cont)` 一次性判定，错误信息更精确。
- **`\${VAR}` 零代码**：双引号 `\\` 分支转义白名单已含 `（tokenize.rs:246），Normal 态 `\\` 分支无条件转义；两路径下  `都被先消费为字面 push，下一轮主循环看到的是 `{` 字面字符，天然得到 `${VAR}` 字面输出，无需新增分支。
- **错误恢复**：复用现有 `Result<Vec<String>, ParseError>` 协议，REPL 上层已统一打印 `syntax error: ...`，无需调整调用链。

**性能与复杂度**：

- 大括号扫描每输入字符 O(1) 推进；`Chars::clone().next()` 在 std 中是 O(1) peek（仅克隆 `&[u8]` 内部指针），整体 tokenize 仍为 O(n)。
- 校验阶段对 NAME 字符串做单次线性扫描，无回溯。

## Implementation Notes

- **接入点最小化**：tokenize.rs Normal 态分支（第 79–120 行）与 InDoubleQuote 态分支（第 258–284 行）各插入约 25 行大括号子分支；其余文件零改动或仅追加（mod.rs 新增 2 个 enum 变体 + display 分支 + 注释；tests.rs 仅追加用例）。
- **不破坏现有 209 个测试**：旧 `$NAME` 路径完全保留，新逻辑仅在 peek 到 `{` 时分流，无回归风险。
- **错误命名一致性**：`BadSubstitution` 沿用 bash 错误术语；`UnterminatedBraceExpansion` 与 `UnterminatedSingleQuote` / `UnterminatedDoubleQuote` 三件套对齐。
- **日志/可观测性**：tokenize 层无日志依赖，错误经 `ParseError` 上抛；REPL 既有 `eprintln!` 输出路径覆盖新错误（`Display` 实现到位即可）。
- **向后兼容**：`tokenize` / `parse` / `parse_pipeline` 签名不变，所有调用点零改动。

## Architecture Design

```mermaid
flowchart TD
    A[tokenize 主循环] --> B{当前字符}
    B -->|'$'| C[peek 下一字符]
    C -->|'{'| D["大括号子分支(新增)"]
    C -->|is_name_start| E[原 $NAME 路径]
    C -->|其他| F[$ 字面降级]
    D --> G[扫描到 '}' 或 EOF]
    G -->|EOF| H[Err UnterminatedBraceExpansion]
    G -->|'}' 闭合| I["NAME 整串校验"]
    I -->|合法| J[查 vars 命中 push 值<br/>未命中 push 空串]
    I -->|非法| K[Err BadSubstitution]
```

两态分支（Normal / InDoubleQuote）都按上图分流；InSingleQuote 不受影响（ `与 `{}` 在该态全部按字面量保留）。

## Directory Structure

```
src/
├── parser/
│   ├── tokenize.rs   # [MODIFY] 在 Normal 与 InDoubleQuote 两态的 '$' 分支顶部插入「peek '{' → 大括号子分支」分流。
│   │                 #          子分支：循环 chars.next() 收集到 '}' 闭合或 EOF；EOF→Err(UnterminatedBraceExpansion)；
│   │                 #          闭合后用 is_name_start(first) && rest.all(is_name_cont) 校验整串 NAME，
│   │                 #          失败→Err(BadSubstitution)，命中查 vars push 值（未命中空串），in_token=true。
│   │                 #          两态对称实现（不抽函数，沿用上一 stage 注释中的设计原则）。
│   ├── mod.rs        # [MODIFY] ParseError 枚举新增 BadSubstitution / UnterminatedBraceExpansion 两个变体；
│   │                 #          fmt::Display 补充对应 message（"syntax error: bad substitution" /
│   │                 #          "syntax error: unterminated brace expansion"）；
│   │                 #          模块头注释追加「${NAME} 大括号展开」语义条款，与现有 $NAME 条款并列。
│   └── tests.rs      # [MODIFY] 在文件末尾追加约 12–15 个 ${VAR} 覆盖用例：
│                     #          - 题面 verbatim：${Item}/${Foo1} 与字面前后缀拼接、stock_${Item}_id；
│                     #          - 双引号内 "${X}" 命中 / 未命中 / 与字面拼接；
│                     #          - 单引号内 '${X}' 字面保留；
│                     #          - 引号外与双引号内 \\${X} 字面输出 ${X}；
│                     #          - 紧邻 ${A}${B} 拼接、${X}end / start${X}/${X}.txt 边界；
│                     #          - 错误形式：${ / ${X / ${} / ${1abc} / ${X-Y} / ${ } 报对应 ParseError；
│                     #          - 下划线/数字 NAME：${_x_1} 命中。
```

`src/parser/parse.rs`、`src/main.rs`、`src/completion.rs`、`src/builtins.rs` 均无需改动（签名稳定、复用同源 helper）。

## Key Code Structures

```rust
// src/parser/mod.rs — ParseError 枚举新增变体
pub enum ParseError {
    // ... 原有变体保留
    /// `${...}` 内部 NAME 非法（空、首字符非法、含非 NAME 字符等）。
    BadSubstitution,
    /// `${` 行尾未见闭合 `}`。
    UnterminatedBraceExpansion,
}
```