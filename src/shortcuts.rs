//! Shell shortcuts (`ctui init bash|zsh|fish`): `ct` for `ctui`, and one
//! name per page that opens the TUI on it.
//!
//! The rule: a page's name is `ct` + its first letter (`ctt`, `ctm`), unless
//! that's taken, then `ct` + the whole name (`cttasks`, `ctmonth`). Taken
//! means an earlier page has it (decided here, once) or the shell already
//! has a command, function or alias of that name (decided by the snippet each
//! time the shell starts, so installing something later wins). A name the
//! snippet itself defined on an earlier run doesn't count as taken, so
//! re-sourcing the rc file is safe. `ct` follows the same rule with no
//! fallback: if another `ct` exists (Arch's `extra/ct`), it's left alone.

use crate::config::View;

/// Each page's possible shortcut names, best first, with names an earlier
/// page claims already removed.
pub fn candidates() -> Vec<(View, Vec<String>)> {
    let mut claimed: Vec<String> = Vec::new();
    View::ALL
        .into_iter()
        .map(|v| {
            let name = v.name();
            let short = format!("ct{}", &name[..1]);
            let long = format!("ct{name}");
            let names: Vec<String> = [short, long]
                .into_iter()
                .filter(|n| !claimed.contains(n))
                .collect();
            claimed.extend(names.iter().cloned());
            (v, names)
        })
        .collect()
}

/// The page a shortcut name opens (`ctt`, `cttasks`, `ctmonth`…).
pub fn page_of(name: &str) -> Option<View> {
    candidates()
        .into_iter()
        .find(|(_, names)| names.iter().any(|n| n == name))
        .map(|(v, _)| v)
}

/// The command a page's shortcut runs.
fn body(v: View) -> String {
    format!("ctui --view {}", v.name())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

/// The helper bash and zsh share; `RC` and `SHELL` are filled in.
const POSIX_HEAD: &str = r#"# ctui shortcuts. In RC: eval "$(ctui init SHELL)"
# A name is only defined if nothing else in your shell has it.
_ctui_alias() {
  local cur
  if cur=$(alias "$1" 2>/dev/null); then
    case "$cur" in
      "alias $1='$2'" | "$1='$2'" | "$1=$2") ;; # ours, from an earlier run
      *) return 1 ;;
    esac
  elif command -v "$1" >/dev/null 2>&1; then
    return 1
  fi
  alias "$1=$2"
}
"#;

const FISH_HEAD: &str = r#"# ctui shortcuts. In ~/.config/fish/config.fish: ctui init fish | source
# A name is only defined if nothing else in your shell has it.
function _ctui_alias
    if functions -q $argv[1]
        # ours, from an earlier run
        string match -q -- "*$argv[2] \$argv*" (functions $argv[1]); or return 1
    else if type -q $argv[1]
        return 1
    end
    alias $argv[1] $argv[2]
end
"#;

/// The code `eval "$(ctui init zsh)"` (or `ctui init fish | source`) runs.
pub fn init(shell: Shell) -> String {
    let (mut s, or, done) = match shell {
        Shell::Bash => (
            POSIX_HEAD
                .replace("RC", "~/.bashrc")
                .replace("SHELL", "bash"),
            " || ",
            "unset -f _ctui_alias\n",
        ),
        Shell::Zsh => (
            POSIX_HEAD.replace("RC", "~/.zshrc").replace("SHELL", "zsh"),
            " || ",
            "unset -f _ctui_alias\n",
        ),
        Shell::Fish => (FISH_HEAD.to_string(), "; or ", "functions -e _ctui_alias\n"),
    };
    s.push_str("_ctui_alias ct ctui\n");
    for (v, names) in candidates() {
        let calls: Vec<String> = names
            .iter()
            .map(|n| format!("_ctui_alias {n} '{}'", body(v)))
            .collect();
        s.push_str(&calls.join(or));
        s.push('\n');
    }
    s.push_str(done);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<(View, Vec<&'static str>)> {
        vec![
            (View::Today, vec!["ctt", "cttoday"]),
            (View::Month, vec!["ctm", "ctmonth"]),
            (View::Tasks, vec!["cttasks"]),
        ]
    }

    #[test]
    fn three_letters_unless_an_earlier_page_has_it() {
        let got: Vec<(View, Vec<String>)> = candidates();
        let want: Vec<(View, Vec<String>)> = names()
            .into_iter()
            .map(|(v, n)| (v, n.into_iter().map(String::from).collect()))
            .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn no_name_belongs_to_two_pages() {
        // The snippet relies on this: it only checks the shell, not other pages.
        let all: Vec<String> = candidates().into_iter().flat_map(|(_, n)| n).collect();
        let mut dedup = all.clone();
        dedup.sort();
        dedup.dedup();
        assert_eq!(all.len(), dedup.len());
    }

    #[test]
    fn names_map_back_to_pages() {
        assert_eq!(page_of("ctt"), Some(View::Today));
        assert_eq!(page_of("cttoday"), Some(View::Today));
        assert_eq!(page_of("ctmonth"), Some(View::Month));
        assert_eq!(page_of("cttasks"), Some(View::Tasks));
        assert_eq!(page_of("ct"), None);
        assert_eq!(page_of("ctx"), None);
    }

    #[test]
    fn snippets_try_the_short_name_first() {
        let sh = init(Shell::Zsh);
        assert!(sh.contains(
            "_ctui_alias ctm 'ctui --view month' || _ctui_alias ctmonth 'ctui --view month'"
        ));
        assert!(sh.contains("\n_ctui_alias cttasks 'ctui --view tasks'\n"));
        let fish = init(Shell::Fish);
        assert!(fish.contains(
            "_ctui_alias ctt 'ctui --view today'; or _ctui_alias cttoday 'ctui --view today'"
        ));
    }
}
