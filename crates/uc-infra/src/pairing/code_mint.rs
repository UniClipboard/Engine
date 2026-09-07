//! 离线邀请码生成，与在线目录申请统一使用六位数字和 `XXX-XXX` 展示格式。
//! 邀请码用于限时发现，成员准入仍由后续认证流程验证。

use rand::Rng;

pub(crate) const INVITATION_CODE_LENGTH: usize = 6;

/// 使用密码学安全的线程随机源生成六位数字，保留前导零。
pub fn mint_invitation_code() -> String {
    let mut rng = rand::rng();
    let mut out = String::with_capacity(INVITATION_CODE_LENGTH + 1);
    for i in 0..INVITATION_CODE_LENGTH {
        if i == INVITATION_CODE_LENGTH / 2 {
            out.push('-');
        }
        out.push(char::from(b'0' + rng.random_range(0..10)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minted_code_is_six_numeric_digits() {
        for _ in 0..100 {
            let code = mint_invitation_code();
            let (left, right) = code.split_once('-').expect("group separator");
            assert_eq!(left.len(), 3);
            assert_eq!(right.len(), 3);
            assert!(left
                .bytes()
                .chain(right.bytes())
                .all(|b| b.is_ascii_digit()));
        }
    }
}
