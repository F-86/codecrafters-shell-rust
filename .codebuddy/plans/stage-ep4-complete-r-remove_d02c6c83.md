---
name: stage-ep4-complete-r-remove
overview: 在 `run_complete` 中新增 `-r <cmd>` 分支：从跨命令存活的 registry 中移除该命令的补全规则，无输出；命令未注册时也静默 Ok。删除后 `-p <cmd>` 自动回退到"no completion specification"，TAB 补全自动回退到默认响铃路径——读端零修改。
todos:
  - id: add-r-branch
    content: 在 `run_complete` 的 match 中插入 `Some("-r")` 分支，调用 `registry.remove`，无输出
    status: completed
  - id: sync-doc
    content: 同步 `run_complete` 顶部 doc 注释，新增 `-r
    status: completed
    dependencies:
      - add-r-branch
---

## 用户需求

为 `complete` 内建命令补充 `-r <cmd>` 子命令，从注册表中删除指定命令对应的补全脚本。删除后 `complete -p <cmd>` 应回落到「未注册」错误（沿现有 err_sink 路径），同时该命令的 TAB 补全行为退化为默认（响铃）。`-r` 自身无任何标准输出 / 标准错误输出，包括对未注册命令的删除请求也保持静默成功。

## 核心特性

- 解析 `complete -r <cmd>`，将 `<cmd>` 从跨命令存活的补全注册表中删除；无输出。
- 对未注册命令的 `-r` 调用按静默成功处理，不报错、不写 sink。
- 缺失第二参的 `-r` 与现有 `-p` 同构：静默 `Ok(())`。
- 多余参数（`-r <cmd> <extra>...`）按容差忽略，与 `-C`/`-p` 一致。
- 联动：删除后 `-p` 自动走"no completion specification"错误路径；TAB 补全自动退化为默认响铃路径——读端零改动，凭 `Rc<RefCell<HashMap>>` 共享表自然达成。

## 技术方案

### 修改范围

- 单文件：`src/builtins.rs`
- 仅触及一处函数：`run_complete`（L173-L198）的 `match` 表达式新增 `Some("-r")` 臂；同步顶部 doc 注释列表。
- 零改动：`main.rs` dispatch、`completion.rs` 读端、`BUILTINS` 常量、redirect 路由。

### 实现策略

在 `match args.first().map(|s| s.as_str())` 的 `Some("-p")` 与 `_` 通配臂之间插入 `Some("-r")` 分支：

1. `args.get(1)` 取命令名；缺失时直接 `Ok(())`（与现有 `-p` 缺第二参的处理同构）。
2. 命中第二参 → `registry.remove(cmd)`，**丢弃返回值**（`HashMap::remove` 返回 `Option<String>`，`None` 即未注册——题面 Notes 明确按静默成功处理）。
3. 不写 `sink` 也不写 `err_sink`，直接 `Ok(())`。
4. 多余参数忽略（match 不做 arity 检查，与 `-C`/`-p` 容差一致）。

### 关键决策

- **复用 `HashMap::remove` 而非 `contains_key + remove`**：单次哈希查找，O(1)；返回 `Option<String>` 可直接丢弃，免去存在性预检的双查找开销，也契合「未注册不报错」语义。
- **不引入新的命令枚举类型**：现有 `match args.first()` 字符串字面分派模式简单清晰且本文件已有 3 处臂（`-C`/`-p`/`_`），新增第 4 处保持风格一致；引入枚举属于 YAGNI。
- **registry 共享表保持原状**：`Rc<RefCell<HashMap<String, String>>>` 由 `main.rs` 持有并克隆给 `ShellHelper`；`-r` 写端通过现有 `&mut HashMap` 形参写入，TAB 补全读端凭共享 Rc 立即可见，无需任何同步原语。
- **错误信道保持 nop**：`-r` 自身不输出，无需新增 sink 或路由；与 `-C` 同构，零侵入 redirect 模块。

### 实施细节

- `src/builtins.rs` 顶部 doc（L164-L173）当前列了 4 条 `complete` 子命令行为，需插入新的一条「`-r <cmd>`：从 registry 删除；无输出」，保持文档与代码同步、便于后续维护读取。
- `mod tests` 末尾追加 3 个回归用例：
- `complete_r_removes_existing_entry`：`-C` 注册 → `-r` 删除 → `-p` 走 err_sink 输出 `complete: git: no completion specification\n`；同时校验 `-r` 自身的 sink/err_sink 均为空。
- `complete_r_unregistered_silent_ok`：直接对未注册命令 `-r`，sink/err_sink 均空、返回 `Ok(())`。
- `complete_r_then_recover_via_C`：`-C → -r → -C` 重注册后 `-p` 重新命中 stdout，验证 registry 无残留状态。
- 不新增 `-r` 缺第二参用例（与 `-p` 同构，已被覆盖路径隐式保护；如需显式可附加 1 例 `complete_r_missing_arg_silent_ok`）。

### 性能与风险

- 新增分支单次 `HashMap::remove`，O(1) 哈希；本身无热路径，性能影响为零。
- 爆炸半径：仅 `run_complete` 函数体内新增分支与 doc，单文件 + 局部修改；不触及任何调用方签名、不影响现有 `-C`/`-p` 既有行为。
- 兼容性：现有 111 通过测试不受影响；新增 3 例后期望 114+ 通过。

### 目录结构

```
project-root/
└── src/
    └── builtins.rs   # [MODIFY] run_complete 新增 Some("-r") 分支：args.get(1) 命中则 registry.remove(cmd)，未命中静默；
                      # 同步顶部 doc 列表插入 -r 一条；mod tests 追加 3 个 complete_r_* 回归用例。
```