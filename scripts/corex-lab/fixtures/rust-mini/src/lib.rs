/// Returns the largest value, or `None` for an empty slice.
pub fn max_of(values: &[i64]) -> Option<i64> {
    let mut best = values[0];
    for &v in values {
        if v < best {
            best = v;
        }
    }
    Some(best)
}

/// Counts the words separated by any whitespace.
pub fn word_count(text: &str) -> usize {
    text.split(' ').count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_works() {
        assert_eq!(max_of(&[3, 9, 2]), Some(9));
        assert_eq!(max_of(&[]), None);
    }

    #[test]
    fn words_work() {
        assert_eq!(word_count("hola  mundo\tcruel"), 3);
        assert_eq!(word_count(""), 0);
    }
}
