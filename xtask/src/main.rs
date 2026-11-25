use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask", about = "twit dev task runner")]
struct Cli {
    #[arg(
        long,
        global = true,
        help = "Pass --locked to cargo (fail if Cargo.lock is out of date)"
    )]
    locked: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    #[command(about = "Build the workspace")]
    Build,
    #[command(about = "Run the workspace tests")]
    Test {
        #[arg(
            long,
            short,
            help = "Only test this package (repeatable; defaults to the whole workspace)"
        )]
        package: Vec<String>,
    },
    #[command(about = "Clippy the workspace and check it without default features")]
    Lint,
    #[command(about = "Rustfmt the workspace")]
    Format {
        #[arg(long, help = "Rewrite the files instead of only checking them")]
        fix: bool,
    },
    #[command(about = "Run everything CI runs: format, lint, test")]
    Ci,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask crate lives one directory below the workspace root")?
        .to_path_buf();
    let x = Xtask {
        root,
        locked: cli.locked,
    };
    match cli.cmd {
        Cmd::Build => x.build(),
        Cmd::Test { package } => x.test(&package),
        Cmd::Lint => x.lint(),
        Cmd::Format { fix } => x.format(fix),
        Cmd::Ci => {
            x.format(false)?;
            x.lint()?;
            x.test(&[])
        }
    }
}

struct Xtask {
    root: PathBuf,
    locked: bool,
}

impl Xtask {
    fn build(&self) -> Result<()> {
        self.cargo(&["build", "--workspace", "--all-targets"])
    }

    fn test(&self, packages: &[String]) -> Result<()> {
        let mut args = vec!["test"];
        if packages.is_empty() {
            args.push("--workspace");
        }
        for p in packages {
            args.extend(["--package", p]);
        }
        self.cargo(&args)
    }

    fn lint(&self) -> Result<()> {
        self.cargo(&[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ])?;
        self.cargo(&["check", "--workspace", "--no-default-features"])
    }

    fn format(&self, fix: bool) -> Result<()> {
        let mut args = vec!["fmt", "--all"];
        if !fix {
            args.extend(["--", "--check"]);
        }
        self.run(&args, false)
    }

    fn cargo(&self, args: &[&str]) -> Result<()> {
        self.run(args, self.locked)
    }

    fn run(&self, args: &[&str], locked: bool) -> Result<()> {
        let (sub, rest) = args.split_first().context("empty cargo command")?;
        let mut cmd = Command::new("cargo");
        cmd.current_dir(&self.root).arg(sub);
        if locked {
            cmd.arg("--locked");
        }
        cmd.args(rest);
        let status = cmd
            .status()
            .context("failed to spawn `cargo` (is it installed and on PATH?)")?;
        if !status.success() {
            bail!("`cargo {}` exited with {status}", args.join(" "));
        }
        Ok(())
    }
}
