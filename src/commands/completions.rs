use anyhow::Result;
use clap::ValueEnum;

#[derive(Clone, Copy, ValueEnum)]
pub enum Shell {
    Zsh,
}

const ZSH_SCRIPT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/completions/_zotero.zsh"
));

pub fn run(shell: Shell) -> Result<()> {
    match shell {
        Shell::Zsh => print!("{}", ZSH_SCRIPT),
    }
    Ok(())
}
