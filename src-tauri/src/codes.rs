/// Strip only the ASCII prefix, preserving the encoded payload byte-for-byte.
pub(crate) fn strip_prefix<'a>(code: &'a str, prefix: &str) -> Option<&'a str> {
    let code = code.trim();
    let head = code.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &code[prefix.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_preserve_payload() {
        for prefix in ["dropbeam:", "dropbeamf1:", "dropbeam1:", "direct"] {
            let payload = "aB_z09X";
            assert_eq!(strip_prefix(&format!(" {}{}\n", prefix.to_ascii_uppercase(), payload), prefix), Some(payload));
        }
        assert_eq!(strip_prefix("💡", "direct"), None);
        assert_eq!(strip_prefix("other:AbC", "dropbeam:"), None);
    }
}
