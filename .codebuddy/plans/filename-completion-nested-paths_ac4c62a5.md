---
name: filename-completion-nested-paths
overview: 扩展参数位置文件名补全以支持嵌套路径：当 token 含 `/` 时，按最后一个 `/` 切分目录与文件名前缀，扫描该目录寻找匹配 entry，单匹配则把整 token 替换为 `<dir>/<entry> `（含尾空格）。复用现有 dispatch 与字面对齐校验，仅泛化目录扫描函数。
todos:
  - id: add-split-helper
    content: 在 src/completion.rs 新增 use std::path::Path 与 split_dir_and_name 纯函数（rfind('/') + split_at），并补 6 个边界单测
    status: completed
  - id: generalize-match-files
    content: "将 match_files_in_cwd 重命名+泛化为 match_files_in_dir(dir: &Path, name_prefix)，仅返回叶子名；同步迁移已有 3 个单测到新签名"
    status: completed
    dependencies:
      - add-split-helper
  - id: wire-nested-completion
    content: 修改 complete_filename_arg 步骤 3、4：调用 split_dir_and_name 切分，按 dir_part 选 scan_dir 调用 match_files_in_dir，单匹配时拼回 dir_part 形成完整路径加尾空格
    status: completed
    dependencies:
      - generalize-match-files
  - id: add-nested-tests
    content: 新增 match_files_in_dir 嵌套目录集成测试（src/ 下找 completion.rs、不存在目录返回空），并执行 cargo build / cargo test 验证零回归
    status: completed
    dependencies:
      - wire-nested-completion
---

## 用户需求

扩展 shell 的文件名 TAB 补全，支持嵌套路径。当用户键入的参数 token 含 `/` 时，按最后一个 `/` 切分为「目录部分」（含尾 `/`）与「叶子前缀」，扫描目录部分查找以叶子前缀开头的条目；若仅有一个匹配，将整个 token 替换为完整路径并追加尾空格。

## 核心特性

- token 含 `/`：以 `rfind('/')` 切分；`fs::read_dir(dir_part)` 扫描该目录。
- 目录路径相对 cwd 由 OS 解析；绝对路径（如 `/etc/h`）天然支持。
- 单匹配：替换为 `<dir_part><entry> `（含尾空格）。
- 0 / 多匹配：BEL 响铃 no-op（与上 stage 一致）。
- 目录不存在 / 非目录 / 不可读：read_dir 静默返回空 → 走 0 候选 BEL。
- 隐藏文件继续按字面前缀匹配。
- 不含 `/` 的旧路径走 `Path::new(".")`，零行为变化。

## 技术栈

- 沿用现有 Rust + rustyline + std::fs 栈，零新依赖。
- 复用 `parser::tokenize` 提取 token，`Path::new` 让 OS 解析路径语义。

## 实现方案

**总体策略**：纯增量改造 `src/completion.rs::complete_filename_arg`，仅修改其「步骤 3 扫描候选」与「步骤 4 拼回 replacement」两段。前缀提取（`extract_arg_prefix`）、字面对齐校验、`start = pos - prefix.len()` 起点计算、dispatch 逻辑全部不动 —— 这保证嵌套路径补全与 cwd 补全共享同一替换协议，零回归风险。

**关键决策**：

1. 把 `match_files_in_cwd(prefix)` 泛化为 `match_files_in_dir(dir: &Path, name_prefix: &str)`，返回**仅叶子名**。dir_part 拼回职责由调用方承担，函数本身不感知"路径前缀"概念，单测更纯。
2. 新增 `split_dir_and_name(token) -> (&str, &str)` 纯函数：`rfind('/')` + `split_at(idx + 1)`，使 `dir_part` 始终含尾 `/`（含 `/` 时）或为空字符串（不含 `/` 时）。返回 `&str` 切片避免分配。
3. 拼回 replacement 时直接 `format!("{}{} ", dir_part, entry)`，dir_part 为空时退化为 cwd 场景（与上 stage 字面一致）。
4. `start` 仍为 `pos - prefix.len()`（prefix = 整 token）：rustyline 把 `[start, pos)` 整段替换为 `<dir_part><entry> `，与现有字面对齐校验天然兼容，无需改 dispatch。

**性能与可靠性**：

- 切分 O(L)，扫描 O(N)（N 为目标目录条目数），TAB 是低频交互，不做缓存。
- read_dir 失败统一静默返回空 Vec（不存在 / 权限 / 非目录 / 损坏）—— 与现有 cwd 不可读路径行为对齐，避免在交互区写错误日志污染输入。
- `'/'` 是 ASCII 字节，`rfind('/')` + `split_at` 在 UTF-8 串上字节安全。

**避免技术债**：

- 不引入新模块；所有改动局限于 `src/completion.rs`。
- 不动 `extract_arg_prefix` / 双 TAB 状态机 / 命令名补全分支。
- 已有 3 个 `match_files_in_cwd` 单测改为调用新签名（`Path::new(".")`），断言不变，覆盖度保留。

## Implementation Notes

- **复用现有模式**：拼回逻辑沿用现有 `Pair { display, replacement }` 协议；BEL 复用 `print!("\x07")` + `flush()` 现有写法。
- **边界**：token 为 `path/`（name_prefix 空）时，`starts_with("")` 永真 → 多匹配 BEL（题目本 stage 不测，合理 fallback）；token 为 `/`（dir_part = `/`、name_prefix 空）同上；token 为 `./f` 时 dir_part = `./`，OS 等价于 `.`，自然走通。
- **零回归保护**：dispatch 不变；token 不含 `/` 时 dir_part = ""、scan_dir = `.`，与现有行为字面等价。

## 架构设计

仅触及 `src/completion.rs`，分层不变：

```
Completer::complete (dispatch 不动)
  └── ShellHelper::complete_filename_arg (主逻辑)
        ├── extract_arg_prefix      [不动]
        ├── 字面对齐校验             [不动]
        ├── split_dir_and_name      [新增] - 切分 token
        ├── match_files_in_dir      [改名+泛化] - 扫描候选
        └── 拼回 dir_part + entry + 尾空格 [修改]
```

## 目录结构

```
src/
└── completion.rs   # [MODIFY] 新增 split_dir_and_name；
                    #          重命名+泛化 match_files_in_cwd → match_files_in_dir(dir, name_prefix)，仅返回叶子名；
                    #          修改 complete_filename_arg 步骤 3、4：按 '/' 切分调用 match_files_in_dir，单匹配时拼回 dir_part；
                    #          新增 use std::path::Path；
                    #          已有 3 个 match_files_in_cwd 测试改用新签名 Path::new(".")，断言不变；
                    #          新增 split_dir_and_name 单测（6 例：空、单字符、含路径、目录尾 /、绝对路径、多层）；
                    #          新增 match_files_in_dir 嵌套目录测试（src/ 下找 completion.rs；不存在目录返回空）。
```

## 关键代码结构

```rust
fn split_dir_and_name(token: &str) -> (&str, &str) {
    match token.rfind('/') {
        Some(idx) => token.split_at(idx + 1), // dir_part 含尾 '/'
        None => ("", token),
    }
}

fn match_files_in_dir(dir: &std::path::Path, name_prefix: &str) -> Vec<String>;
```