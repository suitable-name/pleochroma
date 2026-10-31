//! Opt-in desktop live-render latency harness; see `benchmark::run`.
fn main() -> anyhow::Result<()> {
    pleochroma_cut::benchmark::run()
}
