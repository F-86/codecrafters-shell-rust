# 架构总览

> 本文档描述 `codecrafters-shell-rust` 的分层架构、模块依赖、数据流与关键时序。
> 决策细节请见 [DESIGN_DECISIONS.md](./DESIGN_DECISIONS.md)；逐模块清单请见 [MODULES.md](./MODULES.md)。

## 目录

- [1. 分层架构](#1-分层架构)
- [2. 模块依赖图](#2-模块依赖图)
- [3. 数据流](#3-数据流)
- [4. 关键时序](#4-关键时序)
  - [4.1 启动](#41-启动)
  - [4.2 单次命令执行](#42-单次命令执行)
  - [4.3 后台作业生命周期](#43-后台作业生命周期)
  - [4.4 Pipeline 执行](#44-pipeline-执行)
- [5. 模块依赖矩阵](#5-模块依赖矩阵)

## 1. 分层架构

```mermaid
graph TB
    subgraph "REPL 层"
        M[main.rs - REPL 主循环]
    end
    subgraph "解析层"
        TK[parser::tokenize]
        PS[parser::parse_pipeline]
        TK --> PS
    end
    subgraph "调度层"
        D[dispatch match]
    end
    subgraph "执行层"
        BE[builtins::echo/pwd/cd/type]
        BC[builtins::complete]
        BJ[builtins::jobs]
        BH[builtins::history]
        BD[builtins::declare]
        EX[exec::external]
        EP[exec::pipeline]
        HIO[history_io]
    end
    subgraph "IO 适配层"
        RD[redirect - sink 抽象]
        BP[builtins::path - PATH 查找]
    end
    subgraph "交互层"
        CP[completion - TAB 补全]
        RL[rustyline - readline / history]
    end

    M --> RL
    M --> CP
    M --> PS
    M --> D
    D --> BE
    D --> BC
    D --> BJ
    D --> BH
    D --> BD
    D --> EX
    D --> EP
    D --> HIO
    BE --> RD
    BC --> RD
    BJ --> RD
    BH --> RD
    BD --> RD
    EX --> RD
    EP --> RD
    EX --> BP
    EP --> BP
    BE --> BP
    CP --> BP
    HIO --> RL
    CP --> RL
```

每一层有明确的职责边界：

- **REPL 层**：拉起 rustyline、维护共享状态、prompt 前 reap、读取一行、转发到调度层。详见 [main.rs](../src/main.rs)。
- **解析层**：纯函数，无 IO。输入 `&str` + `&HashMap` 变量后端，输出 `Pipeline / ParsedCommand` 或 `ParseError`。
- **调度层**：dispatch match 把命令名映射到 builtin runner 或 `exec::run_external` / `exec::run_pipeline`。
- **执行层**：builtin / 外部命令 / pipeline 各自的运行逻辑。
- **IO 适配层**：`redirect` 提供 `Box<dyn Write>` sink 统一抽象；`builtins::path` 提供 PATH 查找单一数据源。
- **交互层**：rustyline readline + ShellHelper TAB 补全。

## 2. 模块依赖图

```mermaid
graph LR
    main --> parser
    main --> builtins
    main --> completion
    main --> exec
    main --> redirect
    main --> history_io
    main --> rustyline[(rustyline)]
    completion --> builtins
    completion --> parser
    completion --> rustyline
    exec --> builtins
    exec --> parser
    exec --> redirect
    builtins --> parser
    builtins --> redirect
    history_io --> completion
    history_io --> rustyline
    parser
    redirect
```

**无环依赖**：`parser` 与 `redirect` 是叶节点（不依赖任何业务模块）；`completion` / `exec` / `history_io` 互不依赖。

## 3. 数据流

```mermaid
flowchart LR
    UI[用户输入] -->|readline| MAIN[main 主循环]
    MAIN -->|line: &str| TOK[tokenize]
    TOK -->|Vec\<String\>| PARSE[parse_pipeline]
    PARSE -->|Pipeline| DISP{stages.len}
    DISP -->|>1| RP[run_pipeline]
    DISP -->|==1| OPEN[open_sink / open_err_sink]
    OPEN -->|sink, err_sink| BUILT{builtin?}
    BUILT -->|yes| RUN[run_xxx]
    BUILT -->|no| RE[run_external]
    RUN -->|writeln!| SINK[(sink/err_sink<br/>文件 or 终端)]
    RE -->|spawn| CHILD[(子进程)]
    CHILD -->|dup2 fd| TERM[(终端 / 文件)]
    RP -->|spawn N 段| CHILDREN[(子进程链)]
```

关键数据流转换：

| 阶段 | 输入 | 输出 | 模块 |
|---|---|---|---|
| 词法 | `&str` line | `Vec<String>` tokens | `parser::tokenize` |
| 语法 | tokens | `Pipeline { stages: Vec<ParsedCommand>, background }` | `parser::parse_pipeline` |
| 重定向打开 | `Option<&str>` path + `bool` append | `Box<dyn Write>` sink | `redirect::open_sink` |
| 执行 | `ParsedCommand` + `sink/err_sink` | `io::Result<()>` 或子进程退出码 | builtins / exec |

## 4. 关键时序

### 4.1 启动

```mermaid
sequenceDiagram
    participant OS
    participant main
    participant rustyline
    participant history_io
    participant completion

    OS->>main: argv / env (含 $HISTFILE)
    main->>main: Config::builder().completion_type(List).build()
    main->>rustyline: Editor::with_config(config)
    main->>completion: ShellHelper::new(completions.clone())
    Note over completion: 启动期扫描 PATH，缓存可执行文件名
    main->>rustyline: editor.set_helper(Some(helper))
    main->>main: Rc::new(RefCell::new) × 3<br/>(completions / jobs_table / shell_vars)
    main->>history_io: load_history_from_envfile(&mut editor)
    history_io->>OS: env::var("HISTFILE")
    history_io->>OS: File::open(path)
    history_io->>rustyline: editor.add_history_entry(line) × N
    main-->>main: 进入主循环
```

### 4.2 单次命令执行

```mermaid
sequenceDiagram
    participant user as 用户
    participant main
    participant parser
    participant redirect
    participant builtin as builtins::run_xxx
    participant fs as 文件系统/终端

    main->>main: 自动 reap 三步原子操作
    main->>user: 显示 prompt "$ "
    user->>main: 输入 `echo hello > out`
    main->>parser: parse_pipeline(line, &shell_vars)
    parser-->>main: Pipeline { stages: [ParsedCommand { argv: ["echo","hello"], stdout_redirect: Some("out") }], background: false }
    main->>redirect: open_sink(Some("out"), false)
    redirect->>fs: File::create("out")
    redirect-->>main: Box<File>
    main->>redirect: open_err_sink(None, false)
    redirect-->>main: Box<io::stderr()>
    main->>builtin: run_echo(&mut *sink, args)
    builtin->>fs: writeln!(sink, "hello")
    builtin-->>main: Ok(())
    main->>main: 下一轮 readline
```

### 4.3 后台作业生命周期

```mermaid
sequenceDiagram
    participant main
    participant exec as exec::run_external
    participant jobs as jobs_table (Rc<RefCell>)
    participant child as 子进程
    participant kernel as 内核

    user->>main: `sleep 30 &`
    main->>exec: run_external(parsed.background=true, jobs_table)
    exec->>jobs: borrow() → allocate_job_id(&tbl) → drop borrow
    jobs-->>exec: id=1
    exec->>kernel: Command::spawn (fork + execve)
    kernel-->>exec: Child { pid: 1234 }
    exec->>main: writeln!(stdout, "[1] 1234")
    exec->>jobs: borrow_mut().push(Job{ id:1, pid:1234, status:Running, children:[child] })
    main-->>main: 下一轮 readline

    Note over kernel,child: 30 秒后子进程退出，留下僵尸

    user->>main: 按 Enter（任意输入）
    main->>jobs: borrow_mut() → advance_job_status(&mut tbl)
    jobs->>kernel: child.try_wait() (waitpid WNOHANG)
    kernel-->>jobs: Ok(Some(ExitStatus(0))) ← reap 完成
    jobs->>jobs: job.status = Done
    main->>jobs: render_done_jobs(&mut stdout, &tbl)
    jobs->>main: writeln "[1]+ Done sleep 30"
    main->>jobs: retain_running_jobs(&mut tbl) → drop child
    main-->>main: 显示 prompt
```

### 4.4 Pipeline 执行

```mermaid
sequenceDiagram
    participant main
    participant rp as run_pipeline
    participant prev as PrevOutput
    participant c1 as cat 子进程
    participant c2 as wc 子进程
    participant kernel as 内核

    user->>main: `cat file | wc -l`
    main->>rp: run_pipeline(pipeline, &jobs_table)
    rp->>kernel: Command::new("cat").stdin(inherit).stdout(piped()).spawn()
    kernel-->>rp: Child(cat) + ChildStdout(rd1)
    rp->>prev: prev = ChildPipe(rd1)
    rp->>kernel: Command::new("wc").stdin(Stdio::from(rd1)).stdout(inherit).spawn()
    Note over rp: std::mem::replace(&mut prev, None)<br/>把 rd1 move 给 wc 的 stdin，<br/>父进程不再持有 pipe 写端
    kernel-->>rp: Child(wc)
    rp->>prev: prev = None
    rp->>kernel: drop(prev)  // 显式确认无残留 fd
    loop 每个 child
        rp->>kernel: child.wait()
    end
    kernel-->>rp: ExitStatus × 2
    rp-->>main: 返回
```

关键正确性：`prev` 变量在赋值给下一段后立即被 `std::mem::replace` 取走 move 进 `Stdio::from`，父进程内不再有 `ChildStdout` 句柄——下游收到 EOF 链可正常触发。详见 [DESIGN_DECISIONS.md#pipeline-prev-output](./DESIGN_DECISIONS.md#pipeline-prev-output)。

## 5. 模块依赖矩阵

| 上游 ↓ \ 下游 → | parser | builtins | completion | exec | redirect | history_io | main |
|---|---|---|---|---|---|---|---|
| **parser** | — | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **redirect** | ❌ | ❌ | ❌ | ❌ | — | ❌ | ❌ |
| **builtins** | ✅ | — | ❌ | ❌ | ❌ | ❌ | ❌ |
| **history_io** | ❌ | ❌ | ✅ | ❌ | ❌ | — | ❌ |
| **completion** | ✅ | ✅ | — | ❌ | ❌ | ❌ | ❌ |
| **exec** | ✅ | ✅ | ❌ | — | ✅ | ❌ | ❌ |
| **main** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | — |

读法：上游 `parser` 行 / 下游 `builtins` 列 = ❌ → parser 不依赖 builtins。
- `parser` 与 `redirect` 是纯叶节点（仅依赖标准库）
- `main` 是唯一依赖所有模块的根节点
- 无循环依赖
