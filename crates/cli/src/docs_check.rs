//! The documentation against the program: every command and flag of `acc` is in docs/COMMANDS.md, every `acc` command the
//! documents show is one it has, with flags it takes, every link between the documents leads somewhere, and every
//! document the program's own text names is there. So the documents can't drift from the program unnoticed.

use crate::Cli;
use clap::{Command, CommandFactory};
use std::path::{Path, PathBuf};

/// The documents whose `acc` commands must work as written. The historical ones (docs/PLAN.md and the like) show commands
/// as they were at the time.
const GUIDES: &[&str] = &[
    "README.md",
    "CONTRIBUTING.md",
    "CHANGELOG.md",
    "docs/README.md",
    "docs/GETTING_STARTED.md",
    "docs/COMMANDS.md",
    "docs/LONG_RUNS.md",
    "docs/READING_RESULTS.md",
    "docs/GPU_SETUP.md",
    "docs/TROUBLESHOOTING.md",
    "docs/VISUALIZER.md",
    "docs/ARCHITECTURE.md",
    "docs/FORMATS.md",
    "docs/RESULTS.md",
];

/// The repository's top folder.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", shown(path)))
}

/// A path in the repository as the documents write it: `docs/COMMANDS.md`.
fn shown(path: &Path) -> String {
    path.strip_prefix(root()).unwrap_or(path).display().to_string().replace('\\', "/")
}

/// Every command that does something, as typed after `acc` (`endgame build` for a command of a command), hidden ones too.
fn commands() -> Vec<(String, Command)> {
    fn walk(prefix: &str, command: &Command, out: &mut Vec<(String, Command)>) {
        for sub in command.get_subcommands() {
            let name = format!("{prefix}{}", sub.get_name());
            if sub.has_subcommands() {
                walk(&format!("{name} "), sub, out);
            } else {
                out.push((name, sub.clone()));
            }
        }
    }
    let mut out = Vec::new();
    walk("", &Cli::command(), &mut out);
    out
}

/// What a document has in code: its fenced blocks line by line, and its `inline code`.
fn code(doc: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut fenced = false;
    for line in doc.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if fenced {
            out.push(line);
        } else {
            out.extend(line.split('`').skip(1).step_by(2));
        }
    }
    out
}

/// The part of COMMANDS.md about `acc <name>`: from its heading to the next heading that names a command.
fn section<'a>(doc: &'a str, name: &str) -> Option<&'a str> {
    let title = format!("`acc {name}`");
    let (mut start, mut at, mut fenced) = (None, 0, false);
    for line in doc.split_inclusive('\n') {
        fenced ^= line.trim_start().starts_with("```");
        if !fenced && line.starts_with('#') && line.contains("`acc ") {
            if let Some(start) = start {
                return Some(&doc[start..at]);
            }
            if line.contains(&title) {
                start = Some(at);
            }
        }
        at += line.len();
    }
    start.map(|start| &doc[start..])
}

/// Whether `text` names `flag` (`--threads`) in code, and not only as the start of a longer one (`--threads-x`).
fn names_flag(text: &str, flag: &str) -> bool {
    let quoted = format!("`{flag}");
    text.match_indices(&quoted)
        .any(|(i, m)| !text[i + m.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '-'))
}

/// The words after each `acc` in a piece of code, up to the end of its command (or the next `acc`).
fn invocations(code: &str) -> Vec<Vec<&str>> {
    let words: Vec<&str> = code.split_whitespace().collect();
    let end = |w: &&str| matches!(*w, "|" | ";" | "&&" | ">" | "2>&1" | "acc" | "acc.exe");
    (0..words.len())
        .filter(|&i| matches!(words[i], "acc" | "acc.exe"))
        .map(|i| words[i + 1..].iter().copied().take_while(|w| !end(w)).collect())
        .collect()
}

/// A word as a command or flag, without what punctuates the text around it.
fn bare(word: &str) -> &str {
    word.trim_end_matches([',', '.', ';', ':', ')'])
}

