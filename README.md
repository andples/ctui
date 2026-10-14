# ctui

A minimal terminal calendar and task list. A TUI when you run `ctui`, a
plain CLI when you give it arguments. Your data is local iCalendar files: no
accounts, no Google.

```sh
alias ct=ctui
ct add "dentist fri 2-3pm #health"
ct add "pay rent tom !"
ct ls
ct done rent
ct                      # open the TUI
```

Work in progress: see [DESIGN.md](DESIGN.md) for the plan.
