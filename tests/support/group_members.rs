use dejadoc::Group;

/// The sorted item names of one group.
fn sorted_items(group: &Group) -> Vec<&str> {
    let mut items: Vec<_> = group.sites.iter().map(|site| site.item.as_str()).collect();
    items.sort_unstable();
    items
}

/// The sorted item lists of the given groups.
pub(super) fn members<'a, Groups: IntoIterator<Item = &'a Group>>(
    groups: Groups,
) -> Vec<Vec<&'a str>> {
    let mut out: Vec<Vec<&str>> = groups.into_iter().map(sorted_items).collect();
    out.sort_unstable();
    out
}
