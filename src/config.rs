use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct Template {
    pub name: String,
    pub command: String,
}

#[derive(Serialize, Deserialize)]
pub struct TemplatesFile {
    pub templates: Vec<Template>,
}

pub fn config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/ftree/templates.toml")
}

pub fn default_templates() -> Vec<Template> {
    vec![
        Template {
            name: "ffmpeg 转码 h264".to_string(),
            command: "ffmpeg -i {files_quoted} -c:v libx264 -c:a aac {dir}/out.mp4".to_string(),
        },
        Template {
            name: "cat 合并".to_string(),
            command: "cat {files} > {dir}/merged.bin".to_string(),
        },
        Template {
            name: "tar 打包".to_string(),
            command: "tar -czf {dir}/archive.tar.gz {files}".to_string(),
        },
        Template {
            name: "scp 上传".to_string(),
            command: "scp {files} user@host:/remote/path/".to_string(),
        },
        Template {
            name: "git add".to_string(),
            command: "git add {files}".to_string(),
        },
    ]
}

/// 读取模板配置；不存在时写入默认配置并返回。
pub fn load_templates() -> Vec<Template> {
    let path = config_path();
    if !path.exists() {
        let defaults = default_templates();
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(text) = toml::to_string(&TemplatesFile {
            templates: defaults.clone(),
        }) {
            let _ = fs::write(&path, text);
        }
        return defaults;
    }
    match fs::read_to_string(&path)
        .ok()
        .and_then(|s| toml::from_str::<TemplatesFile>(&s).ok())
    {
        Some(f) if !f.templates.is_empty() => f.templates,
        _ => default_templates(), // 配置损坏时兜底
    }
}