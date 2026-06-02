#[test]
fn active_view_len_uses_shorter_view() {
    assert_eq!(bun_rab_mini::active_view_len(16, 8), 8);
}
