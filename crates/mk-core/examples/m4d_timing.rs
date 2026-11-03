//! M4d timings on an existing project (NFR-PERF-02 core path, first-page cost of filter / find /
//! sort). Usage: `cargo run --release -p mk-core --example m4d_timing -- PROJECT_DIR [RUNS]`.

use std::time::Instant;

use mk_core::{ConnectionQuery, Project, SortColumn};

fn p95(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() as f64 * 0.95).ceil() as usize).clamp(1, v.len()) - 1]
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().expect("project dir"));
    let runs: usize = args.next().map_or(10, |r| r.parse().expect("runs"));

    // Open to a first connections screen: open, summary, count, first page.
    let mut open = Vec::new();
    for _ in 0..runs {
        let t = Instant::now();
        let (project, _) = Project::open(&dir).expect("open");
        let q = ConnectionQuery::default();
        project.connections_summary().expect("summary");
        project.connection_count_for(&q).expect("count");
        project.connections_page(&q, None, 60).expect("page");
        open.push(t.elapsed().as_secs_f64() * 1000.0);
        drop(project);
    }
    println!(
        "open+first screen  p50 {:.0} ms  p95 {:.0} ms  max {:.0} ms",
        {
            let mut s = open.clone();
            s.sort_by(f64::total_cmp);
            s[s.len() / 2]
        },
        p95(open.clone()),
        open.iter().cloned().fold(0.0, f64::max)
    );

    let (project, _) = Project::open(&dir).expect("open");
    let cases: Vec<(&str, ConnectionQuery)> = vec![
        ("default", ConnectionQuery::default()),
        (
            "kind=email",
            ConnectionQuery {
                kinds: vec!["email".into()],
                ..Default::default()
            },
        ),
        (
            "min_sources=3",
            ConnectionQuery {
                min_sources: 3,
                ..Default::default()
            },
        ),
        (
            "file_type=pdf",
            ConnectionQuery {
                file_types: vec!["pdf".into()],
                ..Default::default()
            },
        ),
        (
            "find=acme",
            ConnectionQuery {
                find: "acme".into(),
                ..Default::default()
            },
        ),
        (
            "find=1 (broad)",
            ConnectionQuery {
                find: "1".into(),
                ..Default::default()
            },
        ),
        (
            "sort=value asc",
            ConnectionQuery {
                sort: SortColumn::Value,
                ..Default::default()
            },
        ),
        (
            "sort=value desc",
            ConnectionQuery {
                sort: SortColumn::Value,
                descending: true,
                ..Default::default()
            },
        ),
        (
            "sort=kind",
            ConnectionQuery {
                sort: SortColumn::Kind,
                ..Default::default()
            },
        ),
        (
            "sort=sources desc",
            ConnectionQuery {
                sort: SortColumn::Sources,
                descending: true,
                ..Default::default()
            },
        ),
        (
            "sort=mentions desc",
            ConnectionQuery {
                sort: SortColumn::Mentions,
                descending: true,
                ..Default::default()
            },
        ),
        (
            "sort=file types",
            ConnectionQuery {
                sort: SortColumn::FileTypes,
                ..Default::default()
            },
        ),
        (
            "email + find + sort value",
            ConnectionQuery {
                kinds: vec!["email".into()],
                find: "a".into(),
                sort: SortColumn::Value,
                ..Default::default()
            },
        ),
    ];
    for (name, q) in cases {
        let mut ms = Vec::new();
        let mut rows = 0;
        let mut count = 0;
        for _ in 0..runs.max(10) {
            let t = Instant::now();
            count = project.connection_count_for(&q).expect("count");
            let page = project.connections_page(&q, None, 60).expect("page");
            rows = page.rows.len();
            ms.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!(
            "{name:<28} count {count:>7}  rows {rows:>3}  p95 {:.1} ms",
            p95(ms)
        );
    }
}
