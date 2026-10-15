fn main() -> anyhow::Result<()> {
    recorder_lib::asr_bench(std::env::args().skip(1).collect())
}