/// What is wrong with `acc <words>`, if anything: a command acc doesn't have, or a flag that command doesn't take.
/// `acc` followed by a word that isn't a command is taken for prose quoted from the program's output (`acc works here`),
/// unless a flag or nothing follows it, as in a command.
fn mistake(words: &[&str], all: &[(String, Command)]) -> Option<String> {
    let first = bare(words.first()?);
    if first.starts_with(|c: char| c == '<' || c.is_ascii_digit()) || first == "help" {
        return None;
    }
    if first.starts_with('-') {
        return (!matches!(first, "--help" | "-h" | "--version" | "-V"))
            .then(|| format!("acc has no flag {first}"));
    }
    // A command, or with a command of its own, the two words.
    let two = words.get(1).map(|w| format!("{first} {}", bare(w)));
    let found = all.iter().find(|(name, _)| Some(name) == two.as_ref()).map(|c| (c, 2)).or_else(|| {
        all.iter().find(|(name, _)| name == first || name.starts_with(&format!("{first} "))).map(|c| (c, 1))
    });
    let Some(((name, command), taken)) = found else {
        let like_a_command = words.get(1).is_none_or(|w| w.starts_with("--"));
        return like_a_command.then(|| format!("acc has no command {first}"));
    };
    if !name.contains(' ') || taken == 2 {
        for word in &words[taken..] {
            let flag = bare(word).split('=').next().unwrap_or_default();
            let Some(long) = flag.strip_prefix("--").filter(|l| !l.is_empty()) else { continue };
            if long != "help" && !command.get_arguments().any(|a| a.get_long() == Some(long)) {
                return Some(format!("acc {name} has no flag --{long}"));
            }
        }
    }
    None
}

#[test]
fn commands_md_names_every_command_and_flag() {
    let doc = read(&root().join("docs/COMMANDS.md"));
    let mut missing = Vec::new();
    for (name, command) in commands() {
        let Some(section) = section(&doc, &name) else {
            missing.push(format!("a heading for `acc {name}`"));
            continue;
        };
        for arg in command.get_arguments().filter(|a| !matches!(a.get_id().as_str(), "help" | "version")) {
            let (named, what) = match arg.get_long() {
                Some(long) => (names_flag(section, &format!("--{long}")), format!("--{long}")),
                None => {
                    let value = arg.get_id().as_str().to_uppercase();
                    (section.contains(&format!("`{value}`")), value)
                }
            };
            if !named {
                missing.push(format!("{what} in the part on `acc {name}`"));
            }
        }
    }
    assert!(missing.is_empty(), "docs/COMMANDS.md lacks:\n  {}", missing.join("\n  "));
}

#[test]
fn every_flag_of_acc_batch_help_is_in_commands_md() {
    let doc = read(&root().join("docs/COMMANDS.md"));
    let batch = section(&doc, "batch").expect("a part on acc batch");
    let mut cli = Cli::command();
    let help = cli.find_subcommand_mut("batch").expect("acc batch").render_long_help().to_string();
    // A flag's own line is indented 6 spaces at most (`      --from <FROM>`); the lines that describe it, more.
    let flags: Vec<&str> = help
        .lines()
        .filter(|line| line.len() - line.trim_start().len() <= 6)
        .filter_map(|line| line.trim_start().strip_prefix("--"))
        .map(|rest| rest.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).next().unwrap_or_default())
        .collect();
    assert!(flags.len() > 20, "the flags read from acc batch --help: {flags:?}");
    for flag in flags {
        assert!(
            names_flag(batch, &format!("--{flag}")),
            "docs/COMMANDS.md's part on acc batch lacks --{flag}"
        );
    }
}

#[test]
fn the_commands_in_the_documents_are_real() {
    let all = commands();
    let commands_md = read(&root().join("docs/COMMANDS.md"));
    let mut wrong = Vec::new();
    for guide in GUIDES {
        let doc = read(&root().join(guide));
        for words in code(&doc).into_iter().flat_map(invocations) {
            if let Some(mistake) = mistake(&words, &all) {
                wrong.push(format!("{guide}: `acc {}`: {mistake}", words.join(" ")));
            }
        }
    }
    // Each command the front page names has its part in COMMANDS.md.
    for words in code(&read(&root().join("README.md"))).into_iter().flat_map(invocations) {
        let Some(first) = words.first().map(|w| bare(w)) else { continue };
        let two = words.get(1).map(|w| format!("{first} {}", bare(w)));
        let named = all.iter().find(|(name, _)| name == first || Some(name) == two.as_ref());
        if let Some((name, _)) = named
            && section(&commands_md, name).is_none()
        {
            wrong.push(format!("README.md names acc {name}, which docs/COMMANDS.md has no part on"));
        }
    }
    assert!(wrong.is_empty(), "\n  {}", wrong.join("\n  "));
}

