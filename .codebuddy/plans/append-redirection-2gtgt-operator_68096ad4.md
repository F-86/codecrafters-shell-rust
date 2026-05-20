---
name: append-redirection-2gtgt-operator
overview: 为 shell 增加 `>>` / `1>>` / `2>>` 追加重定向：扩展 tokenizer 识别 `>>` / `1>>` / `2>>`，给 ParsedCommand 的 stdout/stderr 重定向加上 append 标志，sink 打开逻辑新增追加模式，外部命令分支同步使用追加打开的 File 物化 Stdio。
todos:
  - id: extend-tokenize-append
    content: 扩展 src/parser.rs tokenize 在 `'>'` 分支首 peek 下一字符识别 `>>`，按 current=="1"/"2" 合并为 `>>` / `1>>` / `2>>`；新增 ≥3 个 tokenize 单测覆盖紧贴/空格/引号内/转义/数字前缀合并
    status: completed
  - id: extend-parsed-command-append
    content: 为 ParsedCommand 新增 stdout_append / stderr_append 布尔字段；parse 循环识别 4 个新操作符 token 并设置 append 标志；新增 ≥3 个 parse 单测含混用取最后一次、`2>>` 缺目标报错
    status: completed
    dependencies:
      - extend-tokenize-append
  - id: param-open-sinks-append
    content: 在 src/main.rs 给 open_sink / open_err_sink 追加 append 参数（true 走 OpenOptions.append、false 保持 File::create）；新增 open_file_for_redirect helper；REPL 两处调用补传 append；外部命令分支两处 File::create 改用 helper
    status: completed
    dependencies:
      - extend-parsed-command-append
  - id: cargo-test-and-build
    content: 运行 cargo test 验证既有 52 + 新增 ≥6 个测试全绿，cargo build --release 无警告
    status: completed
    dependencies:
      - param-open-sinks-append
  - id: e2e-verify-spec-samples
    content: 在 /tmp/baz 与 /tmp/bar 端到端回放 spec 三条样例（连续 `>>` 累加、`1>>` 等价、`>` 后 `>>` 混用），验证文件内容并清理
    status: completed
    dependencies:
      - cargo-test-and-build
---

## 产品概述

为 Rust 实现的 shell 增加 `>>` / `1>>` / `2>>` 追加重定向支持：与 stdout / stderr 截断重定向架构完全对称，命令输出追加到目标文件而非覆盖；文件不存在时自动创建，已有内容保留。

## 核心功能

- **`>>` / `1>>` 操作符**：把命令的 stdout 追加到目标文件末尾；两者完全等价。
- **`2>>` 操作符（顺带支持）**：把命令的 stderr 追加到目标文件末尾，与 bash 行为一致，避免后续 stage 返工。
- **文件创建语义**：目标文件不存在时自动创建（`O_CREAT`）；存在时**保留原有内容**，新输出从文件末尾追加。
- **与既有重定向共存**：`>` / `1>` / `2>` 截断语义不变；`>>` / `1>>` / `2>>` 追加语义独立；同行可任意混用（如 `> out 2>> err`、`echo a > f; echo b >> f`）。
- **覆盖范围**：builtin（`echo` / `pwd` / `type` / cd 错误等）与外部命令统一适用，路径打开错误时不中断 REPL。

## 验证场景（spec 三条端到端样例）

1. `ls /tmp/baz >> /tmp/bar/bar.md` 连续两次执行 → 文件内容累加，原内容保留。
2. `echo Hello Emily 1>> baz.md` + `echo Hello Maria 1>> baz.md` → 文件含两行，`1>>` 与 `>>` 等价。
3. `echo List of files: > qux.md` 后 `ls /tmp/baz >> qux.md` → 截断 + 追加混用，最终文件含首行 + 后续目录列表。

## 技术栈

- 语言/工具链：Rust（沿用现有 crate，零新增依赖）
- 标准库新增使用：`std::fs::OpenOptions`（追加打开模式）
- 既有：`std::fs::File`、`std::io::{self, Write}`、`std::process::{Command, Stdio}`
- 测试：`cargo test` 内建 unit test 模块；端到端用 `./target/release/codecrafters-shell` 回放 spec

## 实现策略

- **对称扩展，不引入新模式**：完全复用上一 stage「tokenize 切操作符 → parse 归一化字段 → REPL 准备 sink → builtin 写 sink / 外部命令物化 Stdio」四段式架构，把 `>>` / `1>>` / `2>>` 平行加入。**禁止**改动 sink 传递的 `Box<dyn Write>` 抽象、`open_sink` / `open_err_sink` 的返回类型。
- **ParsedCommand 字段最小化扩展**：保留 `stdout_redirect: Option<String>` / `stderr_redirect: Option<String>`，新增两个布尔标志 `stdout_append: bool` / `stderr_append: bool`（默认 `false`）。**关键收益**：现有 ≥10 处 `assert_eq!(p.stdout_redirect, Some(...))` 字段访问形式的测试零回归；避免重构为 `Redirect { path, append }` enum 带来的连锁 match 修改。
- **tokenize 识别 `>>`**：在 `'>' =>` 分支起始 `chars.clone().next()` 做 O(1) peek（既有 InDoubleQuote 态已有同款 peek 模式，可直接复用），若下一字符为 `>` 则 `chars.next()` 消费它，按 `current` 是否为 `"1"` / `"2"` 升级为 `">>"` / `"1>>"` / `"2>>"`；否则走既有 `>` / `1>` / `2>` 路径。引号内 / 转义后的 `>>` 仍按字面量。
- **parse 单次扫描多路径**：`parse()` 循环 `match tok.as_str()`：
- `">"` / `"1>"` → `stdout_redirect = Some(target); stdout_append = false`
- `">>"` / `"1>>"` → `stdout_redirect = Some(target); stdout_append = true`
- `"2>"` → `stderr_redirect = Some(target); stderr_append = false`
- `"2>>"` → `stderr_redirect = Some(target); stderr_append = true`
- 任一操作符后无下一 token → `MissingRedirectTarget`
- 重复 / 混用同向重定向取**最后一次**（如 `> a >> b` 最终 `b` 走 append；`>> a > b` 最终 `b` 走 truncate），与 bash 一致。
- **打开模式参数化**：`open_sink` / `open_err_sink` 签名各扩为 `(target: Option<&str>, append: bool)`：
- `append == true` → `OpenOptions::new().create(true).append(true).open(path)`
- `append == false` → `File::create(path)`（即既有截断行为）
- **外部命令路径去重**：当前外部命令分支两处内联 `File::create(path)` 物化 Stdio，本 stage 抽出 `open_file_for_redirect(path: &str, append: bool) -> io::Result<File>` helper，统一用于 `.stdout(...)` / `.stderr(...)`，并消除既有重复代码（顺手清理，且与 `open_sink` 共享同一打开语义保持一致）。
- **错误处理**：打开失败的错误信息复用既有 `eprintln!("{}: {}: {}", cmd, target, e)` 路径，REPL 不中断。

