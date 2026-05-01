/// 防休眠 guard：在存在预定/执行任务时阻止系统自动睡眠。
///
/// macOS：spawn `caffeinate -s -i` 子进程，释放时 kill。
/// Windows：`SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)`，
///          用专用后台线程持有，释放时通过 channel 通知线程退出并清除标志。
/// Linux：暂不支持，is_active() 返回 false。
use std::sync::Mutex;

#[cfg(target_os = "windows")]
mod win {
    // kernel32 在 Windows 上由 Rust std 默认链接，无需额外依赖
    extern "system" {
        pub fn SetThreadExecutionState(esflags: u32) -> u32;
    }
    pub const ES_CONTINUOUS: u32 = 0x80000000u32;
    pub const ES_SYSTEM_REQUIRED: u32 = 0x00000001u32;
}

pub struct KeepAwakeGuard {
    state: Mutex<GuardState>,
}

struct GuardState {
    active: bool,
    #[cfg(target_os = "macos")]
    child: Option<std::process::Child>,
    #[cfg(target_os = "windows")]
    release_tx: Option<std::sync::mpsc::SyncSender<()>>,
}

impl KeepAwakeGuard {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(GuardState {
                active: false,
                #[cfg(target_os = "macos")]
                child: None,
                #[cfg(target_os = "windows")]
                release_tx: None,
            }),
        }
    }

    /// 激活防休眠（已激活时幂等）。
    pub fn acquire(&self) {
        let Ok(mut s) = self.state.lock() else { return };
        if s.active {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            match std::process::Command::new("caffeinate")
                .args(["-s", "-i"])
                .spawn()
            {
                Ok(child) => {
                    s.child = Some(child);
                    s.active = true;
                }
                Err(e) => {
                    eprintln!("[keep_awake] caffeinate spawn failed: {e}");
                }
            }
            return;
        }
        #[cfg(target_os = "windows")]
        {
            // 用 sync_channel(0) 做 rendezvous：release() 发送后线程立即清除并退出
            let (tx, rx) = std::sync::mpsc::sync_channel::<()>(0);
            std::thread::spawn(move || {
                unsafe {
                    win::SetThreadExecutionState(win::ES_CONTINUOUS | win::ES_SYSTEM_REQUIRED);
                }
                // 阻塞直到 release() 触发（sender drop 或发送消息）
                let _ = rx.recv();
                unsafe {
                    win::SetThreadExecutionState(win::ES_CONTINUOUS);
                }
            });
            s.release_tx = Some(tx);
            s.active = true;
            return;
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            // Linux 暂未实现
            let _ = s;
        }
    }

    /// 释放防休眠（已释放时幂等）。
    pub fn release(&self) {
        let Ok(mut s) = self.state.lock() else { return };
        if !s.active {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            if let Some(mut child) = s.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        #[cfg(target_os = "windows")]
        {
            // drop sender → 后台线程 recv() 返回 Err，执行清除并退出
            drop(s.release_tx.take());
        }
        s.active = false;
    }

    /// 当前是否已激活防休眠。
    pub fn is_active(&self) -> bool {
        self.state.lock().map(|s| s.active).unwrap_or(false)
    }
}
