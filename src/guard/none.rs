//! 其余平台的占位实现：**明确不可用**，不是"静默不生效"。
//!
//! 现有构建目标只有 Linux / Android / macOS / Windows 四族，走到这里说明
//! 出现了第五种平台。它不能假装防护生效，因为那会让"防护已启用"变成一句
//! 空话——真正的失误不是不支持，而是支持与不支持看起来一样。

use crate::guard::Support;

pub fn support() -> Support {
    Support {
        os: std::env::consts::OS,
        functional: false,
        detail: "本平台尚无截断防护实现；截断防护不生效，请勿对 abook 输出使用截断管道",
    }
}

pub fn stdout_pipe() -> Option<String> {
    None
}

pub fn stderr_is_stdout() -> bool {
    true
}

pub fn redirect_stdout_to_stderr() -> bool {
    false
}

pub fn find_truncator(_pipe: &str, _me: &str) -> Option<String> {
    None
}
