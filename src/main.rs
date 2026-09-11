use std::path::PathBuf;

use clap::Parser;
use crank::catalog::{CatalogSource, load_embedded, load_local};

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
    let (catalog, source) = match cli.catalog {
        Some(path) => {
            let bundle = load_local(path)?;
            (bundle.catalog, bundle.source)
        }
        None => {
            let bundle = load_embedded()?;
            (bundle.catalog, bundle.source)
        }
    };

    if cli.validate {
        let location = match source {
            CatalogSource::Embedded => "embedded".to_owned(),
            CatalogSource::Local(path) => path.display().to_string(),
        };
        println!(
            "catalog is valid: {} categories, {} actions ({location})",
            catalog.categories.len(),
            catalog.actions.len()
        );
        return Ok(());
    }

    println!(
        "Crank catalog loaded ({} actions). The interactive interface is coming next.",
        catalog.actions.len()
    );
    Ok(())
}
