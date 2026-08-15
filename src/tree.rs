use std::fs;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeKind {
    Dir,
    File,
}

#[derive(Clone)]
pub struct Node {
    pub kind: NodeKind,
    pub name: String,
    pub path: PathBuf,
    pub expanded: bool,
    pub loaded: bool,
    pub children: Vec<Node>,
}

impl Node {
    fn new(kind: NodeKind, path: PathBuf) -> Self {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        Node {
            kind,
            name,
            path,
            expanded: false,
            loaded: false,
            children: Vec::new(),
        }
    }

    pub fn is_dir(&self) -> bool {
        self.kind == NodeKind::Dir
    }

    /// 懒加载：读取子目录/文件，目录优先、按名称排序。
    /// 已加载过则直接返回（隐藏文件的过滤发生在 load 时）。
    pub fn load_children(&mut self, show_hidden: bool) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        if let Ok(rd) = fs::read_dir(&self.path) {
            for entry in rd.flatten() {
                let path = entry.path();
                let fname = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if !show_hidden && fname.starts_with('.') {
                    continue;
                }
                let kind = if path.is_dir() {
                    NodeKind::Dir
                } else {
                    NodeKind::File
                };
                let n = Node::new(kind, path);
                match kind {
                    NodeKind::Dir => dirs.push(n),
                    NodeKind::File => files.push(n),
                }
            }
        }
        dirs.sort_by(|a, b| a.name.cmp(&b.name));
        files.sort_by(|a, b| a.name.cmp(&b.name));
        self.children = dirs.into_iter().chain(files).collect();
    }
}

/// 目录树 + 可见行索引。
/// visible 中每条记录是从根（不含）到该节点的索引链，用于 `node_at` 定位。
pub struct Tree {
    pub root: Node,
    pub visible: Vec<Vec<usize>>,
    pub cursor: usize,
    pub scroll: usize,
    pub show_hidden: bool,
}

fn visit(n: &Node, chain: &mut Vec<usize>, out: &mut Vec<Vec<usize>>, show_hidden: bool) {
    out.push(chain.clone());
    if n.is_dir() && n.expanded {
        for (i, c) in n.children.iter().enumerate() {
            if !show_hidden && c.name.starts_with('.') {
                continue;
            }
            chain.push(i);
            visit(c, chain, out, show_hidden);
            chain.pop();
        }
    }
}

impl Tree {
    pub fn new(root: PathBuf, show_hidden: bool) -> Self {
        let mut root_node = Node::new(NodeKind::Dir, root);
        root_node.expanded = true; // 根目录默认展开
        let mut t = Tree {
            root: root_node,
            visible: Vec::new(),
            cursor: 0,
            scroll: 0,
            show_hidden,
        };
        if t.root.is_dir() && !t.root.loaded {
            t.root.load_children(t.show_hidden);
        }
        t.rebuild();
        t
    }

    pub fn rebuild(&mut self) {
        self.visible.clear();
        visit(&self.root, &mut Vec::new(), &mut self.visible, self.show_hidden);
        self.cursor = self.cursor.min(self.visible.len().saturating_sub(1));
    }

    pub fn node_at(&self, chain: &[usize]) -> &Node {
        let mut n = &self.root;
        for &i in chain {
            n = &n.children[i];
        }
        n
    }

    pub fn node_at_mut(&mut self, chain: &[usize]) -> &mut Node {
        let mut n = &mut self.root;
        for &i in chain {
            n = &mut n.children[i];
        }
        n
    }

    /// 展开/收缩指定行的目录节点。返回是否发生了切换。
    pub fn toggle_row(&mut self, row: usize) -> bool {
        let chain = match self.visible.get(row) {
            Some(c) => c.clone(),
            None => return false,
        };
        if !self.node_at(&chain).is_dir() {
            return false;
        }
        let show_hidden = self.show_hidden;
        let n = self.node_at_mut(&chain);
        if !n.loaded {
            n.load_children(show_hidden);
        }
        n.expanded = !n.expanded;
        self.rebuild();
        true
    }

    pub fn toggle_cursor(&mut self) {
        self.toggle_row(self.cursor);
    }

    pub fn cursor_node(&self) -> &Node {
        let chain = self.visible.get(self.cursor).cloned().unwrap_or_default();
        self.node_at(&chain)
    }

