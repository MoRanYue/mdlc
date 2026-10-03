# 更新检测

`mdlc` 每次运行都会在后台检查有没有新版本。本文记录**为什么这么设计**、
**实测代价**，以及那个最不显然的坑 —— **「后台」并不自动等于「不拖累调用方」**。

> 实现见 `src\update.rs`。本文只写结论与实测数据。

---

## 0. 结论速览

| 问题 | 答案 |
|---|---|
| 会阻塞编译吗？ | **不会**。网络 I/O 在派生子进程里，主进程只读一个 ~100 字节的缓存文件 |
| 编译速度受影响吗？ | **增量无可测差异**；冷构建 +16 s（多 27 个包） |
| 二进制会变大吗？ | **会，+2.0 MB（5.77 → 7.78 MB，+35%）** —— 主要是 `rustls` + `ring` |
| 提示什么时候出现？ | **下一次运行**。第 1 次派生子进程去查，第 2 次读缓存打印 |
| 会让调用方（Crowbar）白等吗？ | 修之前**会**（+599 ms）；修之后**不会**（19 ms） |
| 怎么关掉？ | `MDLC_NO_UPDATE_CHECK=1`；CI 环境自动跳过 |
| 怎么排错？ | `MDLC_UPDATE_DEBUG=1`（把失败原因打到 stderr） |

---

## 1. 为什么要做「后台子进程」而不是「后台线程」

`mdlc` 是**短命进程**：`mdlc help` 1 ms 就退出，编译一个模型也就几十毫秒。

后台线程会随主进程一起消失 —— 请求根本发不出去。只有 `spawn` 出去的
**子进程**能在父进程退出后继续跑完，并把结果写进缓存供下次运行读取。

代价是提示有**一拍延迟**：

```text
第 1 次运行  ──► 缓存过期 ⟹ 派生子进程（立即返回，不等）
                       └─► 子进程请求 GitHub，写缓存
第 2 次运行  ──► 读缓存 ⟹ 有新版本？打印一行
```

这是所有「后台静默检测」工具的固有形态：要当次就报，就只能阻塞等待。

---

## 2. 实测代价

### 2.1 构建时间

| 配置 | 冷构建 | 增量 |
|---|---|---|
| 无更新检测 | 基线 | 基线 |
| **有更新检测（`ureq` + `rustls`）** | **+16 s** | **无可测差异** |

`Cargo.lock` 的 `[[package]]` 从 **108** 涨到 **135**（+27 个包）。

选型时对比过 `reqwest`（+60 包左右，且拉 tokio）与「curl 子进程」（零依赖，
但用户明确否决了子进程路线：要求用第三方 crate 发请求）。

### 2.2 二进制体积

| 版本 | `mdlc.exe` |
|---|---|
| 无更新检测 | 5,772,800 B |
| **有更新检测** | **7,777,792 B**（+2,005 KB / **+34.7%**） |
| 有更新检测 + `windows` 句柄守卫 | **7,781,888 B**（再 +4 KB） |

⚠️ 早期估算「只增大约 300 KB」**是错的** —— 那个数字来自「三种 HTTP 方案
编出来都是 5,772,800 B」的观察，但那是 **dead-code 消除**的结果（当时
`update.rs` 还没接线，`ureq` 根本没被引用）。真实成本是 `rustls` + `ring`
的加密代码体量。

### 2.3 每次运行的开销

| 状态 | 主进程耗时 |
|---|---|
| 缓存新鲜（24 小时内） | 读一个 ~100 字节文件，**微秒级** |
| 缓存过期 | 同上 + `spawn` 一个子进程（**不等它**） |

---

## 3. ⭐ 管道：为什么「后台」还会让调用方白等几百毫秒

**这是本特性最不显然的一条，也是唯一一条会让用户感知到的。**

### 3.1 症状

Crowbar 编译模型时，如果缓存恰好过期，界面会**卡住半秒**才出结果。
「后台检测」本该毫无感觉才对。

### 3.2 机制

GUI 包装不是 `wait()` 进程退出，而是 `ReadToEnd()` —— **读管道到 EOF**。

EOF 只在**所有写端都关闭**时才到达，**包括我们派生出去的子进程手里那一份**。
于是：

```text
父进程 3 ms 就退出了     ← 我们的「后台」做到了
但子进程还活着 600 ms    ← 它握着调用方 stdout 管道的写端
⟹ 调用方 603 ms 才拿到 EOF
```

**「父进程退出得快」不等于「调用方拿到结果快」。**

### 3.3 根因

`std::process::Command` 在 Windows 上**无条件**用 `bInheritHandles = TRUE`
调 `CreateProcessW`：

- `std\src\sys\process\windows.rs:193` —— `inherit_handles: true`（默认值）
- `:421` —— 把它当第 5 个参数传进去
- `inherit_handles(bool)` 这个 setter 是 **unstable**（`issue = "146407"`），
  stable 上用不了