/// A heading's anchor, as GitHub makes it: lower case, spaces as hyphens, other punctuation left out.
fn anchor(heading: &str) -> String {
    heading
        .trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

/// The anchors of a document's headings, the second of a name with `-1` after it, and so on.
fn anchors(doc: &str) -> Vec<String> {
    let (mut out, mut names) = (Vec::new(), Vec::new());
    let mut fenced = false;
    for line in doc.lines() {
        fenced ^= line.trim_start().starts_with("```");
        let Some(heading) = line.strip_prefix('#').filter(|_| !fenced) else { continue };
        let name = anchor(heading.trim_start_matches('#'));
        let seen = names.iter().filter(|n| **n == name).count();
        out.push(if seen == 0 { name.clone() } else { format!("{name}-{seen}") });
        names.push(name);
    }
    out
}

#[test]
fn the_links_between_the_documents_lead_somewhere() {
    let mut documents: Vec<PathBuf> = GUIDES.iter().map(|g| root().join(g)).collect();
    let docs = std::fs::read_dir(root().join("docs")).expect("docs/").flatten().map(|e| e.path());
    documents.extend(docs.filter(|p| p.extension().is_some_and(|x| x == "md")));
    documents.sort();
    documents.dedup();
    let mut broken = Vec::new();
    for path in &documents {
        let doc = read(path);
        let mut fenced = false;
        for line in doc.lines() {
            fenced ^= line.trim_start().starts_with("```");
            if fenced {
                continue;
            }
            for (i, _) in line.match_indices("](") {
                let target = &line[i + 2..];
                let Some(target) = target.split(')').next().filter(|t| !t.contains(' ')) else { continue };
                if target.contains(':') {
                    continue; // https:, mailto:
                }
                let (file, fragment) = target.split_once('#').unwrap_or((target, ""));
                let linked =
                    if file.is_empty() { path.clone() } else { path.parent().expect("a folder").join(file) };
                if !linked.exists() {
                    broken.push(format!("{}: {target}: no such file", shown(path)));
                } else if !fragment.is_empty()
                    && linked.extension().is_some_and(|x| x == "md")
                    && !anchors(&read(&linked)).iter().any(|a| a == fragment)
                {
                    broken.push(format!("{}: {target}: no such heading", shown(path)));
                }
            }
        }
    }
    assert!(broken.is_empty(), "\n  {}", broken.join("\n  "));
}

#[test]
fn the_documents_the_program_names_are_there() {
    fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("a folder").flatten() {
            let path = entry.path();
            if path.is_dir() {
                sources(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    sources(&root().join("crates/cli/src"), &mut files);
    sources(&root().join("tools"), &mut files);
    files.push(root().join("crates/gpu/src/lib.rs"));
    let mut named = 0;
    for file in &files {
        let text = read(file);
        for (i, _) in text.match_indices("docs/") {
            let rest = &text[i + 5..];
            let stem: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
            if stem.is_empty() || !rest[stem.len()..].starts_with(".md") {
                continue;
            }
            named += 1;
            let doc = format!("docs/{stem}.md");
            assert!(root().join(&doc).exists(), "{} names {doc}, which isn't there", shown(file));
        }
    }
    assert!(named > 5, "only {named} documents named");
}

#[test]
fn the_checks_catch_what_they_are_for() {
    let all = commands();
    let words = |text: &'static str| text.split_whitespace().collect::<Vec<_>>();
    assert_eq!(mistake(&words("run --quick"), &all), None);
    assert_eq!(mistake(&words("endgame build --piles 11"), &all), None);
    assert_eq!(mistake(&words("batch: seeds 0..9"), &all), None);
    assert_eq!(mistake(&words("works here; the WARN lines say"), &all), None, "output, not a command");
    assert!(mistake(&words("run --quik"), &all).is_some());
    assert!(mistake(&words("resum"), &all).is_some());
    assert!(mistake(&words("resum --hours 2"), &all).is_some());
    assert!(mistake(&words("endgame build --threds 2"), &all).is_some());
    assert_eq!(invocations("acc status; acc pause"), vec![vec!["status;"], vec!["pause"]]);
    assert_eq!(mistake(&words("results batch-0-10 shows what they found."), &all), None);
    assert_eq!(code("a `acc run` b\n```\nacc doctor\n```\n"), vec!["acc run", "acc doctor"]);
    assert!(names_flag("`--gpu-config FILE`", "--gpu-config") && !names_flag("`--gpu-config FILE`", "--gpu"));
    assert_eq!(anchor(" `acc endgame build`"), "acc-endgame-build");
    assert_eq!(anchor(" 2. Add NVRTC"), "2-add-nvrtc");
    assert_eq!(anchor(" The GPU's settings"), "the-gpus-settings");
    assert_eq!(anchors("# A\n## A\n```\n# not a heading\n```\n### A"), ["a", "a-1", "a-2"]);
    let doc = "## `acc run`\nx `--deals`\n### `acc status`\ny\n";
    assert_eq!(section(doc, "run"), Some("## `acc run`\nx `--deals`\n"));
    assert_eq!(section(doc, "status"), Some("### `acc status`\ny\n"));
}
