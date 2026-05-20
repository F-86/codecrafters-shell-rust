---
name: shell-double-quote-tokenizer
overview: 在现有支持单引号的 tokenizer 上扩展双引号语义：状态机加入 InDoubleQuote 态，引号内空白保留、特殊字符字面化，与其它引号/裸串相邻拼接；新增未闭合双引号错误变体。本阶段不处理 $ 变量展开与 \\ 转义。
todos:
  - id: extend-parser
    content: 扩展 src/parser.rs：State 增加 InDoubleQuote，ParseError 增加 UnterminatedDoubleQuote，补全 Normal 与新状态的字符处理分支
    status: completed
  - id: add-tests
    content: 在 parser.rs 单元测试中新增 7 个双引号用例：空格保留、双双/双单/双裸拼接、内含单引号字面量、cat 路径、未闭合错误
    status: completed
    dependencies:
      - extend-parser
  - id: verify-stage
    content: 运行 cargo test 与 REPL 端到端验证（echo 三种拼接、cat 双引号路径、未闭合错误），确认 cargo build 通过
    status: completed
    dependencies:
      - add-tests
---

## 用户需求

为 Rust 实现的 shell 增加双引号（`"`）解析能力。双引号内大部分字符按字面量处理（含空格、`'`、`*`、`~`、`;` 等），同时支持双引号串之间、双引号与单引号之间、引号与裸串之间的相邻拼接。

## 产品概述

在已完成单引号支持的 tokenizer 基础上，扩展双引号语义，使 `echo`、`cat` 等命令都能正确接收双引号包裹的参数。本阶段不处理 ` 变量展开与 `\` 反斜杠转义（后续阶段实现），双引号内的 ` 与 `\` 均按字面量保留以满足当前测试。

## 核心功能

- 双引号内连续空白完整保留：`echo "hello    world"` → `hello    world`。
- 双引号内的单引号、`*`、`~`、`;` 等特殊字符按字面量保留：`echo "shell's test"` → `shell's test`。
- 双引号串相邻拼接：`echo "hello""world"` → `helloworld`。
- 双引号与单引号、裸串相邻拼接：`echo "hello"world`、`"a"'b'c` 均生成单个 argument。
- 引号外仍按空白分隔并折叠；多个引号参数能独立成 argv：`echo "quz  hello"  "bar"` → `quz  hello bar`。
- 外部命令同样消费该 tokenizer：`cat "/tmp/file name" "/tmp/'file name' with spaces"` 能把两个含空格路径作为独立 argv 传给子进程。
- 未闭合双引号视为本行解析失败，打印简短错误，REPL 继续运行。

## 技术栈

- 语言：Rust 2024 edition，沿用现有 `Cargo.toml`，本阶段不新增依赖。
- 仅修改 `src/parser.rs`，`src/main.rs` 无需任何改动（它只消费 `Vec<String>`）。

## 实现思路

在现有两态状态机上对称地新增一个 `InDoubleQuote` 状态，使整个 tokenizer 保持「单一字符级 O(n) 扫描 + `in_token` 拼接标志」的统一形态，零特殊分支即可满足所有相邻拼接组合。

- `State` 枚举新增 `InDoubleQuote` 变体（与 `InSingleQuote` 完全对称）。
- `Normal` 分支新增对 `"` 的处理：切到 `InDoubleQuote` 并将 `in_token` 置为真（不追加 `"` 本身）。
- 新增 `InDoubleQuote` 分支：
- 遇 `"`：闭合回 `Normal`，`in_token` 保持为真，支持后续相邻拼接。
- 遇其它任意字符（含空白、`'`、`、`*`、`\`、`;` 等）：原样字面量追加。
- 行尾仍处于 `InDoubleQuote` → 返回新错误变体 `ParseError::UnterminatedDoubleQuote`，REPL 走与单引号同款的 `eprintln!` + `continue` 路径，无需改 main.rs。

### 关于 ` 与 `\`

spec 明确指出 ` 和 `\` 的特殊语义留到后续阶段。本阶段在双引号内将它们当作普通字符处理：

- 既满足当前测试（测试用例中无 ` 变量、无 `\` 转义）；
- 后续阶段只需在 `InDoubleQuote` 分支中针对 ` / `\` 增加分支，不会推翻当前结构；
- 避免与未来阶段功能重叠造成返工。

## 实现要点（Implementation Notes）

- 错误信息文本：`syntax error: unterminated double quote`，与单引号错误格式完全对称，便于测试 / 用户识别。
- `in_token` 标志的语义无需调整：进入任一引号都设为真，退出后保持真直到遇引号外空白；这天然支持 `"a""b"` / `"a"'b'` / `"a"b` / `a"b"` / `''""` 等全部组合。
- 复杂度仍是 O(n) 单次扫描，无回溯；单行典型 < 1KB，性能无压力。
- 兼容性：单引号、未引号、空行、错误处理路径在新逻辑下行为完全不变，旧测试全部保留。
- 字符迭代统一用 `input.chars()`，UTF-8 安全；本阶段不改字节索引模型。

## 架构设计

保持单 binary、`main.rs` + `parser.rs` 的扁平结构，仅在 parser 内部扩状态机：

```mermaid
flowchart LR
    A[stdin 行] --> B[parser::tokenize]
    B --> N[Normal]
    N -->|遇 '| S[InSingleQuote]
    N -->|遇 "| D[InDoubleQuote]
    S -->|遇 '| N
    D -->|遇 "| N
    B -->|Ok Vec String| C[main 命令分发]
    B -->|Err 未闭合单/双引号| E[打印错误 continue]
```

## 目录结构

```
codecrafters-shell-rust/
├── src/
│   ├── main.rs    # 无改动。现有错误处理 `eprintln!("{}", e); continue;` 直接覆盖新错误变体的 Display 输出。
│   └── parser.rs  # [MODIFY] 在现有 tokenizer 上扩展双引号语义。
│                  #   - ParseError 新增 UnterminatedDoubleQuote 变体；
│                  #     在 Display 中输出 "syntax error: unterminated double quote"；
│                  #   - State 枚举新增 InDoubleQuote 变体；
│                  #   - Normal 分支新增 '"' 匹配，切到 InDoubleQuote 且 in_token = true；
│                  #   - 新增 InDoubleQuote 分支：'"' 切回 Normal；其它字符（含空白、单引号、$、\、*、;、~）按字面量 current.push(c)；
│                  #   - 行尾仍在 InDoubleQuote → 返回 ParseError::UnterminatedDoubleQuote；
│                  #   - 单元测试新增 7 个用例覆盖：
│                  #     * 双引号保留连续空白（"hello    world"）
│                  #     * 双引号相邻拼接（"hello""world"）
│                  #     * 双引号与裸串拼接（"hello"world、hello"world"）
│                  #     * 双引号与单引号拼接（"a"'b'、'a'"b"）
│                  #     * 双引号内单引号字面量（"shell's test"）
│                  #     * 多个双引号参数 + cat 测试用例（"quz  hello"  "bar"、cat 双引号路径）
│                  #     * 未闭合双引号返回 Err；
│                  #   - 现有 8 个单元测试保持不变并继续通过。
└── Cargo.toml     # 无改动。
```

## 关键接口

```rust
// src/parser.rs（仅枚举变体新增，函数签名不变）
pub enum ParseError {
    UnterminatedSingleQuote,
    UnterminatedDoubleQuote, // [NEW]
}

// 状态机内部枚举
enum State {
    Normal,
    InSingleQuote,
    InDoubleQuote, // [NEW]
}

// 公开 API 不变
pub fn tokenize(input: &str) -> Result<Vec<String>, ParseError>;
```