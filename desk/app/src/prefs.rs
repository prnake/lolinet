//! 记住的偏好：`<配置目录>/lolidesk/app.json`。字段和旧版（Go）的一样，旧文件照样读。

use std::path::{Path, PathBuf};

use lolidesk_proto::{Quality, VideoSettings};
use serde::{Deserialize, Serialize};

use crate::labels::{quality_from_index, quality_index};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// 编码画面的长边，0 = 原生
    pub long_edge: u32,
    pub fps: u8,
    /// 0 省流、1 标准、2 清晰、3 极清
    pub quality: u8,
    /// 终端字号
    pub font_size: f32,
    /// 终端里 Option 当 Alt（Option+x 发 ESC x），不用它打特殊字符
    pub option_as_alt: bool,
    /// 让网里的设备能连这台：本机主机没在跑时，app 替你开（`lolidesk host install`）
    pub local_host: bool,
    /// 关掉 local_host 前服务跑的程序：再开时还装它（授权跟着程序走，别换成 app 带的）
    pub local_host_program: Option<PathBuf>,
}

impl Default for Prefs {
    fn default() -> Self {
        let v = VideoSettings::default();
        Self {
            long_edge: v.long_edge,
            fps: v.fps,
            quality: quality_index(v.quality),
            font_size: FONT_DEFAULT,
            option_as_alt: true,
            local_host: true,
            local_host_program: None,
        }
    }
}

pub const FONT_DEFAULT: f32 = 13.0;
pub const FONT_MIN: f32 = 9.0;
pub const FONT_MAX: f32 = 28.0;

impl Prefs {
    pub fn path() -> Option<PathBuf> {
        Some(dirs::config_dir()?.join("lolidesk").join("app.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .map(|p| Self::load_from(&p))
            .unwrap_or_default()
    }

    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Prefs>(&s).ok())
            .map(Prefs::sanitized)
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(p) = Self::path()
            && let Err(e) = self.save_to(&p)
        {
            tracing::warn!("存偏好 {}：{e:#}", p.display());
        }
    }

    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// 读进来的值夹回能用的范围。
    pub fn sanitized(mut self) -> Self {
        if self.fps == 0 {
            self.fps = VideoSettings::default().fps;
        }
        self.quality = self.quality.min(3);
        if !self.font_size.is_finite() || self.font_size <= 0.0 {
            self.font_size = 13.0;
        }
        self.font_size = self.font_size.clamp(FONT_MIN, FONT_MAX);
        self
    }

    pub fn quality(&self) -> Quality {
        quality_from_index(self.quality)
    }

    pub fn video(&self) -> VideoSettings {
        VideoSettings {
            long_edge: self.long_edge,
            fps: self.fps,
            quality: self.quality(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_old_file_and_roundtrips() {
        let dir = std::env::temp_dir().join(format!("lolidesk-prefs-{}", std::process::id()));
        let path = dir.join("app.json");
        std::fs::create_dir_all(&dir).unwrap();
        // 旧版写的：字号是浮点，画质是整数
        std::fs::write(
            &path,
            r#"{"long_edge": 2560, "fps": 120, "quality": 3, "font_size": 14}"#,
        )
        .unwrap();
        let p = Prefs::load_from(&path);
        assert_eq!(
            (p.long_edge, p.fps, p.quality, p.font_size),
            (2560, 120, 3, 14.0)
        );
        assert_eq!(p.video().quality, Quality::Ultra);
        // 旧文件没有 local_host：按开算
        assert!(p.local_host);

        let q = Prefs {
            long_edge: 0,
            local_host: false,
            ..p.clone()
        };
        q.save_to(&path).unwrap();
        assert_eq!(Prefs::load_from(&path), q);

        // 坏文件、缺字段、怪值
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(Prefs::load_from(&path), Prefs::default());
        std::fs::write(&path, r#"{"fps": 0, "quality": 9, "font_size": 300}"#).unwrap();
        let p = Prefs::load_from(&path);
        assert_eq!((p.fps, p.quality, p.font_size), (60, 3, FONT_MAX));
        assert_eq!(p.long_edge, 1920);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