## 性能与复杂度

- tokenize 仍是 O(n) 单次扫描；新增 peek 是 O(1)；parse 在原有线性遍历上仅多 2 次字符串比较，复杂度不变。
- 文件 fd：每条命令最多额外 2 个 fd（stdout + stderr），命令结束后自动 close，无泄漏。
- `OpenOptions::append(true)` 在 Linux 下设置 `O_APPEND`，由内核保证每次 `write(2)` 原子定位到文件末尾，无需用户态 seek，性能与 truncate 模式无可见差异。

## 实现注意

- **不要**改 sink 抽象 / `Box<dyn Write>` 返回类型——只扩展打开方式；外部命令路径仍直接 `File::from` → `Stdio::from`。
- **不要**改 `ParseError` 枚举或 `Display` 文案——`MissingRedirectTarget` 自然涵盖 `>>` / `1>>` / `2>>` 缺目标场景。
- **不要**重构 `ParsedCommand` 为 enum 风格（如 `enum RedirectMode { Truncate, Append }`）——会引发 ≥10 处既有测试与 REPL 字段访问连锁修改，违反最小入侵原则。
- **不要**让 `>` / `>>` 之间出现空白时还合并——bash 行为是「`>` `>` 紧贴」才识别为 `>>`；本设计天然满足（peek 仅看下一字符）。
- 测试 `>>` append 行为时，在 `cargo test` 单测层面**无须**真正写文件（parser 层只验证 token / ParsedCommand 字段），文件追加语义放到 e2e 验证阶段在 `/tmp` 实测。

## 架构图（数据流，仅展示新增维度）

```mermaid
flowchart LR
  A[stdin line] --> B[tokenize<br/>识别 >> / 1>> / 2>>]
  B --> C[parse<br/>ParsedCommand{<br/>stdout_redirect, stdout_append,<br/>stderr_redirect, stderr_append}]
  C --> D{builtin?}
  D -->|yes| E[open_sink/open_err_sink<br/>append? OpenOptions.append : File::create]
  E --> F[run_echo / run_pwd / run_type<br/>writeln! sink/err_sink]
  D -->|no| G[open_file_for_redirect helper<br/>Command.stdout/.stderr Stdio::from File]
```

## 目录结构

```
codecrafters-shell-rust/
├── src/
│   ├── parser.rs   # [MODIFY] 文件头注释补充 >> 语义；tokenize 在 '>' 分支首部 peek 下一字符识别 >> 并按 current=="1"/"2" 升级为 ">>"/"1>>"/"2>>"；ParsedCommand 新增 stdout_append/stderr_append 布尔字段；parse 循环识别 4 个新操作符 token 并设置 append 标志；测试模块新增 ≥6 个用例覆盖 tokenize（紧贴/空格/引号内/转义/数字前缀合并/拒绝合并的负样例）+ parse（>> 与 > 共存、混用取最后一次、2>> 缺目标报错）
│   └── main.rs     # [MODIFY] 新增 use std::fs::OpenOptions；open_sink/open_err_sink 签名追加 append: bool 参数（true 走 OpenOptions.create(true).append(true).open，false 保持 File::create）；新增 open_file_for_redirect(path, append) -> io::Result<File> helper 用于外部命令 Stdio 物化；REPL 主循环两处 open_* 调用传入 parsed.{stdout,stderr}_append；外部命令分支两处内联 File::create 改用 helper
└── （Cargo.toml / Cargo.lock / your_program.sh 不动）
```

## 关键接口（仅 2 处签名变化 + 1 个新 helper）

```rust
// parser.rs
pub struct ParsedCommand {
    pub argv: Vec<String>,
    pub stdout_redirect: Option<String>,
    pub stdout_append: bool,           // 新增：true 表示 >> / 1>>
    pub stderr_redirect: Option<String>,
    pub stderr_append: bool,           // 新增：true 表示 2>>
}

// main.rs
fn open_sink(stdout_redirect: Option<&str>, append: bool) -> io::Result<Box<dyn Write>>;
fn open_err_sink(stderr_redirect: Option<&str>, append: bool) -> io::Result<Box<dyn Write>>;
fn open_file_for_redirect(path: &str, append: bool) -> io::Result<File>;  // 新增 helper，外部命令分支共享
```