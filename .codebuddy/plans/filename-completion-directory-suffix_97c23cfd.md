---
name: filename-completion-directory-suffix
overview: 为参数位置文件名补全增加目录识别：单匹配时若 entry 是目录，replacement 加尾 `/` 而非空格；文件维持加空格。引入 `MatchKind` 枚举与两个纯函数（`classify_path` / `format_arg_completion`）以提升可读性与后续可扩展性，符合 Rust API guidelines。
todos:
  - id: add-match-kind-and-helpers
    content: 在 src/completion.rs 紧邻 match_files_in_dir 之后新增 MatchKind 枚举、classify_path、format_arg_completion 三个纯函数，并补 doc 注释
    status: completed
  - id: wire-dir-completion
    content: 改造 complete_filename_arg 单匹配分支：调用 classify_path + format_arg_completion 生成 Pair，并在模块顶部 doc 补一句目录补全语义
    status: completed
    dependencies:
      - add-match-kind-and-helpers
  - id: add-tests-and-verify
    content: 新增 classify_path 3 例与 format_arg_completion 4 例单测，运行 cargo build / cargo test 验证零警告与全测试通过
    status: completed
    dependencies:
      - wire-dir-completion
---

## Product Overview

为 Rust shell 的 TAB 补全增加目录类型识别：单匹配 entry 是目录时，把替换文本结尾的空格换成 `/`，便于用户立即再次按 TAB 进入下一层；文件保持加尾空格语义不变。

## Core Features

- 参数位置单匹配补全时，根据 entry 类型分流：
- **目录** → 替换为 `<full>/`，无尾空格，光标停在 `/` 后。
- **文件** → 替换为 `<full> `（保持上 stage 语义）。
- 目录判定跟随 symlink（与 bash/zsh/fish 真实 shell 行为一致）。
- 嵌套链式 TAB 自然走通：`ls pig/<TAB>` → `ls pig/dog/`，复用现有切分 + 单匹配分支，无需改 dispatch。
- I/O 失败（race / 权限）安全退化为文件分支（加空格，不会破坏后续输入）。

## Tech Stack

- Rust（沿用项目现有栈），`std::fs::metadata` + `std::path::Path`（imports 已具备，无需新增）。
- rustyline `Completer::Pair`（沿用现有 dispatch 与单匹配分支返回结构）。

## Implementation Approach

**策略**：在 `complete_filename_arg` 的「单匹配分支」（src/completion.rs 行 124-134）插入一次轻量 stat，根据结果选择尾随字符。改动面控制在 5 行业务逻辑替换 + 2 个新增纯辅助函数 + 1 个枚举类型。

**关键决策**：

1. **判定时机选择"单匹配后单次 stat"而非"在 `match_files_in_dir` 阶段批量 stat"**：单匹配分支只走一次 `fs::metadata`，0 / ≥2 候选场景不浪费 syscall；保持 `match_files_in_dir` 签名稳定，不污染上 stage 已通过的 5 个 match_files 测试。
2. **用 `MatchKind` 枚举替代 `bool` 标志**：符合 Rust API guidelines（"prefer enums to bool flags when call sites would benefit from clarity"），调用点 `format_arg_completion(&full, MatchKind::Directory)` 自解释；未来扩展 `Symlink` / `Executable` / `Hidden` 等类型时签名稳定。
3. **`fs::metadata` 跟随 symlink 而非 `symlink_metadata`**：贴近真实 shell——bash/zsh/fish 把指向目录的 symlink 当目录补全，按目标类型加尾 `/`。
4. **I/O 失败退化为 `File`**：加尾空格是语义安全的退化（用户至多多打一个空格），优于"目录被识别为文件后无法继续 TAB"的反向 bug。
5. **`display` 与 `replacement` 视觉一致**：目录 Pair 的 `display` 也带 `/`，与 bash `ls -F` / `complete` 的展示风格对齐，未来多匹配列表渲染时直接复用即可。

**性能/复杂度**：单 TAB 至多新增 1 次 `stat()` syscall（μs 级），低频交互路径，无需缓存。

## Implementation Notes

- **复用现有模式**：新增的 `classify_path` / `format_arg_completion` 与已有 `split_dir_and_name` / `match_files_in_dir` 同属"参数位置补全的纯辅助函数"，紧邻其后定义，保持文件内"自顶向下：trait impl → 辅助函数 → 测试"的既定结构。
- **不引入新依赖**：`std::fs` 与 `std::path::Path` 已在 imports（行 42-44）中。
- **不触碰命令名补全 / 双 TAB 状态机 / dispatch / `extract_arg_prefix` / `match_files_in_dir` / `split_dir_and_name`**：blast radius 严格限定在单匹配分支内；上 stage 78 个测试（含 11 个补全相关）零回归。
- **错误处理**：`fs::metadata` 返回 `Result`，按 `match` 模式只关注 `Ok(m) if m.is_dir()`，其余皆为 `File`。**不打日志**：补全是交互路径，stderr 写入会污染 readline 输入区。
- **文档注释**：模块顶部 doc 行 1-38 的"行为速查"补一句「单匹配为目录时尾 `/` 替代尾空格」，让阅读者一眼掌握新引入的语义。
- **Rust 规范**：`MatchKind` 派生 `Debug, Clone, Copy, PartialEq, Eq`（无字段轻量枚举的标配派生集，clippy 推荐）；私有项用 `///` doc 注释保持一致风格。

## Architecture Design

### 数据流（仅展示新增链路）

```
[单匹配分支]
  full = format!("{dir_part}{entry}")
        │
        ▼
  classify_path(Path::new(&full))   // fs::metadata，跟随 symlink，错误退化为 File
        │
        ▼ MatchKind
  format_arg_completion(&full, kind) // 按 kind 分支生成 Pair
        │
        ▼ Pair
  Ok((start, vec![pair]))            // start 与 dispatch 路径完全不变
```

### 模块职责

- `completion.rs`（既有，本 stage 唯一改动文件）：
- 命令名补全（dispatch + 双 TAB 状态机）：**不动**。
- 参数位置补全 `complete_filename_arg`：仅单匹配分支改 5 行。
- 纯辅助函数区：新增 `MatchKind` 枚举 + `classify_path` + `format_arg_completion` 共 ~25 行。

## Directory Structure

```
src/
└── completion.rs   # [MODIFY] 唯一改动文件
                    #   - 文件顶部 doc 行 1-38：补一句目录补全语义；
                    #   - 新增 enum MatchKind { File, Directory } 派生 Debug/Clone/Copy/PartialEq/Eq；
                    #   - 新增 fn classify_path(path: &Path) -> MatchKind：fs::metadata + is_dir，错误退化 File；
                    #   - 新增 fn format_arg_completion(full: &str, kind: MatchKind) -> Pair：按 kind 生成 display/replacement，目录 "{full}/"，文件 "{full} "；
                    #   - 修改 complete_filename_arg 单匹配分支（行 124-134）：调用 classify_path + format_arg_completion；
                    #   - 测试模块新增：classify_path 3 例（src 目录 → Directory、Cargo.toml → File、不存在路径 → File 退化）；format_arg_completion 4 例（File 平坦 / Directory 平坦 / File 嵌套路径 / Directory 嵌套路径）。
```

## Key Code Structures

```rust
/// 单匹配 entry 的类型分类，预留扩展（Symlink / Executable 等）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchKind { File, Directory }

fn classify_path(path: &Path) -> MatchKind;
fn format_arg_completion(full: &str, kind: MatchKind) -> Pair;
```