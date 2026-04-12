use avix_core::ipc::frame;
use avix_core::memfs::{MemFs, VfsPath};
use avix_core::process::{ProcessEntry, ProcessKind, ProcessStatus, ProcessTable};
use avix_core::types::tool::ToolName;
use avix_core::types::Pid;
use criterion::{criterion_group, criterion_main, Criterion};
use serde_json::json;

fn bench_tool_name_mangle(c: &mut Criterion) {
    c.bench_function("tool_name_mangle", |b| {
        let tool = ToolName::parse("fs/read").unwrap();
        b.iter(|| {
            std::hint::black_box(tool.mangled());
        });
    });
}

fn bench_ipc_frame_encode_decode(c: &mut Criterion) {
    c.bench_function("ipc_frame_encode_decode", |b| {
        let payload =
            json!({"jsonrpc": "2.0", "id": 1, "method": "fs/read", "params": {"path": "/tmp/test"}});
        b.iter(|| {
            let encoded = frame::encode(&payload).unwrap();
            let decoded: serde_json::Value = frame::decode(&encoded).unwrap();
            std::hint::black_box(decoded);
        });
    });
}

fn bench_process_table_get(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let table = ProcessTable::new();
    rt.block_on(async {
        for i in 0..1000u32 {
            table
                .insert(ProcessEntry {
                    pid: Pid::from_u64(i),
                    name: format!("proc-{i}"),
                    kind: ProcessKind::Agent,
                    status: ProcessStatus::Running,
                    spawned_by_user: "alice".to_string(),
                    ..Default::default()
                })
                .await;
        }
    });

    c.bench_function("process_table_get", |b| {
        let pid = Pid::from_u64(500);
        b.iter(|| rt.block_on(async { std::hint::black_box(table.get(pid).await) }));
    });
}

fn bench_vfs_read(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let fs = MemFs::new();
    let path = VfsPath::parse("/test/data.txt").unwrap();
    rt.block_on(async {
        fs.write(&path, b"hello world".to_vec()).await.unwrap();
    });

    c.bench_function("vfs_file_read", |b| {
        b.iter(|| rt.block_on(async { std::hint::black_box(fs.read(&path).await) }));
    });
}

criterion_group!(
    benches,
    bench_tool_name_mangle,
    bench_ipc_frame_encode_decode,
    bench_process_table_get,
    bench_vfs_read,
);
criterion_main!(benches);
