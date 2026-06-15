mod config;
mod markdown;
mod models;
mod parser;
mod utils;

use std::fs::File;
use std::io::{stdout, BufWriter};
use std::process;

use anyhow::{Context, Result};
use clap::Parser;
use log::info;

use crate::config::{build_config, Cli};
use crate::markdown::generate_markdown;
use crate::parser::parse_openapi;

/// Parses CLI arguments, parses the spec, and writes markdown to the
/// requested output (file or stdout).
fn run() -> Result<()> {
    // Initialize logger
    env_logger::init();

    // Parse command-line arguments
    let cli = Cli::parse();

    // Build configuration
    let config = build_config(&cli);

    // Parse OpenAPI spec
    let api_doc = parse_openapi(&cli.input)
        .with_context(|| format!("Failed to parse OpenAPI file: {:?}", cli.input))?;

    // Generate markdown
    if let Some(output_path) = &cli.output {
        // Write to file
        let file = File::create(output_path)
            .with_context(|| format!("Failed to create output file: {:?}", output_path))?;
        let mut writer = BufWriter::new(file);

        generate_markdown(&mut writer, &api_doc, &config)
            .with_context(|| "Failed to generate markdown")?;

        info!("Documentation written to: {:?}", output_path);
    } else {
        // Write to stdout
        let mut writer = stdout();

        generate_markdown(&mut writer, &api_doc, &config)
            .with_context(|| "Failed to generate markdown")?;
    }

    Ok(())
}

fn main() {
    if let Err(err) = run() {
        eprintln!("Error: {:#}", err);
        process::exit(1);
    }
}
