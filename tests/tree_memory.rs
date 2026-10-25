//! Presupuesto de memoria del árbol: va en su propio binario para que otras pruebas no ensucien el RSS.
#![cfg(target_os = "linux")]

use wincleaner::space::tree::{Kind, Tree};

fn rss_kb() -> u64 {
    let s = std::fs::read_to_string("/proc/self/status").expect("leer /proc/self/status");
    let l = s.lines().find(|l| l.starts_with("VmRSS:")).expect("VmRSS");
    l.split_whitespace().nth(1).and_then(|n| n.parse().ok()).expect("número")
}

#[test]
fn million_and_a_half_nodes_fit_in_250_mb() {
    let before = rss_kb();
    let mut t = Tree::new("root");
    let dirs = (0..150_000).map(|i| (format!("carpeta-{i:012}").into_boxed_str(), Kind::Dir, 0, 0)).collect();
    let first = t.add_children(t.root(), dirs);
    for d in first..first + 150_000 {
        let files = (0..10).map(|j| (format!("archivo-{d:08}-{j:03}").into_boxed_str(), Kind::File, 100, 1)).collect();
        t.add_children(d, files);
    }
    t.finish();
    let delta_mb = (rss_kb() - before.min(rss_kb())) as f64 / 1024.0;
    eprintln!("delta RSS: {delta_mb:.1} MB, archivos: {}", t.files());
    assert_eq!(t.files(), 1_500_000);
    assert!(delta_mb < 250.0, "{delta_mb} MB");
}
