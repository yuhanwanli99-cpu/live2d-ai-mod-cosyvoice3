//! 子进程抽象、Windows 入口解析、以及**唯一**的真实 `Command` 拼装点。
//!
//! # 为什么要抽一层
//!
//! 单测**不 exec 真进程**：注入 [`Spawner`] 返回假句柄，就能断言 argv / 停止调用 /
//! 失败路径。真实实现 [`real_spawn`] 只在这里出现一次，「有没有走 shell」「stdio
//! 怎么接」「怎么收进程树」这类红线才有单一审计点。
//!
//! # Windows 入口：`.cmd` 是**故意**支持的（与 local-tts 相反）
//!
//! `live2d-ai-mod-local-tts` 在 Windows 上**拒绝**一切 `.sh/.bat/.cmd`——因为它
//! 内置的引擎入口是 bash 脚本，把脚本交给 `CreateProcess` 只会得到一句
//! 「不是有效的 Win32 应用程序」。**本 Mod 的存在理由正好相反**：CosyVoice3 的
//! sidecar 是本机 **Windows 原生**进程（`start.cmd` 里直接起 `python.exe` + CUDA），
//! Windows 侧唯一的入口就是 `D:/live2dai/tts-cosyvoice3/start.cmd`。**它不经过
//! `wsl.exe`、也不经过 bash**——本 Mod 的目标就是「没有 WSL2 也能跑」。
//! 所以这里**显式**按扩展名挑解释器，而不是把脚本直接扔给 CreateProcess：
//!
//! | 扩展名 | 实际 argv | 理由 |
//! | --- | --- | --- |
//! | `.exe` | `[program]` | 原生可执行 |
//! | `.cmd` / `.bat` | `[cmd.exe, /C, program]` | CreateProcess 不执行批处理，必须经 cmd |
//! | `.ps1` | `[powershell.exe, -NoProfile, -ExecutionPolicy, Bypass, -File, program]` | 脚本必须经解释器 |
//! | 其它（`.sh` / `.py` 等） | **报错** | 不猜解释器、也不经 `wsl.exe`/bash；请换成上面三种启动器 |
//!
//! 判据是**纯函数** [`entry_argv`]（带 `windows` 参数），所以 Linux 单测也能把
//! 三条路径全锁住。非 Windows 目标（开发 / 门禁）按路径原样 exec，不经过 shell。
//!
//! # 进程形态
//!
//! - `stdin = null`，`stdout = inherit`，`stderr = piped` 并由本模块的读线程读干、
//!   透传到宿主 stderr、再留最后 [`STDERR_TAIL_MAX`] 字节——子进程**起来就失败**时，
//!   `start` 的 Err 要带上**启动器自己印的原因**（不是我们编的「未知错误」）。
//! - **Unix**：子进程放进**自己的进程组**（`process_group(0)`），停止时
//!   `kill -s KILL -- -<pgid>` 一次收掉 `start.sh` fork 出的孙进程。
//! - **Windows**：`taskkill /T /F /PID <pid>` 收掉**我们自己拉起的整棵进程树**
//!   （cmd.exe → python.exe → …）。**只按本 Mod 拿到的 pid 杀**，不扫端口、不按镜像名
//!   杀别的进程，所以不会误伤用户手动起的 sidecar。

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// stderr 尾巴的保留上限（**字节**）：它只给 Err 附一句「启动器说了什么」。
pub const STDERR_TAIL_MAX: usize = 2048;

/// 一次非阻塞观察的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildProbe {
    /// 仍在跑（`try_wait` 回了 `Ok(None)`）。
    Running,
    /// 已退出；`code` 为 `None` = 被信号终止（拿不到退出码，**不编一个**）。
    Exited { code: Option<i32> },
}

/// 子进程句柄（生产实现 = `std::process::Child` 的薄包装）。
pub trait ChildProcess: Send {
    /// 进程号（拿不到时 `None`）。
    fn pid(&self) -> Option<u32>;
    /// **非阻塞**观察；出错原样上抛（调用方据此判 `spawn_failed`）。
    fn try_wait(&mut self) -> std::io::Result<ChildProbe>;
    /// 请求停止（含**整棵**本 Mod 拉起的进程树）；停完还要 [`ChildProcess::wait`] 收尸。
    fn stop(&mut self) -> std::io::Result<()>;
    /// 阻塞等待结束（收尸）。
    fn wait(&mut self) -> std::io::Result<()>;
    /// 子进程 **stderr 的尾巴**（没有捕获 / 一个字都没输出 → `None`，**不编**）。
    fn stderr_tail(&self) -> Option<String>;
}

/// 一次 spawn 请求（argv + 可选工作目录）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnRequest {
    /// 完整 argv（含 `argv[0]`）。
    pub argv: Vec<String>,
    /// `current_dir`；`None` = 不设（继承宿主 cwd）。
    pub workdir: Option<PathBuf>,
}

/// 可注入的 spawner（单测注入假实现，不 exec 真进程）。
pub type Spawner = Box<dyn Fn(&SpawnRequest) -> std::io::Result<Box<dyn ChildProcess>> + Send>;

