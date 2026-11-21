//! 配置文件读写： `%LOCALAPPDATA%\GXNUCampusLogin\config.json`。

use crate::credentials::StoredCredential;
use crate::logging;
use crate::models::Settings;
use std::io::{self, Write};
use std::path::PathBuf;

pub const APP_DIR_NAME: &str = "GXNUCampusLogin";

pub fn default_app_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(APP_DIR_NAME)
}

pub struct SettingsStore {
    pub app_dir: PathBuf,
    pub path: PathBuf,
}

impl SettingsStore {
    pub fn new() -> Self {
        let app_dir = default_app_dir();
        let path = app_dir.join("config.json");
        Self { app_dir, path }
    }

    /// 读取失败不崩溃：返回默认值并记一条警告。
    pub fn load(&self) -> Settings {
        self.load_strict().unwrap_or_else(|err| {
            if err.kind() != io::ErrorKind::NotFound {
                logging::warn(&format!("[CONFIG_ERROR] 读取配置失败，使用默认值：{err}"));
            }
            Settings::default().normalized()
        })
    }

    pub fn load_strict(&self) -> io::Result<Settings> {
        let text = std::fs::read_to_string(&self.path)?;
        if !text.trim_start().starts_with('{') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "配置应为 JSON 对象",
            ));
        }
        Ok(serde_json::from_str::<Settings>(&text)?.normalized())
    }

    fn stage(&self, settings: &Settings) -> io::Result<tempfile::NamedTempFile> {
        std::fs::create_dir_all(&self.app_dir)?;
        let mut file = tempfile::NamedTempFile::new_in(&self.app_dir)?;
        serde_json::to_writer_pretty(file.as_file_mut(), &settings.normalized())?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        Ok(file)
    }

    pub fn save_account(
        &self,
        settings: &Settings,
        previous_id: &str,
        password: &str,
        stored: Option<&StoredCredential>,
        mut write_credential: impl FnMut(Option<&StoredCredential>) -> Result<(), String>,
    ) -> Result<bool, String> {
        let account = settings.student_id.trim();
        if password.is_empty() {
            let matches =
                stored.is_some_and(|cred| cred.username == account && !cred.password.is_empty());
            if !matches && (account != previous_id || stored.is_some()) {
                return Err("更换学号时请同时输入该账号的密码".into());
            }
        } else if account.is_empty() {
            return Err("保存密码前请输入学号".into());
        }

        let file = self
            .stage(settings)
            .map_err(|err| format!("准备配置失败：{err}"))?;
        if !password.is_empty() {
            let new = StoredCredential {
                username: account.to_owned(),
                password: password.to_owned(),
            };
            write_credential(Some(&new))?;
        }
        if let Err(err) = file.persist(&self.path) {
            if !password.is_empty() {
                write_credential(stored).map_err(|rollback| {
                    format!("提交配置失败，恢复原凭据也失败，请重新保存账号和密码：{rollback}")
                })?;
            }
            return Err(format!("提交配置失败：{}", err.error));
        }
        Ok(!password.is_empty()
            && !stored.is_some_and(|cred| cred.username == account && cred.password == password))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_accounts_never_commits_half_an_update() {
        let root = tempfile::tempdir().unwrap();
        let store = SettingsStore {
            app_dir: root.path().into(),
            path: root.path().join("config.json"),
        };
        let first = Settings {
            student_id: "fixture-a".into(),
            ..Settings::default()
        };
        let second = Settings {
            student_id: "fixture-b".into(),
            ..first.clone()
        };
        let mut saved = Some(StoredCredential {
            username: first.student_id.clone(),
            password: "fake-old".into(),
        });
        store
            .save_account(&first, "", "", saved.as_ref(), |_| {
                panic!("unchanged password")
            })
            .unwrap();
        assert!(store
            .save_account(&second, &first.student_id, "", saved.as_ref(), |_| panic!(
                "wrong account"
            ))
            .is_err());
        assert!(store
            .save_account(
                &second,
                &first.student_id,
                "fake-new",
                saved.as_ref(),
                |_| Err("mock write failed".into())
            )
            .is_err());
        assert_eq!(store.load().student_id, first.student_id);

        let old = saved.clone();
        let blocked = SettingsStore {
            app_dir: root.path().into(),
            path: root.path().to_owned(),
        };
        assert!(blocked
            .save_account(
                &second,
                &first.student_id,
                "fake-new",
                old.as_ref(),
                |value| {
                    saved = value.cloned();
                    Ok(())
                }
            )
            .is_err());
        assert!(saved.as_ref().unwrap().matches_account(&first.student_id));
        store
            .save_account(
                &second,
                &first.student_id,
                "fake-new",
                old.as_ref(),
                |value| {
                    saved = value.cloned();
                    Ok(())
                },
            )
            .unwrap();
        assert!(saved.unwrap().matches_account(&store.load().student_id));
    }
}
