/// Estimate tokens for premium↔agent bridge payloads.
///
/// # ponytail: chars/4; swap for a real tokenizer if Cursor ever exposes usage
pub fn estimate_tokens(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    (text.chars().count() as u64).div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_zero() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn four_chars_is_one_token() {
        assert_eq!(estimate_tokens("abcd"), 1);
    }

    #[test]
    fn five_chars_ceils_to_two() {
        assert_eq!(estimate_tokens("abcde"), 2);
    }

    #[test]
    fn single_char_is_one_token() {
        assert_eq!(estimate_tokens("a"), 1);
    }

    #[test]
    fn three_chars_ceils_to_one() {
        assert_eq!(estimate_tokens("abc"), 1);
    }

    #[test]
    fn eight_chars_is_two_tokens() {
        assert_eq!(estimate_tokens("abcdefgh"), 2);
    }

    #[test]
    fn unicode_single_char_is_one_token() {
        assert_eq!(estimate_tokens("\u{00e9}"), 1);
    }

    #[test]
    fn unicode_multi_byte_chars_count_individually() {
        assert_eq!(estimate_tokens("\u{00e9}\u{20ac}\u{03bb}"), 1);
    }

    #[test]
    fn whitespace_only_counts_chars() {
        assert_eq!(estimate_tokens("   "), 1);
    }

    #[test]
    fn newlines_count_as_chars() {
        assert_eq!(estimate_tokens("\n"), 1);
    }

    #[test]
    fn zero_chars_is_exactly_zero() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn large_input_correct_ceiling() {
        let input = "x".repeat(1000);
        assert_eq!(estimate_tokens(&input), 250);
    }

    #[test]
    fn one_below_multiple_ceils_up() {
        assert_eq!(estimate_tokens("ab"), 1);
    }

    #[test]
    fn one_above_multiple_ceils_up() {
        assert_eq!(estimate_tokens("abcde"), 2);
    }
}