/// 启动器路径 → 实际 argv（**纯函数**，`windows` 由调用方给，单测因此不用跑 Windows）。
///
/// 非 Windows 目标：按路径原样 exec（脚本靠可执行位；本仓在 Linux 上只跑门禁）。
/// Windows 目标：见模块头注的扩展名表；不认识的扩展名**报错**而不是猜。
pub fn entry_argv(program: &str, windows: bool) -> Result<Vec<String>, String> {
    let trimmed = program.trim();
    if trimmed.is_empty() {
        return Err("program 为空：没有可执行入口（argv[0]）".to_string());
    }
    if !windows {
        return Ok(vec![trimmed.to_string()]);
    }
    let ext = Path::new(trimmed)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let argv: Vec<String> = match ext.as_str() {
        "exe" => vec![trimmed.to_string()],
        "cmd" | "bat" => vec!["cmd.exe".into(), "/C".into(), trimmed.to_string()],
        "ps1" => vec![
            "powershell.exe".into(),
            "-NoProfile".into(),
            "-ExecutionPolicy".into(),
            "Bypass".into(),
            "-File".into(),
            trimmed.to_string(),
        ],
        // 无扩展名：可能真是个 exe（Windows 不强制扩展名），按原样交给 CreateProcess。
        "" => vec![trimmed.to_string()],
        other => {
            return Err(format!(
                "Windows 原生环境不能直接执行 .{other}：{trimmed}。本 Mod 不经过 wsl.exe / bash，\
                 也不替你挑解释器：请把 program 配成 start.cmd（推荐，sidecar 目录里那个）/ .bat / .exe / .ps1 启动器"
            ));
        }
    };
    Ok(argv)
}

/// 生产实现：`std::process::Command`，逐参数、无 shell、stderr 接管道并读干。
pub fn real_spawn(req: &SpawnRequest) -> std::io::Result<Box<dyn ChildProcess>> {
    let Some((program, args)) = req.argv.split_first() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "argv 为空（连 argv[0] 都没有）",
        ));
    };
    let mut command = Command::new(program);
    command.args(args);
    if let Some(dir) = &req.workdir {
        command.current_dir(dir);
    }
    command.stdin(Stdio::null());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::piped());
    // 自己的进程组：停的时候 group-kill 一次收掉 start.sh fork 出的孙进程。
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut handle = command.spawn()?;
    let tail: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let reader = handle.stderr.take().map(|stderr| {
        let sink = Arc::clone(&tail);
        std::thread::spawn(move || drain_stderr(stderr, sink))
    });
    Ok(Box::new(RealChild {
        child: handle,
        tail,
        reader: Mutex::new(reader),
    }))
}

/// 把 stderr 读干：逐行透传到宿主 stderr，同时在内存里留最后 [`STDERR_TAIL_MAX`] 字节。
fn drain_stderr(source: impl std::io::Read, sink: Arc<Mutex<String>>) {
    let mut reader = BufReader::new(source);
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        // 非 UTF-8 用 lossy：宁可看见一个替换字符，也不要丢掉这行原因。
        let text = String::from_utf8_lossy(&buf);
        let text = text.trim_end_matches(['\n', '\r']);
        eprintln!("{text}");
        let mut guard = sink.lock().unwrap_or_else(|e| e.into_inner());
        guard.push_str(text);
        guard.push('\n');
        if guard.len() > STDERR_TAIL_MAX {
            let cut = guard.len() - STDERR_TAIL_MAX;
            // 按字符边界裁（错误消息里是中文）。
            let cut = guard
                .char_indices()
                .map(|(i, _)| i)
                .find(|i| *i >= cut)
                .unwrap_or(guard.len());
            guard.drain(..cut);
        }
    }
}

/// 收掉**整个进程组**（Unix）。返回是否成功发出信号。
#[cfg(unix)]
fn kill_process_group(pid: u32) -> bool {
    Command::new("kill")
        .arg("-s")
        .arg("KILL")
        .arg("--")
        .arg(format!("-{pid}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// 收掉**我们自己拉起的**整棵进程树（Windows）。
///
/// `/T` = 连子进程一起，`/F` = 强杀，`/PID` = **只按本 Mod 记录的 pid**。
/// 不用 Job Object 是因为那要引入 Win32 FFI（本 crate `unsafe_code = deny`），
/// 而 `taskkill /T` 的作用域同样是「这棵树的根是我们」，不会碰到用户手动起的 sidecar。
#[cfg(windows)]
fn taskkill_tree(pid: u32) -> bool {
    Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// `std::process::Child` 的薄包装（把 `ExitStatus` 归一成 `Option<i32>`）。
struct RealChild {
    child: std::process::Child,
    tail: Arc<Mutex<String>>,
    /// 读线程句柄；`stderr_tail` 时取回收尾（**只能 join 一次**）。
    reader: Mutex<Option<JoinHandle<()>>>,
}

impl RealChild {
    /// 把读线程收回来。子进程已退出 → 管道 EOF → 线程立刻结束，不会卡住。
    fn join_reader(&self) {
        let handle = self.reader.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

impl ChildProcess for RealChild {
    fn pid(&self) -> Option<u32> {
        Some(self.child.id())
    }

    fn try_wait(&mut self) -> std::io::Result<ChildProbe> {
        match self.child.try_wait()? {
            None => Ok(ChildProbe::Running),
            Some(status) => Ok(ChildProbe::Exited {
                code: status.code(),
            }),
        }
    }

    fn stop(&mut self) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            // 组杀成功就足够（子进程本身是组成员）；失败再按单进程兜底。
            if !kill_process_group(self.child.id()) {
                self.child.kill()?;
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            if !taskkill_tree(self.child.id()) {
                self.child.kill()?;
            }
            Ok(())
        }
        #[cfg(not(any(unix, windows)))]
        {
            self.child.kill()
        }
    }

    fn wait(&mut self) -> std::io::Result<()> {
        self.child.wait().map(|_| ())
    }

    fn stderr_tail(&self) -> Option<String> {
        // 先 join：进程刚死时最后一行可能还在读线程的缓冲里。
        self.join_reader();
        let guard = self.tail.lock().unwrap_or_else(|e| e.into_inner());
        if guard.trim().is_empty() {
            None
        } else {
            Some(guard.clone())
        }
    }
}
