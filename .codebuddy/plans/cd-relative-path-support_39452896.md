---
name: cd-relative-path-support
overview: 当前 `cd` 分支调用 `std::env::set_current_dir(target)` 直接透传给 `chdir(2)`，相对路径（`./`、`../`、`dirname`）由内核基于当前 cwd 解析，本阶段功能已天然支持；只需更新分支头部注释（移除"仅处理绝对路径"的过时说明）并通过端到端测试验证行为。
todos:
  - id: refresh-cd-comment
    content: 更新 src/main.rs 中 cd 分支注释，明确已支持绝对+相对路径，执行逻辑不变
    status: completed
  - id: verify-relative-paths
    content: 运行 cargo build 与 ./your_program.sh，覆盖 ./dir/sub、../../、裸目录名、无效相对路径四类场景验证
    status: completed
    dependencies:
      - refresh-cd-comment
---

## 产品概述

扩展现有 Rust shell 的 `cd` 内建命令，使其在已支持绝对路径的基础上正确处理**相对路径**。

## 核心功能

- `cd ./local/bin`：进入当前目录下的 `local/bin` 子目录
- `cd ../../`：相对当前目录向上回退多层
- `cd local`：与 `./local` 等价，进入当前目录下的 `local` 子目录
- 切换成功无输出；切换失败输出 `cd: <directory>: No such file or directory`，且当前工作目录保持不变
- 错误输出中保留用户输入的原始路径串（如 `./does_not_exist`），不做归一化

## 技术栈

- 语言：Rust（edition 2024，rust-version 1.95），沿用现有项目栈
- 标准库 API：`std::env::set_current_dir(path)`（封装 `chdir(2)`）

## 实现思路

### 关键洞察（基于已有代码核查）

上一阶段在 `src/main.rs` 第 88-98 行的 `cd` 分支：

```rust
if let Some(target) = parts.next() {
    if std::env::set_current_dir(target).is_err() {
        println!("cd: {}: No such file or directory", target);
    }
}
```

`std::env::set_current_dir` 内部即 `chdir(2)` 系统调用，**对相对路径原生支持**：内核会基于进程当前 cwd 自动解析 `./`、`../`、子目录名等所有相对路径形式，且失败时进程 cwd 保持不变。当时实现并未加 `starts_with('/')` 限制（该决策已在上一阶段 plan 中明确——"让 `set_current_dir` 自然处理"），因此**当前实现已满足本阶段的全部行为要求**。

### 本阶段所需变更

仅需刷新 `cd` 分支注释，使其反映"绝对路径与相对路径均已支持"的现状，避免后续阅读者被旧注释误导；不新增任何执行逻辑、不引入分支判断，零行为改动。

### 关键决策

- **不显式处理 `./` / `../`**：`chdir(2)` 已正确处理，自行字符串解析（如 `Path::components` 拼接）反而会引入符号链接语义差异（bash 的 `-L` 模式才会进行字符串归一化），与"切换至真实目录"的最小行为冲突。YAGNI。
- **不归一化错误信息中的路径**：测试样例与 bash 一致——错误中回显用户输入原文，不替换为绝对路径。
- **`~` 仍留给后续阶段**：本阶段题面明确仅处理相对路径，`~` 不在范围内，避免越界改动。

### 性能与影响

- 单次 `chdir(2)` 系统调用，O(1)
- 仅注释变更，无语义/二进制行为变化，零回归风险

## 实现注意事项

- 复用现有 `cd` 分支结构，不新增辅助函数
- 中文注释风格与现有代码保持一致
- 验证策略：通过 `printf` 喂入命令序列驱动 REPL，覆盖 `./dir/sub`、`../../`、裸目录名（`local`）、无效相对路径四类场景，分别校验切换后 `pwd` 输出与失败时 cwd 不变

## 目录结构

```
codecrafters-shell-rust/
└── src/
    └── main.rs   # [MODIFY] 仅更新 cd 分支（约第 88-98 行）的中文注释：去掉"仅处理绝对路径"的限定描述，明确说明 set_current_dir 已天然支持相对路径（./、../、子目录名），失败语义由 chdir(2) 保证 cwd 不变；执行逻辑保持原样。
```

## Agent Extensions

无需使用扩展。本阶段为现有项目的局部注释更新与验证，标准代码工具足以覆盖。