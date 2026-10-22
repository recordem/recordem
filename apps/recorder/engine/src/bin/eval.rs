fn main() -> anyhow::Result<()> {
    recordem_engine::eval::main(std::env::args().skip(1).collect())
}
