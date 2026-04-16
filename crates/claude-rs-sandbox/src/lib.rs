pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }

    #[test]
    fn test_add_boundary_zero() {
        assert_eq!(add(0, 0), 0);
    }

    #[test]
    fn test_add_error_case_not_applicable_still_deterministic() {
        assert_ne!(add(1, 2), 0);
    }
}
