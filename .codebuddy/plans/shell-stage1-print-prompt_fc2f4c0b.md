---
name: shell-stage1-print-prompt
overview: 完成 CodeCrafters "Build Your Own Shell" 第一关：在 `src/main.rs` 中启用打印 `$ ` 提示符的代码并刷新 stdout。
todos:
  - id: enable-prompt
    content: 修改 src/main.rs，取消 print!("$ ") 与 stdout flush 两行注释
    status: completed
  - id: verify-build
    content: 运行 ./your_program.sh 验证编译通过并输出 "$ " 提示符
    status: completed
    dependencies:
      - enable-prompt
---

## 用户需求

使用 Rust 实现 CodeCrafters "Build Your Own Shell" 挑战的第一关：让 shell 程序启动时打印一个提示符 `$ `，表明它已就绪、等待用户输入。

## 产品概述

这是一个 POSIX 风格 shell 的起点实现。本阶段不涉及任何命令解析或执行逻辑，仅需在程序启动时向标准输出打印固定的提示符字符串 `$ `（美元符号 + 一个空格，不换行），并立即刷新输出缓冲区，以便 CodeCrafters 测试服务器能在管道中即时读取到该提示符。

## 核心功能

- 程序启动后立即向 stdout 输出 `$ ` 提示符
- 强制 flush stdout，避免行缓冲在非 TTY（管道）环境下吞掉提示符
- 程序结束即可，不需要读取或处理任何输入

## 技术栈

- 语言：Rust（edition = 2024，rust-version = 1.95，沿用 `Cargo.toml` 现有配置）
- 标准库：`std::io::{self, Write}`（模板已 `use`）
- 现有依赖 `anyhow` / `bytes` / `thiserror` 本阶段无需使用，保持原样不动以避免影响后续阶段

## 实现思路

直接沿用 `src/main.rs` 中 CodeCrafters 模板的注释代码，将其取消注释即可：

1. 通过 `print!("$ ")` 输出提示符（注意是 `print!` 而不是 `println!`，提示符末尾不应有换行）
2. 调用 `io::stdout().flush().unwrap()` 立即冲刷缓冲区——这是关键点：Rust 的 stdout 是行缓冲的，在测试服务器以管道方式驱动 shell 时，没有换行就不会自动刷新，会导致测试读不到提示符而超时

## 实现要点

- **最小改动原则**：只取消 `src/main.rs` 中两行注释，不引入新依赖、不重构结构、不新增文件，保持与 CodeCrafters 模板的零偏离
- **零阻塞退出**：本阶段不读 stdin，打印 + flush 后让 `main` 自然返回；测试只检查提示符是否被写出
- **为后续阶段留好扩展空间**：当前 `main.rs` 结构足够支撑下一阶段（Stage 2 引入 REPL 循环 + 命令解析）时再做演进，本次不做超前设计（YAGNI）
- **不要**改用 `println!` 或追加 `\n`：会破坏与后续阶段在同一行读取命令的交互形态，也不符合常见 POSIX shell 提示符习惯
- **不要**改成 `eprint!`：CodeCrafters 测试读取的是 stdout，写到 stderr 会判失败

## 目录结构

仅修改一个文件，无新增：

```
codecrafters-shell-rust/
└── src/
    └── main.rs   # [MODIFY] 取消两行注释：print!("$ "); io::stdout().flush().unwrap();
```