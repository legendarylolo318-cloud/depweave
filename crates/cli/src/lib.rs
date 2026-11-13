//! Plain-text, script-friendly command line interface. Read-only.

use clap::{Parser, Subcommand, ValueEnum};
use depweave_core::export::{to_dot, to_json};
use depweave_core::graph::human_size;
use depweave_core::why::{format_chain, removal_command, why};
use depweave_core::{load, DepGraph, Direction, LoadOptions, Reason};
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "depweave",
    version,
    about = "Pacman dependency graph explorer (read-only)"
)]
pub struct Cli {
    /// Override the pacman DBPath (default: from /etc/pacman.conf)
    #[arg(long, global = true, value_name = "DIR")]
    pub dbpath: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Explain why a package is installed (chains from explicit packages)
    Why {
        pkg: String,
        /// Maximum number of chains to print
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// List orphans (same set as `pacman -Qdtq`)
    Orphans {
        /// Also print installed size
        #[arg(short, long)]
        long: bool,
    },
    /// Show what `pacman -Rs <pkg>` would remove
    Impact {
        pkg: String,
        /// Print only the names of every package that would be removed
        /// (target included), one per line, like `pacman -Rsp --print-format %n`
        #[arg(short, long)]
        quiet: bool,
    },
    /// List installed packages (filters combine like `pacman -Q`)
    List {
        /// Only explicitly installed packages (`pacman -Qe`)
        #[arg(short, long)]
        explicit: bool,
        /// Only packages installed as dependencies (`pacman -Qd`)
        #[arg(short, long)]
        deps: bool,
        /// Only foreign packages, not found in any sync db (`pacman -Qm`)
        #[arg(short = 'm', long)]
        foreign: bool,
        /// Only native packages (`pacman -Qn`)
        #[arg(short, long)]
        native: bool,
        /// Only packages nothing requires or optionally requires (`pacman -Qt`)
        #[arg(short = 't', long)]
        unrequired: bool,
    },
    /// Direct dependencies of a package, resolved like libalpm (`pactree -d1`)
    Deps {
        pkg: String,
        /// List every installed provider of each dependency, not just the one
        /// pacman resolves to
        #[arg(long)]
        all_providers: bool,
        /// Also list installed optional dependencies
        #[arg(long)]
        optional: bool,
    },
    /// Packages that directly depend on a package (required by)
    Rdeps {
        pkg: String,
        /// Also list packages that optionally depend on it
        #[arg(long)]
        optional: bool,
    },
    /// Package metadata (name, version, repo, reason, installed size)
    Info {
        #[arg(required = true)]
        pkgs: Vec<String>,
    },
    /// Export the graph or a neighbourhood of one package
    Export {
        #[arg(long, value_enum)]
        format: Format,
        #[arg(long)]
        root: Option<String>,
        #[arg(long, default_value_t = 2)]
        depth: usize,
        #[arg(long, value_enum, default_value_t = Dir::Both)]
        direction: Dir,
        /// Include installed optional dependencies
        #[arg(long)]
        optional: bool,
        /// Write to a file instead of stdout
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum Format {
    Dot,
    Json,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum Dir {
    Deps,
    Dependents,
    Both,
}

/// Run a CLI subcommand. Returns the process exit code.
pub fn run(cli: Cli) -> i32 {
    let Some(cmd) = cli.command else { return 0 };
    let g = match load(&LoadOptions {
        db_path: cli.dbpath,
        ..LoadOptions::default()
    }) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("depweave: {e}");
            return 2;
        }
    };
    let find = |name: &str| -> Result<usize, i32> {
        g.find_or_provider(name).ok_or_else(|| {
            eprintln!("depweave: package '{name}' is not installed");
            1
        })
    };
    let out = std::io::stdout();
    let mut out = out.lock();
    let res = match cmd {
        Command::Why { pkg, limit } => find(&pkg).map(|i| print_why(&g, i, limit, &mut out)),
        Command::Orphans { long } => {
            let mut orphans = g.orphans();
            orphans.sort_unstable_by(|&a, &b| g.packages[a].name.cmp(&g.packages[b].name));
            for i in orphans {
                let p = &g.packages[i];
                let _ = if long {
                    writeln!(out, "{}\t{}", p.name, human_size(p.size))
                } else {
                    writeln!(out, "{}", p.name)
                };
            }
            Ok(())
        }
        Command::Impact { pkg, quiet: true } => {
            let i = match find(&pkg) {
                Ok(i) => i,
                Err(c) => return c,
            };
            let imp = g.removal_impact(i);
            if !imp.blocked_by.is_empty() {
                for &b in &imp.blocked_by {
                    eprintln!(
                        "depweave: removing {} breaks {}",
                        g.packages[i].name, g.packages[b].name
                    );
                }
                return 1;
            }
            let mut names: Vec<&str> = std::iter::once(i)
                .chain(imp.members.iter().copied())
                .map(|k| g.packages[k].name.as_str())
                .collect();
            names.sort_unstable();
            for n in names {
                let _ = writeln!(out, "{n}");
            }
            Ok(())
        }
        Command::List {
            explicit,
            deps,
            foreign,
            native,
            unrequired,
        } => {
            let mut v: Vec<&str> = (0..g.len())
                .filter(|&i| {
                    let p = &g.packages[i];
                    (!explicit || p.reason == Reason::Explicit)
                        && (!deps || p.reason == Reason::Dependency)
                        && (!foreign || p.is_foreign())
                        && (!native || !p.is_foreign())
                        && (!unrequired
                            || (g.required_by(i).next().is_none()
                                && g.optional_for(i).next().is_none()
                                && !g.requires_itself(i)
                                && !g.optionally_requires_itself(i)))
                })
                .map(|i| g.packages[i].name.as_str())
                .collect();
            v.sort_unstable();
            for n in v {
                let _ = writeln!(out, "{n}");
            }
            Ok(())
        }
        Command::Deps {
            pkg,
            all_providers,
            optional,
        } => find(&pkg).map(|i| {
            let mut v: Vec<usize> = if all_providers {
                g.depends_on(i).collect()
            } else {
                g.resolved_deps(i).collect()
            };
            if optional {
                v.extend(g.optional_deps(i));
            }
            print_names(&g, v, &mut out);
            for u in &g.unresolved[i] {
                eprintln!(
                    "depweave: {}: unresolved dependency {u}",
                    g.packages[i].name
                );
            }
        }),
        Command::Rdeps { pkg, optional } => find(&pkg).map(|i| {
            let mut v: Vec<usize> = g.required_by(i).collect();
            if g.requires_itself(i) {
                v.push(i);
            }
            if optional {
                v.extend(g.optional_for(i));
            }
            print_names(&g, v, &mut out);
        }),
        Command::Info { pkgs } => {
            let mut code = Ok(());
            for (n, pkg) in pkgs.iter().enumerate() {
                let i = match find(pkg) {
                    Ok(i) => i,
                    Err(c) => {
                        code = Err(c);
                        continue;
                    }
                };
                let p = &g.packages[i];
                if n > 0 {
                    let _ = writeln!(out);
                }
                let _ = writeln!(out, "Name           : {}", p.name);
                let _ = writeln!(out, "Version        : {}", p.version);
                let _ = writeln!(out, "Repository     : {}", p.repo_label());
                let _ = writeln!(
                    out,
                    "Install Reason : {}",
                    if p.reason == Reason::Explicit {
                        "explicit"
                    } else {
                        "dependency"
                    }
                );
                let _ = writeln!(out, "Installed Size : {}", pacman_size(p.size));
                let _ = writeln!(out, "Size (bytes)   : {}", p.size);
            }
            code
        }
        Command::Impact { pkg, quiet: false } => find(&pkg).map(|i| {
            let imp = g.removal_impact(i);
            let p = &g.packages[i];
            let _ = writeln!(
                out,
                "Removing {} (recursively) would free {}",
                p.name,
                human_size(imp.freed_size)
            );
            if !imp.blocked_by.is_empty() {
                let n: Vec<_> = imp
                    .blocked_by
                    .iter()
                    .map(|&b| g.packages[b].name.as_str())
                    .collect();
                let _ = writeln!(
                    out,
                    "Note: required by {} — pacman would refuse without -c",
                    n.join(", ")
                );
            }
            let mut m = imp.members.clone();
            m.sort_by_key(|&i| std::cmp::Reverse(g.packages[i].size));
            for i in m {
                let _ = writeln!(
                    out,
                    "{}\t{}",
                    g.packages[i].name,
                    human_size(g.packages[i].size)
                );
            }
        }),
        Command::Export {
            format,
            root,
            depth,
            direction,
            optional,
            output,
        } => {
            let nodes = match root.as_deref().map(find) {
                Some(Err(c)) => return c,
                Some(Ok(r)) => {
                    let d = match direction {
                        Dir::Deps => Direction::Dependencies,
                        Dir::Dependents => Direction::Dependents,
                        Dir::Both => Direction::Both,
                    };
                    Some(g.neighborhood(r, depth, d, optional))
                }
                None => None,
            };
            let text = match format {
                Format::Dot => to_dot(&g, nodes.as_deref(), optional),
                Format::Json => to_json(&g, nodes.as_deref(), optional),
            };
            match output {
                Some(p) => {
                    if let Err(e) = std::fs::write(&p, text) {
                        eprintln!("depweave: {}: {e}", p.display());
                        return 2;
                    }
                }
                None => {
                    let _ = out.write_all(text.as_bytes());
                }
            }
            Ok(())
        }
    };
    match res {
        Ok(()) => 0,
        Err(c) => c,
    }
}

fn print_names(g: &DepGraph, mut v: Vec<usize>, out: &mut impl Write) {
    v.sort_unstable_by(|&a, &b| g.packages[a].name.cmp(&g.packages[b].name));
    v.dedup();
    for i in v {
        let _ = writeln!(out, "{}", g.packages[i].name);
    }
}

/// Installed size formatted exactly like `pacman -Qi` ("9.59 MiB").
pub fn pacman_size(bytes: u64) -> String {
    const U: [&str; 9] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB", "ZiB", "YiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while u < U.len() - 1 && v > 2048.0 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.2} {}", U[u])
}

fn print_why(g: &DepGraph, i: usize, limit: usize, out: &mut impl Write) {
    let p = &g.packages[i];
    let r = why(g, i, limit);
    let _ = writeln!(out, "{} {} ({})", p.name, p.version, p.repo_label());
    if r.target_is_explicit {
        let _ = writeln!(
            out,
            "Explicitly installed (install date: {}).",
            format_date(p.install_date)
        );
        let n = g.required_by(i).count();
        if n > 0 {
            let _ = writeln!(out, "Also required by {n} package(s).");
        }
    }
    if r.is_orphan {
        let _ = writeln!(
            out,
            "ORPHAN: installed as a dependency but nothing requires it ({}).",
            human_size(p.size)
        );
        let _ = writeln!(out, "To remove it: {}", removal_command(g, i));
    }
    for c in &r.chains {
        let _ = writeln!(out, "  {}", format_chain(g, c));
    }
    if r.truncated {
        let _ = writeln!(out, "  … more chains exist; raise --limit to see them");
    }
    for (f, d) in &r.optional_for {
        let _ = writeln!(
            out,
            "Optional dependency of {}{}",
            g.packages[*f].name,
            d.as_ref().map(|d| format!(": {d}")).unwrap_or_default()
        );
    }
    if !r.target_is_explicit && r.chains.is_empty() && !r.is_orphan && r.optional_for.is_empty() {
        let _ = writeln!(out, "No installation chain found.");
    }
}

/// `YYYY-MM-DD` from a unix timestamp (UTC), without extra dependencies.
pub fn format_date(ts: i64) -> String {
    if ts <= 0 {
        return "unknown".into();
    }
    let days = ts.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn sizes_match_pacman_qi_format() {
        use super::pacman_size;
        assert_eq!(pacman_size(0), "0.00 B");
        assert_eq!(pacman_size(2048), "2048.00 B");
        assert_eq!(pacman_size(2049), "2.00 KiB");
        assert_eq!(pacman_size(10_055_840), "9.59 MiB");
        assert_eq!(pacman_size(3 * 1024 * 1024 * 1024), "3.00 GiB");
    }

    #[test]
    fn dates() {
        assert_eq!(super::format_date(0), "unknown");
        assert_eq!(super::format_date(1_700_000_000), "2023-11-14");
        assert_eq!(super::format_date(951_782_400), "2000-02-29");
    }
}
