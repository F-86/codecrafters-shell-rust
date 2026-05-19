---
name: shell-invalid-command-error
overview: "在现有 Rust shell 基础上，实现读取用户输入并对任意命令打印 `{command}: command not found` 错误信息的功能。"
todos:
  - id: impl-repl-and-not-found
    content: "改造 src/main.rs：实现 REPL 循环、读取输入、trim 处理、空行跳过，并按 \"{cmd}: command not found\" 格式输出错误，EOF 时退出"
    status: completed
  - id: verify-locally
    content: "本地通过 ./your_program.sh 验证：输入 xyz 后输出严格匹配 \"xyz: command not found\"，提示符 \"$ \" 正常显示"
    status: completed
    dependencies:
      - impl-repl-and-not-found
---

## 产品概述

基于 Rust 实现 POSIX 兼容 shell 的 CodeCrafters 挑战项目。当前阶段需要在已有的 `$ ` 提示符基础上，新增"读取用户输入并对未知命令打印错误信息"的能力，为后续 REPL 循环、内建命令、外部命令执行打下基础。

## 核心功能

- **显示提示符**：在每次等待输入前，向 stdout 输出 `$ `（注意 ` 后有一个空格），并立即 flush 确保提示符在阻塞读取前可见。
- **读取一行输入**：从 stdin 按行读取用户输入，去除末尾换行符。
- **打印未找到错误**：将所有命令（即输入行的首个 token，本阶段也可视为整行）按 `{command}: command not found` 的格式严格输出到 stdout，冒号后保留一个空格。
- **REPL 雏形**：以循环结构组织"提示 -> 读 -> 处理"的流程，便于后续阶段扩展（如内建命令、外部程序执行），同时在 EOF（Ctrl-D）时正常退出。

## 验收要点

- 输出格式严格匹配：`xyz` -> `xyz: command not found`
- 保留上一阶段的 `$ ` 提示符行为
- 处理输入末尾换行符，避免输出多余空白或换行错位

## 技术栈

- 语言：Rust（edition = "2024", rust-version = "1.95"）
- 依赖：仅使用标准库 `std::io`（`Cargo.toml` 中已有 `anyhow`/`bytes`/`thiserror` 本阶段不引入，避免过度设计）
- 构建/运行：通过项目自带的 `your_program.sh` 编译并执行

## 实现思路

采用最小可演进的 REPL 循环：

1. 在循环顶部使用 `print!("$ ")` + `io::stdout().flush()` 确保提示符立刻可见（`print!` 是行缓冲，必须显式 flush，否则在阻塞读取前看不到提示符）。
2. 使用 `io::stdin().read_line(&mut input)` 读取一行；返回值为 `Ok(0)` 表示 EOF，跳出循环正常结束进程。
3. 用 `trim_end_matches(&['\n', '\r'][..])`（或 `trim()`）去除行尾换行；若整行为空则跳过本次循环（继续打印下一个提示符），避免输出 `: command not found`。
4. 本阶段将整行作为命令名打印：`println!("{}: command not found", line)`。后续阶段可在此处替换为 token 解析 + 命令分发逻辑。

## 关键技术决策

- **使用 REPL 循环而非单次读取**：CodeCrafters 后续阶段几乎都依赖 REPL 行为；现在引入循环结构，下一阶段可零成本扩展，且不会让本阶段测试失败（测试通常发送一行输入后关闭 stdin，循环遇到 EOF 自然退出）。
- **使用标准库而非第三方库**：避免引入 `rustyline` 等依赖增加构建时间，标准库 `read_line` 完全满足当前需求。
- **trim 处理**：`read_line` 会保留末尾 `\n`，必须裁剪，否则错误信息会变成 `xyz\n: command not found`。
- **空输入保护**：用户直接回车时不应输出 `: command not found`，应继续显示下一个提示符。

## 实施注意事项

- **flush 时机**：每次打印提示符后必须 `flush`，否则在管道/非 tty 环境下 grader 可能读不到提示符。
- **错误处理**：`read_line` 失败时使用 `expect`/`unwrap` 即可，本阶段无需复杂错误传播；后续接入 `anyhow` 时再统一改造。
- **输出通道**：错误信息（`command not found`）输出到 **stdout**（CodeCrafters 题目要求），不要写到 stderr。
- **保持向后兼容**：保留 `#[allow(unused_imports)]` 风格不必强求，但 `use std::io::{self, Write}` 的 `Write` 必须保留以支持 `flush`。
- **避免过度设计**：不要现在就拆分模块（`parser.rs`、`executor.rs` 等），CodeCrafters 风格更适合在后续阶段按需拆分。

## 架构设计

单文件结构，main 函数承载 REPL 循环：

```
main()
  loop:
    print prompt "$ " -> flush
    read_line(input)
       |- Ok(0)  -> break (EOF)
       |- Ok(_)  -> trim -> if empty continue; else println!("{cmd}: command not found")
       |- Err(e) -> eprintln + break
```

## 目录结构

```
codecrafters-shell-rust/
└── src/
    └── main.rs   # [MODIFY] 在现有打印提示符基础上，增加 REPL 循环：读取一行输入、trim 处理、空行跳过、按 "{cmd}: command not found" 格式输出，遇 EOF 退出。保留 use std::io::{self, Write}，每次提示符后 flush stdout。
```