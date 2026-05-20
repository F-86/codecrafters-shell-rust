---
name: register-complete-builtin
overview: 把 `complete` 注册为内建命令，使 `type complete` 输出 `complete is a shell builtin`；本阶段不实现任何补全逻辑。
todos:
  - id: register-complete-builtin
    content: 在 src/builtins.rs 的 BUILTINS 常量数组末尾追加 "complete"
    status: completed
  - id: verify-type-output
    content: 运行 cargo build 并交互验证 type complete 输出 "complete is a shell builtin"
    status: completed
    dependencies:
      - register-complete-builtin
---

## Product Overview

在 CodeCrafters Rust Shell 项目中，将 `complete` 注册为新的 shell 内建命令，使其能被 `type` 命令识别。

## Core Features

- `type complete` 输出 `complete is a shell builtin`
- 不实现任何补全逻辑（后续阶段处理）
- 不影响现有 builtin（`echo` / `exit` / `type` / `pwd` / `cd`）行为
- TAB 补全候选源自动包含 `complete`（追加 BUILTINS 的免费收益）

## 技术栈

- 沿用现有项目：Rust + 标准库，无新增依赖
- 不修改 `Cargo.toml`，不新建文件

## 实现策略

唯一改动点：在 `src/builtins.rs:16` 的 `BUILTINS` 常量数组中追加字符串字面量 `"complete"`。

`BUILTINS` 是项目中所有 builtin 名称的单一事实来源，已被 `run_type`（builtin 判定）与 `completion.rs`（TAB 候选源）两处消费。仅追加该数组即可同时让 `type complete` 命中并自动出现在 TAB 补全候选中——零侵入、零回归风险。

## 关键设计决策

- **不在 main.rs dispatch 中添加 `complete` 分支**：本阶段测试只跑 `type complete`，不会真正执行 `complete` 命令。提前加空分支反而会污染未知命令处理路径，等后续阶段实际实现时再补 runner 更稳妥。
- **不改 `completion.rs`**：现有 `for name in BUILTINS` 循环已自动覆盖新增项，无需任何改动。
- **顺序选择**：追加到数组末尾，符合源文件第 15 行注释指引"后续阶段新增内建时只需在此处追加"。

## 实施细节（Implementation Notes）

- 改动行数：1 行（数组字面量增量）
- 兼容性：完全向后兼容，不影响任何已通过的早期阶段测试
- 验证路径：`cargo build` → 启动 shell → 输入 `type complete` 应输出 `complete is a shell builtin`

## 目录结构

项目结构总览：仅 1 个文件单行变更，无新增/删除文件。

```
codecrafters-shell-rust/
└── src/
    └── builtins.rs   # [MODIFY] 在第 16 行 BUILTINS 数组末尾追加 "complete"。
                      # 不需要为 complete 实现 run_complete runner（本阶段不要求）。
                      # 修改后 run_type（同文件 line 110）的 BUILTINS.contains 判定
                      # 与 completion.rs（line 252）的 TAB 候选枚举自动生效，无需联动改动。
```