而且 `std` **没有**用 `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` 收窄集合
（全 std 源码 grep = 0 命中）⟹ 无法只传递指定句柄。

**`Stdio::null()` 挡不住**：它只换掉子进程的 0/1/2 三个槽，管道句柄仍在
可继承集合里。

### 3.4 判别实验

同一份过期缓存、同一个 exe，**只换重定向方式**，交替跑：

| 场景 | 耗时 |
|---|---|
| **FILE** + 过期（`Start-Process -RedirectStandardOutput <文件>`） | **138 ms**，重跑 **2 ms** |
| **PIPE** + 过期（.NET `RedirectStandardOutput`） | **658 ms / 433 ms** |
| FILE + 新鲜 | 7 ms |
| PIPE + 新鲜 | 7 ms |

⟹ **管道是唯一变量**。这是对机制的直接证明（此前只有「子进程寿命 ≈
多等时长」的间接吻合）。

### 3.5 修复

在 `spawn` **之前**把自己三个 std 句柄的 `HANDLE_FLAG_INHERIT` 位清掉，
`spawn` 之后恢复。用 `windows` crate（`HANDLE` 是 newtype 而不是 `isize`，
`GetStdHandle` 直接给 `Result`，`HANDLE::is_invalid()` 一次覆盖 `NULL` 与
`INVALID_HANDLE_VALUE` 两个哨兵 —— 这几处正好都是容易写错的地方；
代价是 +11 个包，含两个 proc-macro crate）。

用 `Drop` 而不是「`spawn` 之后手动恢复」：`spawn` 可能失败，中间也可能
panic，而**忘了恢复会让本进程后续派生的进程拿不到 stdout** —— 那种 bug
比原本多等几百毫秒严重得多。

### 3.6 修复效果（实测）

`target\ab\pipe_ab2.js`，管道重定向、缓存过期、交替跑各 3 轮：

| 调用形态 | 修复前（中位） | 修复后（中位） | 省 |
|---|---|---|---|
| **官方形态 `-game <dir> <qc>`**（Crowbar 走的） | **618 ms** | **19 ms** | **599 ms** |
| mdlc 自有形态 `--version` | 420 ms | 17 ms | 403 ms |

原始数据：

```text
官方形态  A(nofix) 673, 618, 444 ms   B(fix) 20, 19, 18 ms
自有形态  A(nofix) 420, 400, 540 ms   B(fix) 17, 16, 23 ms
```

### 3.7 不会饿死子进程（非空洞性验证）

清掉可继承位后，子进程的 stdio 从哪来？—— `std` 的 `Stdio::Null` 是自己
打开 `\\.\NUL` 并显式设 `inherit_handle(true)`（`windows.rs:642`），用的
**不是**我们那个句柄。而 `Stdio::inherit()` 走 `DuplicateHandle(..,
bInheritHandle = TRUE, ..)`，新句柄的可继承性由**参数**决定，与源句柄无关。

实测（`target\ab\verify_grandchild.js`，删掉缓存后跑）：

```text
父进程关闭花了 23 ms，输出 = "mdlc 0.1.0"
✓ 孙进程在 750 ms 内写出了缓存：{"last_check":1791036283,"latest":"v0.1.0"}
✓ 缓存内容合法
```

⟹ **父进程不再被拖累，而检测照样完成。**

---

## 4. 诊断流路由

更新提示是一条诊断，所以它跟着 `crate::diag` 的既有规则走：

| 调用形态 | 提示落点 |
|---|---|
| 官方兼容形态（`-game <dir> <qc>` 或裸 `<qc>`） | **stdout**（与官方 `studiomdl` 一致，Crowbar 只认 stdout） |
| mdlc 自有形态（`build-qc` / `--version` / …） | **stderr** |

⚠️ **路由必须在 `startup()` 之前定好**。最初的实现把更新检测放在
`main()` 最前面，而 `set_to_stdout(true)` 在 `run_official()` 里 —— 于是
官方形态下的提示落到了 stderr 上，Crowbar 看不到。判据（`official_form`）
纯由 `rest` 决定、没有副作用，所以可以安全地提到检测之前。

实测（`target\ab\e2e_all.js`，四种形态）：

```text
✓ 官方：-game <dir> <qc>      exit=0    提示在 stdout
✓ 官方：裸 <qc>                exit=0    提示在 stdout
✓ mdlc：--version           exit=0    提示在 stderr
✓ mdlc：build-qc --help     exit=0    提示在 stderr
```

---

## 5. 缓存与开关

| 项 | 值 |
|---|---|
| 位置 | Windows `%LOCALAPPDATA%\mdlc\update-check.json`；其余 `$XDG_CACHE_HOME/mdlc/` 或 `~/.cache/mdlc/` |
| 内容 | `{"last_check": <unix 秒>, "latest": "v0.1.0"}` |
| 间隔 | 24 小时；期内**不派生任何进程** |
| 关闭 | 环境变量 `MDLC_NO_UPDATE_CHECK=1` |
| CI | 检测到 `CI` 环境变量时自动跳过 |
| 排错 | `MDLC_UPDATE_DEBUG=1` 时把失败原因打到 stderr |

