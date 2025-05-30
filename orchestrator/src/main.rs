use eyre::Result;

fn main() -> Result<()> {
    drone_lab::init()?;
    drone_lab::banner();
    drone_lab::cli::run()
}
