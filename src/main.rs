use std::path::PathBuf;

use clap::Parser;
use crank::catalog::{CatalogBundle, CatalogFiles, CatalogSource, load_embedded, load_local};

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Load a catalog directory instead of the bundled catalog
    #[arg(long, value_name = "DIRECTORY")]
    catalog: Option<PathBuf>,

    /// Validate the selected catalog and exit
    #[arg(long)]
    validate: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("crank: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.catalog {
        Some(path) => run_bundle(load_local(path)?, cli.validate),
        None => run_bundle(load_embedded()?, cli.validate),
    }
}

fn run_bundle<F: CatalogFiles>(
    bundle: CatalogBundle<F>,
    validate_only: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if validate_only {
        let location = match &bundle.source {
            CatalogSource::Embedded => "embedded".to_owned(),
            CatalogSource::Local(path) => path.display().to_string(),
        };
        println!(
            "catalog is valid: {} categories, {} actions ({location})",
            bundle.catalog.categories.len(),
            bundle.catalog.actions.len()
        );
        return Ok(());
    }

    crank::tui::run(bundle.catalog, &bundle.files, bundle.source)?;
    Ok(())
}
