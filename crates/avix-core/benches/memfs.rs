use avix_core::memfs::{MemFs, VfsPath};
use criterion::{criterion_group, criterion_main, Criterion};

fn bench_memfs(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let fs = MemFs::new();
    let path = VfsPath::parse("/proc/57/status.yaml").unwrap();
    rt.block_on(async {
        fs.write(&path, b"status: running".to_vec()).await.unwrap();
    });
    c.bench_function("memfs_read", |b| {
        b.iter(|| rt.block_on(async { fs.read(&path).await.unwrap() }));
    });
}

criterion_group!(benches, bench_memfs);
criterion_main!(benches);
