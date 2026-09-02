use super::*;

#[test]
fn format_duration_secs_buckets() {
    assert_eq!(format_duration_secs(0), "0s");
    assert_eq!(format_duration_secs(59), "59s");
    assert_eq!(format_duration_secs(60), "1m");
    assert_eq!(format_duration_secs(3599), "59m");
    assert_eq!(format_duration_secs(3600), "1h");
    assert_eq!(format_duration_secs(86_399), "23h");
    assert_eq!(format_duration_secs(86_400), "1d");
    assert_eq!(format_duration_secs(10 * 86_400), "10d");
    assert_eq!(format_duration_secs(999 * 86_400).len(), 4);
}

#[test]
fn format_size_bytes_switches_unit_at_one_mebibyte() {
    assert_eq!(format_size_bytes(0), "0KB");
    assert_eq!(format_size_bytes(203), "0KB");
    assert_eq!(format_size_bytes(1024), "1KB");
    assert_eq!(format_size_bytes(110_592), "108KB");
    assert_eq!(format_size_bytes(1024 * 1024 - 1), "1023KB");
    assert_eq!(format_size_bytes(1024 * 1024), "1.0MB");
    assert_eq!(format_size_bytes(716_975), "700KB");
    assert_eq!(format_size_bytes(4_608_690), "4.4MB");
    assert_eq!(format_size_bytes(43_096_909), "41.1MB");
}

#[test]
fn format_size_bytes_stays_inside_the_column() {
    for bytes in [0, 1024, 1024 * 1024 - 1, 43_096_909, 8 * 1024 * 1024 * 1024] {
        assert!(
            format_size_bytes(bytes).len() <= 8,
            "{bytes} rendered as {}",
            format_size_bytes(bytes)
        );
    }
}

#[test]
fn wrap_index_handles_empty() {
    assert_eq!(wrap_index(0, 0, 1), 0);
    assert_eq!(wrap_index(5, 0, -3), 0);
}

#[test]
fn wrap_index_wraps_both_directions() {
    assert_eq!(wrap_index(0, 4, 1), 1);
    assert_eq!(wrap_index(3, 4, 1), 0); // forward wrap
    assert_eq!(wrap_index(0, 4, -1), 3); // backward wrap
    assert_eq!(wrap_index(2, 4, 5), 3); // big delta
    assert_eq!(wrap_index(2, 4, -7), 3); // big negative delta
}
