use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::cell::Cell;
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;

/// 需要排除的目录名（这些目录通常很大且变化频繁，不需要实时监控）
const EXCLUDED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    ".cache",
    ".npm",
    ".cargo",
    "target",
    "build",
    "dist",
    ".gradle",
    ".m2",
    ".ivy2",
    "__pycache__",
    ".venv",
    "venv",
    ".tox",
    ".eggs",
];

/// 目录规模阈值：超过这个数量的子目录，使用非递归模式
const RECURSIVE_THRESHOLD: usize = 1000;

/// 初始化状态
enum InitState {
    /// 正在初始化
    Pending,
    /// 初始化完成，watcher 已就绪
    Ready(RecommendedWatcher),
    /// 初始化失败
    Failed,
}

pub struct FsWatcher {
    /// watcher 状态（Arc<Mutex> 允许后台线程更新）
    state: Arc<Mutex<InitState>>,
    rx: Receiver<bool>,
    /// 刷新期间暂停事件处理，避免 load_children() 触发的事件导致循环刷新
    paused: Cell<bool>,
}

impl FsWatcher {
    /// 创建一个文件系统监听器（异步初始化）。
    /// 立即返回，后台线程完成 watch 创建，不阻塞 UI。
    pub fn new(root: &Path) -> Option<Self> {
        let (tx, rx) = mpsc::channel();
        let state = Arc::new(Mutex::new(InitState::Pending));
        let state_clone = Arc::clone(&state);

        let root_path = root.to_path_buf();

        // 后台线程：创建 watcher 并添加 watches
        thread::spawn(move || {
            // 创建 watcher
            let mut watcher = match notify::recommended_watcher(
                move |res: notify::Result<notify::Event>| {
                    if let Ok(event) = res {
                        use notify::EventKind::*;
                        // 只监听创建/删除事件（文件树结构变化）。
                        match event.kind {
                            Create(_) | Remove(_) => {
                                let _ = tx.send(true);
                            }
                            _ => {}
                        }
                    }
                },
            ) {
                Ok(w) => w,
                Err(_) => {
                    if let Ok(mut s) = state_clone.lock() {
                        *s = InitState::Failed;
                    }
                    return;
                }
            };

            // 智能判断：根据目录规模选择监控模式
            let mode = if should_use_recursive(&root_path) {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };

            // 添加 watch（忽略错误，失败时只是没有自动刷新功能）
            let _ = watcher.watch(&root_path, mode);

            // 更新状态为就绪
            if let Ok(mut s) = state_clone.lock() {
                *s = InitState::Ready(watcher);
            }
        });

        Some(Self {
            state,
            rx,
            paused: Cell::new(false),
        })
    }

    /// 检查 watcher 是否初始化完成
    pub fn is_ready(&self) -> bool {
        self.state
            .lock()
            .map(|s| matches!(*s, InitState::Ready(_)))
            .unwrap_or(false)
    }

    /// 暂停事件处理（在 refresh 前调用）
    pub fn pause(&self) {
        self.paused.set(true);
    }

    /// 恢复事件处理，并清空暂停期间积压的事件
    pub fn resume(&self) {
        self.paused.set(false);
        // 清空暂停期间可能积压的事件
        while self.rx.try_recv().is_ok() {}
    }

    /// 消费所有待处理的刷新信号，返回是否需要刷新。
    /// 暂停期间返回 false。
    pub fn drain_refresh(&self) -> bool {
        if self.paused.get() {
            return false;
        }
        let mut found = false;
        while self.rx.try_recv().is_ok() {
            found = true;
        }
        found
    }
}

/// 智能判断是否应该使用递归模式
/// 考虑因素：
/// 1. 是否是用户主目录（通常很大）
/// 2. 第一层子目录数量
/// 3. 总子目录数量（限制统计深度，避免太慢）
fn should_use_recursive(root: &Path) -> bool {
    // 规则 1：如果是用户主目录，使用非递归模式
    if let Ok(home) = std::env::var("HOME") {
        if root == Path::new(&home) {
            return false;
        }
    }

    // 规则 2：快速检查第一层子目录数量
    let mut first_level_count = 0;
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // 排除已知的大目录
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if EXCLUDED_DIRS.contains(&name) {
                        continue;
                    }
                }
                first_level_count += 1;
                // 第一层超过 50 个目录，很可能是大目录树
                if first_level_count > 50 {
                    return false;
                }
            }
        }
    }

    // 规则 3：统计总子目录数量（限制最大统计数量，避免遍历太深）
    let total_count = count_subdirs(root, RECURSIVE_THRESHOLD * 2);
    total_count <= RECURSIVE_THRESHOLD
}

/// 统计目录下的子目录数量（排除已知大目录，限制最大统计数量）
fn count_subdirs(root: &Path, max_count: usize) -> usize {
    let mut count = 0;
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        if count >= max_count {
            break;
        }

        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // 排除已知的大目录
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if EXCLUDED_DIRS.contains(&name) {
                            continue;
                        }
                    }
                    count += 1;
                    stack.push(path);
                }
            }
        }
    }

    count
}
