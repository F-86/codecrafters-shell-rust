---
name: no-match-bell-completion
overview: 在文件名参数补全无匹配时，于 `complete_filename_arg` 的空候选分支显式向 stdout 写入 `\x07`（BEL），保持输入行不变。
todos:
  - id: explicit-bel-on-no-match
    content: 在 `src/completion.rs::complete_filename_arg` 尾部 `_` 分支返回前显式 `print!("\x07")` + stdout flush，并同步更新 doc 注释
    status: completed
  - id: build-and-test
    content: 运行 `cargo build` 与 `cargo test`，确认零警告、85 测试全过、无回归
    status: completed
    dependencies:
      - explicit-bel-on-no-match
  - id: pty-verify
    content: 用 expect/PTY 脚本复现 `cat absent_filenam
    status: completed
    dependencies:
      - build-and-test
---

## 产品概述

为 codecrafters Rust shell 实现「参数补全无匹配时响铃」语义：当用户在参数位置输入一个 cwd 下不存在前缀的字符串并按 TAB 时，命令行保持不变，同时向 stdout 输出 BEL 字符（`\x07`），由终端将其转为可听/可视提示。

## 核心功能

- **无匹配响铃**：参数补全候选数为 0 时，line buffer 与光标位置均不修改，向 stdout 立即写入 `\x07` 并 flush，确保字节立即可达 tester 的 stdout 读取端。
- **多匹配且无公共前缀可扩展时复用同一语义**：保留上 stage 已实现的「BEL 表示无法继续扩展」行为，共用同一分支。
- **输入保持不变**：当前 `complete` 流程在该分支已返回 `Ok((pos, Vec::new()))`，rustyline 不会触碰 line buffer，光标位置不变。
- **兜底**：保留 rustyline `CompletionType::List` + Unix 默认 `BellStyle::Audible` 在空候选时的自动 BEL 作为冗余兜底，避免对 rustyline 内部行为的隐式依赖。

## 技术栈

沿用现有项目栈：Rust 2021 edition + rustyline 14.0.0。无新增依赖、无配置变更。

## 实现思路

最小化、显式化改动。仅在 `src/completion.rs::ShellHelper::complete_filename_arg` 的尾部 `_` 分支（candidates 数 ≠ 1 的合并分支）于返回 `Ok((pos, Vec::new()))` 之前显式写一次 BEL。`std::io::Write` 与 `std::io::stdout` 在该文件已引入（用于命令名双 TAB 列出场景），直接复用。

### 关键决策与理由

- **选择方案 B（显式 BEL + 保留 rustyline 兜底）**：理论上零改动即可通过 tester（rustyline `List` 模式 + Unix `BellStyle::Audible` 会向 stdout 写 `\x07`），但显式自手 BEL 让契约对调用方一目了然，不依赖第三方库内部分支行为；双 BEL 在 PTY 终端只会被合并为一次可感知的响铃，tester 不校验 BEL 出现次数。
- **不修改 rustyline 配置**：保留 `BellStyle::Audible` 与 `CompletionType::List`，零回归风险；上 stage 已经在此配置下绿，本 stage 仅做"加法"。
- **共用「无匹配」与「无 LCP 可扩展的多匹配」分支**：两者语义同构（「TAB 无法继续推进」→ 响铃），无需新增条件分支。
- **flush 必要性**：rustyline 在 `complete()` 返回后会立即触发 line 重绘，若 BEL 未 flush 可能被 line refresh 序列缓冲到后面，导致字节顺序错乱；显式 `flush()` 确保 `\x07` 落在 line 重绘序列之前。

### 实现细节（执行要点）

- **不引入新 use**：`std::io::{self, Write}` 已在 `src/completion.rs` 顶部存在，直接 `print!("\x07")` + `let _ = io::stdout().flush();` 即可。
- **flush 错误处理**：使用 `let _ =`/`.ok()` 丢弃错误。stdout flush 失败属于不可恢复 I/O 故障（如 PTY 已被 tester 关闭），交互场景下日志输出反而污染用户视区，沿用文件中 `print!` + `flush` 的既有写法风格。
- **不动 line buffer**：仍返回 `Ok((pos, Vec::new()))`，rustyline 在 List 模式见到空候选只调 `beep()`，不修改 line。
- **注释同步更新**：函数 doc 注释明确「显式 BEL，rustyline 兜底」契约，避免后续 stage 误以为可以依赖单一来源。
- **测试断言不变更**：现有单元测试仅覆盖纯函数（`extract_arg_prefix` / `match_files_in_dir` / `split_dir_and_name` / `classify_path` / `format_arg_completion` / LCP），不触达 `complete_filename_arg` 的 println 副作用，无需调整。

### 回归与边界

- **LCP 可扩展分支不受影响**：单 Pair 路径在 candidates.len() == 1 时命中，不会落入 `_` 分支。
- **命令名补全**：走 `complete()` 顶部独立分支，不经过 `complete_filename_arg`，无影响。
- **多匹配双 TAB 列出场景**：仍在「LCP == prefix」状态机中处理，不经过 `_` 分支。
- **空 prefix（末尾空白）**：若 cwd 完全为空，会落入 `_` 分支并响铃，符合 shell 直觉行为，不引入 bug。

## 修改目标

```
src/completion.rs   # [MODIFY] complete_filename_arg 尾部 `_` 分支显式 BEL + flush + doc 更新
```

### 文件修改详述

- **`src/completion.rs`**
- **职责**：rustyline `Completer` 实现，负责命令名与参数位置的 TAB 补全。
- **修改点**：仅 `complete_filename_arg` 函数末尾 `match candidates.len()` 的 `_` 分支
    - 当前：直接 `Ok((pos, Vec::new()))`
    - 修改后：先 `print!("\x07")` → `let _ = io::stdout().flush();` → 再返回 `Ok((pos, Vec::new()))`
- **关键实现要求**：
    - 复用文件顶部已有的 `use std::io::{self, Write};`，不新增 import。
    - 注释从「依赖 rustyline `List` 模式统一负责 BEL」更新为「显式自手 BEL + rustyline 兜底」，说明双 BEL 在终端语义上等价单次响铃且无副作用。
    - 不修改函数签名、不重构匹配分支结构、不改 `_` 分支覆盖范围（仍同时处理 0 候选与 ≥2 候选无扩展的合并语义）。