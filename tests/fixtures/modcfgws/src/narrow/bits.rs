/// Sum of the doubled values over a limit.
pub fn spread(values: &[u32], limit: u32) -> u32 {
    let mut total = 0;
    for value in values {
        if *value > limit {
            total += value * 2;
        } else {
            total -= 1;
        }
    }
    let shifted = total + limit;
    shifted
}
