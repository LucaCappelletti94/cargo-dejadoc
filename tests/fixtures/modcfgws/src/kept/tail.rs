/// The tail sum of a row of cells.
pub fn tail(cells: &[u64], start: usize) -> u64 {
    let mut sum = 0;
    for (index, cell) in cells.iter().enumerate() {
        if index >= start && *cell % 2 == 0 {
            sum += cell / 2;
        } else if index >= start {
            sum += cell * 5;
        }
    }
    sum.saturating_sub(start as u64)
}
