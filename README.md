[![progress-banner](https://backend.codecrafters.io/progress/shell/42db5ea8-face-4eb3-8ec4-a91493a839f2)](https://app.codecrafters.io/users/codecrafters-bot?r=2qF)

# codecrafters-shell-rust

一个用 Rust 实现的 POSIX 兼容交互式 shell，源自
[CodeCrafters「Build Your Own Shell」挑战](https://app.codecrafters.io/courses/shell/overview)。

具备完整的命令解析（引号 / 转义 / `$VAR` 展开 / 重定向 / pipeline / 后台作业）+
TAB 自动补全 + 历史持久化的最小可用 shell。

## 已实现特性

- **9 个内建命令**：`echo` / `exit` / `pwd` / `cd` / `type` / `complete` / `jobs`
  / `history` / `declare`
- **解析能力**：单/双引号、引号外反斜杠、`\$` 转义、`$VAR` / `${NAME}` 展开 +
  null word removal
- **重定向**（6 种算子）：`>` / `1>` / `>>` / `1>>` / `2>` / `2>>`，stdout / stderr 完全正交
- **Pipeline**：N 段命令以 `|` 串联，每段独立保留重定向；`echo` / `pwd` / `type`
  可作为 pipeline 段；SIGPIPE 自然回收
- **后台作业**（`&`）：`try_wait` 非阻塞 reap + 每轮 prompt 前显示 `Done` 通知行 +
  `[N] PID` 通知行不被 `>` 捕获 + 作业编号「最小可用正整数」复用
- **TAB 补全**：命令名（builtin + PATH）/ 参数路径（cwd + 嵌套目录）/
  `complete -C` 注册的命令级脚本三套独立状态机；LCP 扩展 + 双 TAB（首次 BEL → 二次列出）
- **历史持久化**：启动时 `$HISTFILE` 加载 + 退出时保存 + `history -r/-w/-a` 三段文件 IO

## 架构总览

```
       ┌─────────────────────┐
       │  main.rs - REPL     │── readline ───────┐
       └─────────┬───────────┘                   ▼
                 │                       ┌──────────────┐
                 ▼                       │  rustyline   │
       ┌─────────────────────┐           │  + 补全 helper│
       │  parser (词法+语法) │           └──────────────┘
       └─────────┬───────────┘                   ▲
                 │ Pipeline                      │
                 ▼                               │
       ┌─────────────────────┐         ┌─────────┴──────┐
       │   dispatch  match   │────────▶│  completion::* │
       └────┬───────────┬────┘         │  (3 套状态机)  │
            │           │              └────────────────┘
            ▼           ▼
   ┌──────────────┐  ┌────────────┐         ┌──────────────┐
   │ builtins::*  │  │  exec::*   │────────▶│   redirect   │
   │ (9 个 builtin)│  │ external/  │         │ Box<dyn Write│
   │ + Job 管理   │  │ pipeline   │         └──────────────┘
   └──────┬───────┘  └─────┬──────┘
          │                │
          ▼                ▼
   ┌──────────────────────────────┐
   │   builtins::path  (PATH 单一  │
   │   数据源，跨 builtins/exec/   │
   │   completion 共用)            │
   └──────────────────────────────┘
```

完整的分层架构图、模块依赖图、4 条关键时序图见
[docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md)。

## 模块组织

```
src/
├── main.rs            — REPL 主循环骨架（241 行，仅 dispatch）
├── parser/            — 词法 + 语法（叶节点，纯函数）
│   ├── mod.rs         — 公开 API + ParseError
│   ├── tokenize.rs    — 字符级状态机（Normal / InSingleQuote / InDoubleQuote 三态）
│   ├── parse.rs       — 重定向识别 + pipeline 切分
│   └── tests.rs       — 60+ 单元测试
├── builtins/          — 9 个内建命令各自独立成文件
│   ├── path.rs        — find_in_path / list_path_executables
│   ├── echo.rs / pwd.rs / cd.rs / type_cmd.rs
│   ├── complete.rs    — -C / -p / -r 三路分派
│   ├── jobs.rs        — Job/JobStatus + 状态推进/渲染/移除 + run_jobs
│   ├── history.rs     — 渲染逻辑（纯函数）
│   └── declare.rs     — NAME 校验 + 值转义 + 5 路分派
├── completion/        — TAB 补全
│   ├── mod.rs         — ShellHelper 结构 + Completer trait impl
│   ├── command.rs     — 命令名状态机
│   ├── argpath.rs     — 参数路径状态机
│   ├── script.rs      — complete -C 命令级脚本状态机
│   └── helpers.rs     — LCP / 路径分类 / 目录扫描等纯函数
├── exec/              — 命令执行
│   ├── mod.rs
│   ├── external.rs    — 单命令外部进程
│   └── pipeline.rs    — N 段管线（PrevOutput 三态枚举）
├── redirect.rs        — sink 抽象（Box<dyn Write>）
└── history_io.rs      — $HISTFILE 启动加载/退出保存 + history -r/-w/-a
```

逐模块的「职责 / 公开 API / 依赖 / 关键不变量」见 [docs/MODULES.md](./docs/MODULES.md)。

## 技术选型一句话总结

唯一运行时依赖 **`rustyline = "14"`**（readline + history + 补全 trait 框架）；
其它一律手写：状态机 tokenizer、`Rc<RefCell<>>` 单线程共享状态、`try_wait` +
prompt 前 reap 的非阻塞作业回收、`PrevOutput` 三态枚举驱动 pipeline、`Cell` 内部
可变性承载三套独立 TAB 状态机。

每项决策的「问题 / 选择 / 原因 / 代价 / 备选方案」五段式见
[docs/DESIGN_DECISIONS.md](./docs/DESIGN_DECISIONS.md)。

## 快速开始

### 前置要求

- Rust **1.95+**（edition 2024）
- Linux / macOS（Unix fd 继承 + 信号语义；Windows 不支持）

### 构建与运行

```sh
# 直接跑
./your_program.sh

# 或显式编译
cargo build --release
./target/release/codecrafters-shell
```

### 跑测试

```sh
cargo test                      # 全部（216 单元 + 16 集成）
cargo test --lib parser         # 仅 parser 单元测试
cargo test --test pipeline_basic
```

### 生成 rustdoc 文档站点

```sh
cargo doc --no-deps --document-private-items --open
```

## 文档导航

| 文档 | 主题 |
|---|---|
| [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) | 分层架构 + 模块依赖图 + 4 条关键时序（启动/单次命令/后台作业/Pipeline） |
| [docs/DESIGN_DECISIONS.md](./docs/DESIGN_DECISIONS.md) | 8 个技术选型主题的五段式（问题/选择/原因/代价/备选方案） |
| [docs/MODULES.md](./docs/MODULES.md) | 逐模块的职责 / 公开 API / 依赖 / 关键不变量 |
| [docs/TESTING.md](./docs/TESTING.md) | 测试组织 / FIFO 验证 stdio 继承 / 新增测试指引 |

## CodeCrafters 提交

```sh
codecrafters submit
```

测试输出会流式回到当前终端。
