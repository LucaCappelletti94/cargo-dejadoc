/// Every ordering of 0 through n-1.
pub(super) fn permutations(n: usize) -> Vec<Vec<usize>> {
    fn extend(n: usize, prefix: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        if prefix.len() == n {
            out.push(prefix.clone());
            return;
        }
        for next in 0..n {
            if !prefix.contains(&next) {
                prefix.push(next);
                extend(n, prefix, out);
                prefix.pop();
            }
        }
    }
    let mut out = Vec::new();
    extend(n, &mut Vec::new(), &mut out);
    out
}
