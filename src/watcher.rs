use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::cell::Cell;
use std::path::Path;
use std::sync::mpsc::{self, Receiver};

pub struct FsWatcher {
    _watcher: RecommendedWatcher,
    rx: Receiver<bool>,
    /// 刷新期间暂停事件处理，避免 load_children() 触发的事件导致循环刷新
    paused: Cell<bool>,
}

impl FsWatcher {
    /// 创建一个文件系统监听器，监听 root 目录的递归变化。
    pub fn new(root: &Path) -> Option<Self> {
        let (tx, rx) = mpsc::channel();

        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
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
        })
        .ok()?;

        watcher.watch(root, RecursiveMode::Recursive).ok()?;

        Some(Self {
            _watcher: watcher,
            rx,
            paused: Cell::new(false),
        })
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
