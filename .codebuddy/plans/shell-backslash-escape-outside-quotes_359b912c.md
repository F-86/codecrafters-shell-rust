---
name: shell-backslash-escape-outside-quotes
overview: 在现有 tokenizer 上扩展「引号外反斜杠转义」语义：Normal 态遇 `\` 时消费下一字符并按字面量追加，同时开启/延续当前 token；行尾孤立 `\` 视为语法错误。引号内 `\` 行为本阶段保持不变。
todos:
  - id: extend-parser-backslash
    content: 扩展 src/parser.rs：ParseError 新增 TrailingBackslash 变体并补 Display；主循环改为 while-let 以消费下一字符；Normal 分支新增 `\` 入口（push 下一字符并置 in_token=true，None 时返错）；顶部 doc-comment 同步更新
    status: completed
  - id: add-backslash-tests
    content: 在 parser.rs 测试模块新增 8 个用例：转义空格保持 token、转义空格与未转义空格混合、转义字母丢反斜杠、双反斜杠、转义单/双引号字面量、cat 多文件转义路径、单引号内 `\` 字面量回归守护、行尾孤立反斜杠错误
    status: completed
    dependencies:
      - extend-parser-backslash
  - id: verify-stage
    content: 运行 cargo test 与 REPL 端到端验证（spec 三个 echo 用例 + cat 三文件用例 + 行尾 `\` 错误），并执行 cargo build --release 确认无警告
    status: completed
    dependencies:
      - add-backslash-tests
---

## 用户原始需求

在 Rust shell 中支持**引号外**的反斜杠转义：`\X` 移除 `X` 的特殊含义并按字面量保留 `X`，反斜杠本身被丢弃。覆盖任意下一字符（空白、`'`、`"`、`、`*`、`?`、字母等）。

## 核心特性

- 引号外 `\<空格>` 是字面量空格，并保持当前 token 不被分隔
- 引号外 `\'`、`\"` 是字面量引号，不进入引号态
- 引号外 `\\` 产出一个字面量反斜杠
- 引号外 `\<普通字符>`（如 `\n`、`\_`、`\2`）丢弃反斜杠，保留字面字符（不做 C 风格转义）
- 引号内（单 / 双引号）`\` 行为不变，仍按字面量保留（spec 范围之外）
- 行尾孤立 `\`：按错误处理，打印 `syntax error: trailing backslash` 后继续 REPL

## 验证用例

- `echo multiple\ \ \ \ spaces` → `multiple    spaces`
- `echo \'\"literal quotes\"\'` → `'"literal quotes"'`
- `echo ignore\_backslash` → `ignore_backslash`
- `cat /tmp/\_ignored_1 /tmp/ignore_\2 /tmp/just_one_\\_3` → 三个文件内容拼接

## 技术栈

沿用既有：Rust 2021、`std` 单文件模块化（`src/parser.rs` + `src/main.rs`），无新增依赖。

## 实现方案

仅修改 `src/parser.rs`，扩展三态状态机的 `Normal` 分支，新增 `\` 入口；`InSingleQuote `/ `InDoubleQuote` 不动。`main.rs` 因消费 `Vec<String>` + `Display` 错误，自动适配新增错误变体，**无需改动**。

### 关键技术决策

1. **迭代器改造**：`for ch in input.chars()` → `let mut chars = input.chars(); while let Some(ch) = chars.next()`，以便 `\` 分支可调用 `chars.next()` 消费下一字符。结构对外不变，复杂度仍 O(n)。
2. **`in_token` 不变量复用**：`\X` 分支统一执行 `current.push(c); in_token = true;`。这一行同时解决：

- `\<空格>` 不分隔 token（不走普通空白的 push 分支）
- `\X` 作为参数首字符可独立开启 token（如 `\_ignored_1`）
- 与左右相邻引号 / 裸串自然拼接（如 `\'hello\'`）

3. **行尾 `\` 错误化**：与 `UnterminatedSingleQuote`、`UnterminatedDoubleQuote` 同风格，新增 `ParseError::TrailingBackslash`。REPL 已有 `eprintln!("{}", e); continue;` 兜底，零改动适配。
4. **不在引号内做转义**：spec 明示 "outside of quotes"；双引号内 `\` 的部分转义留待后续阶段，避免本阶段过度设计。

### 性能与可靠性

- 单次字符级扫描，O(n) 时间、O(n) 空间（输出 token 累积）
- `chars.next()` 返回 `Option<char>`，`None` 显式处理为 `TrailingBackslash`，无 panic 路径
- 无新增分配点，`std::mem::take` 复用既有

## 实现细节注释

### 关键不变量

- `Normal` 态遇 `\`：消费下一字符 `c`，`current.push(c); in_token = true;`；若无下一字符 → `Err(TrailingBackslash)`
- `\<whitespace>` 路径**不**走 `is_whitespace()` 分支，因此不触发 token 边界，这是 `three\ \ \ spaces` 合并为单 token 的核心
- 引号内分支保持只匹配闭合引号 + 字面追加，**不**新增 `\` 入口

### 既有行为守护（防回归）

- 单引号内 `\` 仍按字面量：`'a\b' `→ `a\b`（新增专项测试）
- 双引号内空白、单引号字面量、跨引号拼接行为均不受影响（既有 7 个双引号测试覆盖）

### 错误处理与日志

- 复用 `eprintln!("{}", e)` 单行输出，错误信息精简且可定位（`syntax error: trailing backslash`）；不打印输入原文以避免长行刷屏

## 目录结构

```
project-root/
├── src/
│   ├── parser.rs   # [MODIFY] 扩展 Normal 分支处理 `\`，新增 TrailingBackslash 错误变体；
│   │               #          重写主循环为 while-let 以支持消费下一字符；
│   │               #          顶部 doc-comment 更新本阶段语义；
│   │               #          新增 ≥8 个单元测试覆盖 spec 全部用例 + 既有行为守护
│   └── main.rs     # [UNCHANGED] 现有 ParseError Display + continue 兜底自动适配新错误变体
```

## 关键代码结构（仅接口）

```rust
pub enum ParseError {
    UnterminatedSingleQuote,
    UnterminatedDoubleQuote,
    TrailingBackslash,           // 新增
}

// 签名不变
pub fn tokenize(input: &str) -> Result<Vec<String>, ParseError>;
```