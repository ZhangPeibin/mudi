//! macOS 剪贴板写入适配器。
//!
//! 这个模块只负责一件事：把用户明确要求复制的**一段完整文本**写入系统剪贴板。
//! 它不会主动读取剪贴板，也不会决定什么时候应该复制；是否执行复制由上层 UI 和应用层控制。
//!
//! 从 Rust 架构角度看，这个文件展示了一个很常见的“端口 + 适配器”写法：
//!
//! - [`ClipboardPort`] trait 在 Application 边界定义上层真正需要的能力，即“写入文本”；
//! - [`MacosClipboard`] 是生产环境适配器，通过 macOS 自带的 `pbcopy` 实现该能力；
//! - 测试可以实现自己的 `ClipboardPort`，记录传入值或主动返回错误，而不需要真的修改系统剪贴板。
//!
//! 这样，上层代码依赖的是抽象能力，而不是 `/usr/bin/pbcopy` 这个具体命令。

// `std::io` 提供统一的 I/O 错误类型和写入 trait：
//
// - `io`：让下文可以写 `io::Result`、`io::Error`，而不必反复写完整路径；
// - `Write`：提供 `write_all` 方法。这个 trait 必须在作用域中，才能对
//   `ChildStdin` 调用 `write_all`。
use crate::application::clipboard::ClipboardPort;
use std::io::{self, Write};

// `Command` 用于配置并启动子进程；`Stdio` 用于配置子进程的标准输入、输出和错误流。
use std::process::{Command, Stdio};

/// 使用 macOS 系统命令 `/usr/bin/pbcopy` 的剪贴板适配器。
///
/// 这是一个“单元结构体”（unit struct）：它没有字段，也不保存状态，所以创建实例时只需写
/// `MacosClipboard`，不需要花括号或构造函数。这个类型的主要作用是承载 `ClipboardPort` 实现，
/// 把具体的 macOS 行为封装在抽象接口后面。
pub struct MacosClipboard;

// `impl Trait for Type` 表示“为某个具体类型实现某个 trait”。
// 实现以后，`MacosClipboard` 就可以出现在任何需要 `ClipboardPort` 的位置。
impl ClipboardPort for MacosClipboard {
    fn write_text(&mut self, value: &str) -> io::Result<()> {
        // `Command::new` 只创建命令配置，还不会立即启动进程。
        // 使用绝对路径可以避免依赖用户的 `PATH`，明确调用 macOS 系统自带的 pbcopy。
        let mut child = Command::new("/usr/bin/pbcopy")
            // `pbcopy` 从标准输入读取要放入剪贴板的内容。
            // `Stdio::piped()` 要求 Rust 在父子进程之间建立一条匿名管道，并把管道的
            // 写入端暴露为 `child.stdin`，供父进程写数据。
            .stdin(Stdio::piped())
            // `spawn` 真正启动子进程，并返回 `Child` 句柄。
            // 末尾的 `?` 是错误传播运算符：如果启动失败，立即把 `io::Error` 返回给调用方；
            // 如果成功，则从 `Ok(child)` 中取出 `child`，继续向下执行。
            .spawn()?;

        // `child.stdin` 的类型是 `Option<ChildStdin>`，因为只有在命令配置了管道时，
        // Rust 才能提供可写的标准输入句柄；API 用 `Option` 在类型层面表达“可能不存在”。
        let mut stdin = child
            .stdin
            // `take()` 把 `ChildStdin` 的所有权从 `child.stdin` 移到当前局部变量中，
            // 同时把原位置替换为 `None`。不能直接把字段移动出来后继续使用整个 `child`，
            // 而 `take()` 可以避免形成“部分移动”，所以后面仍然能调用 `child.wait()`。
            .take()
            // 如果理论上应当存在的管道没有创建出来，就把 `None` 转换成一个明确的 I/O 错误。
            // `ok_or_else` 接收闭包并延迟构造错误：只有值确实为 `None` 时才执行 `|| ...`。
            .ok_or_else(|| io::Error::other("pbcopy stdin unavailable"))?;

        // `str::as_bytes()` 只创建一个指向同一段 UTF-8 数据的字节切片，不会重新编码或复制。
        // `write_all` 会持续写入，直到全部字节都写完或遇到错误；相比单次 `write`，它不会要求
        // 调用方自己处理“只写入了一部分字节”的正常情况。
        //
        // 这里暂时不使用 `?`，而是把结果保存下来。这样即使写入失败，下面仍会先关闭管道并
        // 回收子进程，避免过早返回后留下尚未等待的 `pbcopy` 进程。
        let write_result = stdin.write_all(value.as_bytes());

        // 显式销毁 `stdin`，也就是关闭父进程持有的管道写入端。
        // 对 `pbcopy` 来说，读到 EOF 才知道输入已经结束，随后它才能完成剪贴板更新并退出。
        // 如果不显式 `drop`，`stdin` 默认要到函数作用域末尾才释放，而下一行 `wait()` 又会等待
        // 子进程退出，两边就可能互相等待。因此这里的资源释放顺序是正确性要求，不只是优化。
        drop(stdin);

        // 等待子进程结束并取得 `ExitStatus`。这也会回收操作系统中的子进程资源。
        // 如果等待本身失败，`?` 会立即返回该错误。
        let status = child.wait()?;

        // 子进程已经被等待并回收，现在再传播之前保存的写入错误。
        // 对 `Result<()>` 使用 `?`：成功时得到 `()` 并继续；失败时把原 `io::Error` 返回。
        write_result?;

        // “成功写完管道”不等于“pbcopy 执行成功”，所以还必须检查进程退出状态。
        if status.success() {
            Ok(())
        } else {
            // `io::Error::other` 用于创建 `ErrorKind::Other` 错误。
            // `format!` 生成拥有所有权的 `String`；`{status}` 使用 `ExitStatus` 的 Display 实现，
            // 把退出码或信号等信息放进错误消息，便于上层向用户报告失败原因。
            Err(io::Error::other(format!(
                "pbcopy exited with status {status}"
            )))
        }
    }
}
