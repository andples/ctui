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

## Credits

25 of the themes in `themes/` (`serika-dark`, `8008`, `iceberg-dark`, …) are
adapted from [Monkeytype](https://github.com/monkeytypegame/monkeytype)'s themes
by the Monkeytype contributors, licensed under
[GPL-3.0](https://github.com/monkeytypegame/monkeytype/blob/master/LICENSE); those
files keep that license. [`themes/MONKEYTYPE.md`](themes/MONKEYTYPE.md) lists them
and explains how the colours were mapped. Other palette credits are in
[`themes/CREDITS.md`](themes/CREDITS.md).
