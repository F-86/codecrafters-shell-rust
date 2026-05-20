---
name: shell-tab-completion-path-executables
overview: 在 builtin 补全基础上扩展 PATH 可执行文件补全：启动时一次性扫描 PATH 下所有可执行文件名并缓存，TAB 时与 builtin 候选合并去重（builtin 优先），保持「首词触发 + 末尾追加空格」语义不变。
todos:
  - id: add-path-scanner
    content: 在 src/builtins.rs 新增 list_path_executables 函数，按 find_in_path 同标准扫描 PATH 收集可执行文件名，错误静默跳过
    status: completed
  - id: cache-and-merge-completion
    content: 改造 src/completion.rs：ShellHelper 增加 path_executables 缓存字段，complete 中合并 builtin 与 PATH 候选并以 HashSet 去重（builtin 优先）
    status: completed
    dependencies:
      - add-path-scanner
  - id: verify-build-and-e2e
    content: cargo build 验证无警告，用 expect 端到端测试 custom_executable 前缀补全 + 回归 ech/exi builtin 补全
    status: completed
    dependencies:
      - cache-and-merge-completion
---

## 用户需求

扩展 shell 的 TAB 自动补全：在已有的 builtin 候选基础上，新增对 `PATH` 环境变量中**所有可执行文件**的前缀匹配补全。补全成功后命令名末尾追加空格，便于继续输入参数。

## 核心功能

- 启动时扫描 `PATH` 一次性收集所有可执行文件名缓存到 helper
- TAB 触发时，仅在「首词」位置进行候选汇总：先 builtin，后 PATH，**同名以 builtin 优先**
- 候选的 replacement 末尾保留空格，与上 stage 行为一致
- PATH 中不存在 / 不可读 / 非目录的条目静默跳过，REPL 不中断

## 技术栈

保持现有项目栈不变：Rust edition 2024 + `rustyline = "14"`。无需新增第三方依赖。

## 实现方案

**整体策略**：以最小侵入扩展 `ShellHelper` 的候选源。新增「启动期扫描 PATH 并缓存可执行文件名列表」的辅助函数，`Completer::complete` 在原 builtin 候选之后追加 PATH 候选，并用 `HashSet<&str>` 跟踪已加入的名字实现「builtin 优先」的去重语义。

**关键决策与权衡**：

1. **启动期扫描 + 缓存**（用户决策 q3）：PATH 扫描代价主要来自系统 syscall（`read_dir` + per-entry `metadata`）。把成本一次性付清，单次 TAB 补全的复杂度从 O(PATH_files) 降到 O(cached_count)，实测毫秒级以内。代价是无法感知运行期新增可执行 / PATH 变化——本 stage 测试在启动后只做一次 TAB，缓存策略足够；后续若需要可改为带 mtime 失效的懒缓存。
2. **可执行性判定复用 `find_in_path` 同标准**（用户决策 q2）：`is_file()` + `mode() & 0o111 != 0`，单一事实来源，避免补全候选与实际执行结果不一致。
3. **去重语义为 builtin 优先**（用户决策 q1）：用 `HashSet<&str>` 在生成候选时跟踪已加入名字。第一阶段先收 builtin（同时记录到 seen），第二阶段遍历 path 缓存仅在 seen 不含时纳入；PATH 内部同名（不同目录）也只保留首个，与 `find_in_path` 按 PATH 顺序解析的行为对齐。
4. **错误处理**：`PATH` 缺失 → 缓存为空 vec；目录 `read_dir` 失败 → 跳过；entry `metadata` 失败 → 跳过。任何错误都不让构造或补全失败，与 bash 行为一致。

**性能要点**：

- 启动期 O(N) 扫描，N = PATH 下所有 entry 总数；典型 Linux 桌面 N ≈ 数千，扫描 < 50ms。
- 单次 TAB 补全 O(B + P)（B=builtin 数 ≈5，P=缓存大小），HashSet 查询 O(1)。
- 缓存使用 `Vec<String>`（保留扫描顺序，便于 PATH 内部去重以 PATH 顺序为准）。

## 实施细节

- **复用现有约定**：新函数与 `find_in_path` 同放 `src/builtins.rs`，保持 PATH 处理逻辑的单一模块归属；中文 doc 注释风格、边界处理风格与 `find_in_path` 保持一致。
- **不引入新模块**：`completion.rs` 现有结构足以承载缓存字段，无需拆分。
- **向后兼容**：`ShellHelper::new()` 签名不变（无参），调用点 `main.rs` 零改动；`Completer::complete` 行为对原 builtin 用例完全保持。
- **避免日志噪声**：扫描期错误（坏目录等）静默跳过，不向 stderr 写任何信息——TAB 补全是高频热路径，污染终端会干扰交互。

## 架构与模块关系

```
main.rs          (REPL，零改动)
   └─ ShellHelper::new()
         └─ list_path_executables()  ← 启动期扫描，新增
                └─ env::split_paths(PATH)
                     └─ read_dir → metadata 过滤
   └─ editor.readline (TAB)
         └─ ShellHelper::complete()  ← 改造：builtin 候选 + PATH 候选 + 去重
```

## 目录结构

```
src/
├── builtins.rs       # [MODIFY] 新增 list_path_executables() -> Vec<String>。
│                     #   遍历 PATH 各目录、对每个 entry 应用与 find_in_path 一致的
│                     #   可执行性判定（is_file + 0o111），收集 file_name 为 String。
│                     #   PATH 缺失 / 目录读取失败 / entry metadata 失败均静默跳过。
│                     #   保持中文 doc 注释 + 与 bash 行为一致性说明的现有风格。
├── completion.rs     # [MODIFY] ShellHelper 增加 path_executables: Vec<String> 字段；
│                     #   new() 调用 list_path_executables() 填充缓存。
│                     #   complete() 改造：首词限定保留；先收 BUILTINS 前缀匹配候选并
│                     #   将名字记入 HashSet<&str> seen；再遍历 path_executables，仅当
│                     #   name 以 prefix 开头且不在 seen 中时纳入；replacement 仍为
│                     #   format!("{} ", name)，替换起点 0。
└── main.rs           # 无修改（ShellHelper::new() 调用点天然位于启动期）
```

## 关键接口

```rust
// src/builtins.rs
/// 启动期一次性扫描 PATH，返回所有可执行文件 basename 的有序列表
/// （按 PATH 顺序、目录内 read_dir 顺序；不去重，去重责任在调用方）。
pub fn list_path_executables() -> Vec<String>;

// src/completion.rs
pub struct ShellHelper {
    path_executables: Vec<String>,  // 启动期缓存
}
```