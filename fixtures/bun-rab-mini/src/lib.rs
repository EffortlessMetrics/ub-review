pub fn active_view_len(backing_len: usize, view_len: usize) -> usize {
    backing_len.min(view_len)
}
