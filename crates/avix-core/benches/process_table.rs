use avix_core::process::{ProcessEntry, ProcessKind, ProcessStatus, ProcessTable};
use avix_core::types::Pid;
use criterion::{criterion_group, criterion_main, Criterion};

fn entry(pid: u32) -> ProcessEntry {
    ProcessEntry {
        pid: Pid::new(pid),
        name: format!("agent-{pid}"),
        kind: ProcessKind::Agent,
        status: ProcessStatus::Running,
        spawned_by_user: "alice".to_string(),
        ..Default::default()
    }
}

fn bench_process_table(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let table = ProcessTable::new();
    rt.block_on(async {
        for i in 0..1000u32 {
            table.insert(entry(i)).await;
        }
    });
    c.bench_function("process_table_get", |b| {
        b.iter(|| rt.block_on(async { table.get(Pid::new(42)).await }));
    });
}

criterion_group!(benches, bench_process_table);
criterion_main!(benches);
