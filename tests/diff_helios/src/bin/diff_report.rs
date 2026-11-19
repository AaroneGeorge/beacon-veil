//! Runs the differential test and writes `evidence/diff/{report.json,report.md,cases.tsv,run.log}`.
//! `cargo run -p diff_helios --bin diff_report` (threads: `BV_DIFF_THREADS`, default 4). Exit code 1 on any failure.

fn main() -> anyhow::Result<()> {
    let s = diff_helios::run_and_report(&diff_helios::default_columns(), diff_helios::threads())?;
    for f in &s.failures {
        eprintln!("FAIL {f}");
    }
    println!(
        "total {} valid {} gloas {} crossing_valid {} v1_v2_disagreements {} helios_accepts_we_refuse {} pass {}",
        s.json["total"],
        s.json["valid"],
        s.json["gloas_cases"],
        s.json["fork_crossing_valid"],
        s.json["v1_v2_disagreements"],
        s.json["helios_accepts_we_refuse"]["by_reason"],
        s.json["pass"]
    );
    if !s.failures.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}
