fn main() -> anyhow::Result<()> {
    recordem_engine::bench::main(std::env::args().skip(1).collect())
}
