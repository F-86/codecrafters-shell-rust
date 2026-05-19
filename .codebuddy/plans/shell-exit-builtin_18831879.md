---
name: shell-exit-builtin
overview: 在现有 REPL 循环基础上新增 `exit` 内建命令的识别与处理：当用户输入 `exit`（或 `exit <code>`）时，shell 立即终止。
todos:
  - id: impl-exit-builtin
    content: 改造 src/main.rs：在非空输入分支拆分 cmd/args，匹配 "exit" 时解析可选退出码并 process::exit，未匹配保持 command not found
    status: completed
  - id: verify-exit
    content: 本地验证：输入 invalid_command_1 输出未找到，紧接 exit 后 shell 终止退出码 0；额外验证 exit 0 同样终止
    status: completed
    dependencies:
      - impl-exit-builtin
---

## 产品概述

在已有 Rust shell（提示符 + REPL + 未知命令报错）的基础上，新增第一个内建命令 `exit`：当用户输入 `exit` 时，shell 立即终止，不再继续 REPL，也不再输出 `command not found`。

## 核心功能

- **内建命令分发**：把整行输入按空白拆分为 `command` 和 `args`，按命令名进行 match 分发，未命中再走 "command not found" 路径，为后续 `echo`/`type`/`pwd`/`cd` 等内建预留扩展点。
- **exit 内建**：匹配到 `exit` 时立即终止进程，退出码默认 0；同时兼容 `exit <n>`（解析失败回退为 0），为后续阶段的 `exit 0` 测试场景做防御性兼容。
- **保留既有行为**：`$ ` 提示符、空行跳过、EOF 退出、未知命令打印 `{cmd}: command not found` 等逻辑保持不变。

## 技术栈

- Rust（edition = "2024", rust-version = "1.95"）
- 仅使用标准库 `std::io` + `std::process::exit`，不引入新依赖

## 实现思路

在现有 REPL 循环的"读取 -> trim -> 非空"之后，插入一个内建分发步骤：

1. 用 `line.split_whitespace()` 拆分得到迭代器；取首个 token 作为 `cmd`，剩余为 `args`。
2. `match cmd`：

- `"exit"`：解析可选第一个 arg 为 `i32` 退出码（解析失败则用 0），调用 `std::process::exit(code)` 直接结束进程。
- 其他：保持原行为，`println!("{}: command not found", line)`。

3. 整个 main 仍是 `loop { ... }`，仅在 `exit` 分支主动结束进程；EOF 仍走 `break` 自然返回。

## 关键技术决策

- **使用 `process::exit` 而非 `break`**：`exit` 内建语义是"立即终止 shell"，使用 `process::exit(code)` 表达更直接，并能携带退出码；后续若 `exit n` 测试出现，无需再改造控制流。
- **`split_whitespace` 而非 `split(' ')`**：天然处理多个空格、Tab，与 POSIX shell 行为一致，且为后续 `echo`/`type` 复用同一拆分逻辑做准备。
- **`println!` 输出未找到错误**：保持上一阶段一致，错误信息写到 stdout（CodeCrafters 题目要求）。
- **不抽模块**：当前只有一个内建，仍在 main 内联 match；待 builtin 数量达到 3+ 个再抽 `builtins.rs`，避免过早设计。
- **错误码默认 0**：本阶段题目仅 `exit`（无参），但 CodeCrafters 后续阶段常出现 `exit 0`；提前兼容零成本，规避回归风险。

## 实施注意事项

- **拆分对原始行的影响**：仅基于 `line` 拆分用于分发，未知命令仍打印整行 `line`（保持上一阶段输出格式 `invalid_command_1: command not found`）。
- **空 token 保护**：`split_whitespace` 不会产生空 token，但仍需 `if let Some(cmd) = ...` 保护以防极端情况；空行已在前面 `is_empty` 处过滤。
- **flush 保留**：`exit` 直接 `process::exit`，stdout 已写入但不必担心，因为前面 println 自带换行 + 行缓冲在退出时由运行时刷新；为稳妥可在 exit 前 `stdout.flush()`，但本阶段无输出，可省。
- **保持 blast radius 最小**：不动现有提示符、读取、EOF、空行处理逻辑，只在 "非空 line" 后插入 6~10 行分发代码。

## 架构设计

单文件 main 内联 REPL + 内建分发：

```
loop:
  print "$ " + flush
  read_line
    Ok(0) -> break (EOF)
    Ok(_) -> trim
              if empty: continue
              split_whitespace -> (cmd, args)
              match cmd:
                "exit"  -> parse args[0] as i32 (default 0) -> process::exit(code)
                _       -> println!("{line}: command not found")
    Err   -> eprintln + break
```

## 目录结构

```
codecrafters-shell-rust/
└── src/
    └── main.rs   # [MODIFY] 在现有 REPL 中插入内建命令分发：split_whitespace 拆出 cmd/args，匹配到 "exit" 时解析可选退出码并 std::process::exit，未匹配走原 "command not found" 分支。其他逻辑（提示符、flush、EOF、空行、错误处理）保持不变。
```