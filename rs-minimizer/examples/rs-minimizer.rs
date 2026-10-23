fn main() {
    std::process::exit(rs_minimizer::cli::run(std::env::args().skip(1).collect()));
}
