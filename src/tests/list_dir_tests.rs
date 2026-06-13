use crate::agent::tools::format_size;

#[test]
fn format_0_bytes() {
    assert_eq!(format_size(0), "0 B");
}

#[test]
fn format_1_byte() {
    assert_eq!(format_size(1), "1 B");
}

#[test]
fn format_1023_bytes() {
    assert_eq!(format_size(1023), "1023 B");
}

#[test]
fn format_1024_bytes_1_kb() {
    assert_eq!(format_size(1024), "1.0 KB");
}

#[test]
fn format_1536_bytes() {
    assert_eq!(format_size(1536), "1.5 KB");
}

#[test]
fn format_1048576_bytes_1_mb() {
    assert_eq!(format_size(1048576), "1.0 MB");
}

#[test]
fn format_1073741824_bytes_1_gb() {
    assert_eq!(format_size(1073741824), "1.0 GB");
}

#[test]
fn format_above_gb_remains_gb() {
    // 2 TB
    assert_eq!(format_size(2_199_023_255_552), "2048.0 GB");
}

#[test]
fn format_512_bytes() {
    assert_eq!(format_size(512), "512 B");
}

#[test]
fn format_2048_bytes() {
    assert_eq!(format_size(2048), "2.0 KB");
}

#[test]
fn format_2560_bytes() {
    assert_eq!(format_size(2560), "2.5 KB");
}

#[test]
fn format_large_kb() {
    assert_eq!(format_size(1_047_552), "1023.0 KB");
}

use crate::agent::tools::count_dir_entries;

#[test]
fn count_empty_dir() {
    let dir = std::env::temp_dir().join(format!("zs_empty_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(count_dir_entries(&dir), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn count_dir_with_files() {
    let dir = std::env::temp_dir().join(format!("zs_files_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), b"a").unwrap();
    std::fs::write(dir.join("b.txt"), b"b").unwrap();
    assert_eq!(count_dir_entries(&dir), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn count_nonexistent_dir() {
    assert_eq!(
        count_dir_entries(std::path::Path::new("/nonexistent_xyz_test")),
        0
    );
}