写入是**原子**的：先写 `update-check.json.tmp` 再 rename。

### 手动触发

```console
$ mdlc __update-check
```

隐藏子命令（不出现在 `--help` 里），**同步**跑一次检测。它同时是派生出
去的那个子进程的入口 —— 所以「手动跑」与「后台跑」走**同一条代码路径**，
不会分叉。

---

## 6. `--version`

顺带补上的：`mdlc --version` / `-V` 现在可用（原先被
`.disable_version_flag(true)` 关掉）。

⚠️ **官方兼容形态仍不认 `--version`**（`build_official_cli()` 保留
`.disable_version_flag(true)`）—— 官方 `studiomdl` 没有这个选项，而
`mdlc -game … --version` 会被当成未知选项警告后继续。

---

## 7. 验收清单

| 项 | 命令 | 期望 |
|---|---|---|
| 单元测试 | `cargo test --locked --lib update::` | 10 passed |
| 全量测试 | `cargo test --locked` | 815 passed / 0 failed / 6 ignored |
| clippy | `cargo clippy --release --all-targets --locked -- -W clippy::all -D warnings` | 0 warning |
| 文档 | `RUSTDOCFLAGS=-D warnings cargo doc --no-deps --locked --target-dir target\doccheck` | 0 warning |
| 回归 | `node docs\_probe\parity_snapshot.js` | 101/101 |
| 管道 A/B | `node docs\_probe\update_pipe_ab.js <nofix.exe> <release.exe> parity\anim.qc` | 官方形态省 ~600 ms |
| 子进程存活 | `node docs\_probe\update_grandchild.js target\release\mdlc.exe` | 孙进程写出缓存 |
| 落点 | `node docs\_probe\update_e2e_routing.js target\release\mdlc.exe parity\anim.qc` | 四种形态全过 |

⚠️ 上面三个探针在 `docs\_probe\` 下，被 `.gitignore` 忽略（不是仓库资产，
但也不是一次性脚本 —— 改动 `spawn_check` 时应重跑）。

⚠️ **句柄守卫的两个单测必须串行**：它们改的是**进程级**的 std 句柄状态。
并行跑时一个测试的守卫析构会把可继承位恢复成 `true`，正好插进另一个测试
「断言已清掉」的窗口 —— 实测不加锁时 `cargo test --lib update::`
**每 5 次里约 1 次失败**（`--test-threads=1` 则 10/10 通过）。
修法是 `mod tests` 里一把 `static SERIAL: Mutex<()>`。

---

## 8. 已知取舍

1. **+2.0 MB 二进制** —— 换一个「有新版会告诉你」的功能，值不值取决于用途。
   编译器的使用者往往是装一次用很久，体积不是瓶颈。
2. **提示有一拍延迟** —— 见 §1，这是「不阻塞」的必然代价。
3. **引入仓库第一处 `unsafe`** —— 只有 `no_inherit` 模块里的
   `GetStdHandle` / `GetHandleInformation` / `SetHandleInformation` 三个
   Win32 调用，每个都带 `// SAFETY:` 注释。stable 上没有安全替代
   （`inherit_handles(false)` 是 unstable，std 也没用
   `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`）。
4. **`windows` 成为 Windows 专属直接依赖** —— 真实代价 **+11 个包**
   （含 `windows-implement` / `windows-interface` 两个 proc-macro crate）。

   ⚠️ 早期版本这里写的是「`windows-sys 0.52` 已在 lock 里、**不新增任何包**」，
   **那是错的**：`ring` 对 `windows-sys` 的依赖挂在
   `cfg(all(all(target_arch = "aarch64", target_endian = "little"), target_os = "windows"))`
   下（`ring-0.17.14\Cargo.toml`），x86_64 上根本不进 lock。
   实测 `git show 32ffe0c:Cargo.lock` 搜 `windows-sys` = **0 命中**。

   三种方案的实测包数（基线 135）：

   | 方案 | 包数 | 新增 |
   |---|---|---|
   | `windows-sys = "0.52"` | 135 | +0（**当时不在 lock 里，实际是 +2**） |
   | `windows-sys = "0.61"` | 137 | +2（`windows-sys` / `windows-link`） |
   | **`windows = "0.62"`（采用）** | **146** | **+11** |

   ⚠️ `windows-sys 0.61` 把 `HANDLE` 从 `isize` 改成了
   `*mut core::ffi::c_void`，`if h == 0` 这类写法直接编译不过
   （实测 `error[E0308] --> src\update.rs:353`）—— 这也是选 `windows`
   的一个附带理由：它把「句柄有效性」收进 `HANDLE::is_invalid()`，
   不再依赖调用方记得两个哨兵值。