    /// 当前行对应的"文件夹"：目录行 = 自身；文件行 = 父目录。
    pub fn cursor_dir(&self) -> PathBuf {
        let n = self.cursor_node();
        if n.is_dir() {
            n.path.clone()
        } else {
            n.path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| n.path.clone())
        }
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let last = self.visible.len().saturating_sub(1);
        self.cursor = (self.cursor as isize + delta).clamp(0, last as isize) as usize;
    }

    /// 滚轮滚动：直接滚动内容，光标保持在视口内
    pub fn scroll_by(&mut self, delta: isize, view_h: usize) {
        let rows = self.visible.len();
        if rows == 0 {
            return;
        }
        let view_h = view_h.max(1);
        // 调整 scroll
        let new_scroll = (self.scroll as isize + delta).clamp(0, rows.saturating_sub(view_h) as isize);
        self.scroll = new_scroll as usize;
        // 确保 cursor 在视口内
        if self.cursor < self.scroll {
            self.cursor = self.scroll;
        } else if self.cursor >= self.scroll + view_h {
            self.cursor = self.scroll + view_h - 1;
        }
    }

    /// 返回上级：当前行是展开的目录则先收缩，否则跳到父行。
    pub fn collapse_up(&mut self) {
        let chain = self.visible.get(self.cursor).cloned().unwrap_or_default();
        if chain.is_empty() {
            return;
        }
        let n = self.node_at(&chain);
        if n.is_dir() && n.expanded {
            self.toggle_row(self.cursor);
            return;
        }
        let parent = chain[..chain.len() - 1].to_vec();
        if let Some(pos) = self.visible.iter().position(|c| c == &parent) {
            self.cursor = pos;
        }
    }

    /// 滚动跟随光标，保证光标行可见。
    pub fn ensure_visible(&mut self, view_h: usize) {
        let rows = self.visible.len();
        if rows == 0 {
            self.scroll = 0;
            return;
        }
        let view_h = view_h.max(1);
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        }
        if rows > view_h && self.cursor >= self.scroll + view_h {
            self.scroll = self.cursor + 1 - view_h;
        }
        if self.scroll + view_h > rows {
            self.scroll = rows.saturating_sub(view_h);
        }
        let max_scroll = rows.saturating_sub(1);
        self.scroll = self.scroll.min(max_scroll);
    }

    /// 刷新：重新读取所有已展开目录的子项（用于检测外部新增/删除的文件）。
    pub fn refresh(&mut self) {
        fn reload_node(n: &mut Node, show_hidden: bool) {
            if n.is_dir() && n.expanded {
                n.loaded = false;
                n.children.clear();
                n.load_children(show_hidden);
                for c in &mut n.children {
                    reload_node(c, show_hidden);
                }
            }
        }
        reload_node(&mut self.root, self.show_hidden);
        self.rebuild();
    }

    /// 切换隐藏文件显示；重载整棵已展开树（保留展开状态）。
    pub fn flip_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        fn rebuild_node(n: &mut Node, show: bool) {
            if n.is_dir() {
                n.loaded = false;
                n.children.clear();
                if n.expanded {
                    n.load_children(show);
                    for c in &mut n.children {
                        rebuild_node(c, show);
                    }
                }
            }
        }
        rebuild_node(&mut self.root, self.show_hidden);
        self.rebuild();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

    /// 每个测试独享 fixture 目录，避免并行测试互相干扰。
    fn fixture_tree() -> (PathBuf, Tree) {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!("ftree-test-tree-{id}"));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("sub/deep")).unwrap();
        fs::write(base.join("a.mp4"), b"").unwrap();
        fs::write(base.join("b.mp4"), b"").unwrap();
        fs::write(base.join("sub/c.txt"), b"").unwrap();
        fs::write(base.join(".hidden"), b"").unwrap();
        let tree = Tree::new(base.clone(), false);
        (base, tree)
    }

    #[test]
    fn visible_rows_order() {
        let (base, mut tree) = fixture_tree();
        // 根、sub（目录优先）、a.mp4、b.mp4（隐藏文件被过滤）
        assert_eq!(tree.visible.len(), 4);
        assert_eq!(tree.node_at(&tree.visible[0]).kind, NodeKind::Dir);
        assert_eq!(tree.node_at(&tree.visible[1]).name, "sub");
        assert_eq!(tree.node_at(&tree.visible[2]).name, "a.mp4");
        // 子目录懒加载前不可见
        assert!(tree.visible.iter().all(|c| c.len() <= 1));
        // 展开 sub（sub 下还有 deep 目录，目录优先）
        tree.move_cursor(1); // sub
        tree.toggle_cursor();
        assert_eq!(tree.visible.len(), 6); // 根+sub+deep+c.txt+a.mp4+b.mp4
        assert_eq!(tree.node_at(&tree.visible[1]).name, "sub");
        assert_eq!(tree.node_at(&tree.visible[2]).name, "deep");
        assert_eq!(tree.node_at(&tree.visible[3]).name, "c.txt");
        let _ = base;
    }

    #[test]
    fn flip_hidden_reloads() {
        let (_, mut tree) = fixture_tree();
        assert_eq!(tree.visible.len(), 4);
        tree.flip_hidden();
        // 根 + sub + a.mp4 + b.mp4 + .hidden（sub 未展开，其子项不可见）
        assert_eq!(tree.visible.len(), 5);
        let names: Vec<String> = tree
            .visible
            .iter()
            .map(|c| tree.node_at(c).name.clone())
            .collect();
        assert!(names.contains(&".hidden".to_string()));
    }

    #[test]
    fn cursor_dir_for_file() {
        let (base, tree) = fixture_tree();
        // 文件 a.mp4 的父目录 = base
        assert_eq!(tree.cursor_dir(), base);
    }

    #[test]
    fn expand_collapse_keeps_expanded_state() {
        let (_base, mut tree) = fixture_tree();
        tree.move_cursor(1); // sub
        tree.toggle_cursor();
        assert_eq!(tree.visible.len(), 6);
        tree.move_cursor(-(tree.cursor as isize)); // 回到根行
        tree.toggle_cursor(); // 收起根
        assert_eq!(tree.visible.len(), 1);
        tree.toggle_cursor(); // 再展开根
        assert_eq!(tree.visible.len(), 6); // sub 仍是展开状态
    }
}
