//! Function duplicates the integration tests and the dogfood job pin.

#[cfg(windows)]
mod platform;

/// Sum of the doubled values over a limit.
pub fn total_over(values: &[u32], limit: u32) -> u32 {
    let mut total = 0;
    for value in values {
        if *value > limit {
            total += value * 2;
        } else {
            total -= 1;
        }
    }
    let doubled = total + limit;
    doubled
}

/// The same body under another name.
#[inline]
fn sum_over(values: &[u32], limit: u32) -> u32 {
    let mut total = 0;
    for value in values {
        if *value > limit {
            total += value * 2;
        } else {
            total -= 1;
        }
    }
    let doubled = total + limit;
    doubled
}

/// The same body again, public API this time.
pub fn total_too(values: &[u32], limit: u32) -> u32 {
    let mut total = 0;
    for value in values {
        if *value > limit {
            total += value * 2;
        } else {
            total -= 1;
        }
    }
    let doubled = total + limit;
    doubled
}

// dejadoc: allow
pub fn kept_on_purpose(values: &[u32], limit: u32) -> u32 {
    let mut total = 0;
    for value in values {
        if *value > limit {
            total += value * 2;
        } else {
            total -= 1;
        }
    }
    let doubled = total + limit;
    doubled
}

/// Two nested copies are no sites.
pub fn outer() -> u32 {
    fn first(values: &[u32], limit: u32) -> u32 {
        let mut total = 0;
        for value in values {
            if *value > limit {
                total += value * 2;
            } else {
                total -= 1;
            }
        }
        let doubled = total + limit;
        doubled
    }
    fn second(values: &[u32], limit: u32) -> u32 {
        let mut total = 0;
        for value in values {
            if *value > limit {
                total += value * 2;
            } else {
                total -= 1;
            }
        }
        let doubled = total + limit;
        doubled
    }
    first(&[1], 0) + second(&[2], 0) + sum_over(&[3], 0)
}

pub mod render {
    pub struct Alpha;
    pub struct Beta;

    impl Alpha {
        /// Comma separated values.
        pub fn render(&self, values: &[u32]) -> String {
        let mut out = String::new();
        for value in values {
            out.push_str(&value.to_string());
            out.push(',');
        }
        out.pop();
        out.push('!');
        out
    }
    }

    impl Beta {
        pub fn render(&self, values: &[u32]) -> String {
        let mut out = String::new();
        for value in values {
            out.push_str(&value.to_string());
            out.push(',');
        }
        out.pop();
        out.push('!');
        out
    }
    }
}

pub mod scale {
    /// Scaled values on Unix.
    #[cfg(unix)]
    pub fn scaled(values: &[u32], factor: u32) -> Vec<u32> {
        let mut out = Vec::new();
        for value in values {
            out.push(value * factor + 1);
        }
        out.sort_unstable();
        out.dedup();
        out.reverse();
        out
    }

    /// The same body on every other platform.
    #[cfg(not(unix))]
    pub fn scaled_elsewhere(values: &[u32], factor: u32) -> Vec<u32> {
        let mut out = Vec::new();
        for value in values {
            out.push(value * factor + 1);
        }
        out.sort_unstable();
        out.dedup();
        out.reverse();
        out
    }
}

pub mod describe {
    pub struct Gamma;

    pub trait Short {
        fn short(&self, values: &[u32]) -> String;
    }

    pub trait Long {
        fn long(&self, values: &[u32]) -> String;
    }

    impl Short for Gamma {
        fn short(&self, values: &[u32]) -> String {
            let mut out = String::from("[");
            for value in values {
                out.push_str(&value.to_string());
                out.push(';');
            }
            out.push(']');
            out
        }
    }

    impl Long for Gamma {
        fn long(&self, values: &[u32]) -> String {
            let mut out = String::from("[");
            for value in values {
                out.push_str(&value.to_string());
                out.push(';');
            }
            out.push(']');
            out
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn totals_small_values() {
        let values = [1, 2, 3];
        let total = super::total_over(&values, 1);
        assert_eq!(total, 9);
        assert!(total > 0);
        assert!(total < 100);
    }

    #[test]
    fn totals_large_values() {
        let values = [1, 2, 3];
        let total = super::total_over(&values, 1);
        assert_eq!(total, 9);
        assert!(total > 0);
        assert!(total < 100);
    }
}
