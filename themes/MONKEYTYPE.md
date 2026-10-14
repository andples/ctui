# Themes from Monkeytype

The 25 themes below are adapted from [Monkeytype](https://github.com/monkeytypegame/monkeytype)'s
theme definitions (`frontend/src/ts/constants/themes.ts`) by the Monkeytype
contributors, licensed under the
[GNU General Public License v3.0](https://github.com/monkeytypegame/monkeytype/blob/master/LICENSE).
These files keep that license, whatever license the rest of ctui ends up under.
They came to ctui by way of [ttyp](https://github.com/andples/tui-type), which
adapted them first.

How they were adapted: ctui's `bg`, `fg`, `sub`, `main` and `error` are
Monkeytype's `bg`, `text`, `sub`, `main` and `error`. Monkeytype has no
colours for calendar lists, so `lists` is not taken from Monkeytype: it's six
hues stepped evenly around Monkeytype's `main` in OKLCH, keeping `main`'s
lightness and chroma where possible and nudged until each colour has at least
3:1 contrast on `bg`. Names are lowercased with `-` for spaces.

| ctui name | Monkeytype theme |
|---|---|
| `8008` | 8008 |
| `80s-after-dark` | 80s After Dark |
| `aurora` | Aurora |
| `bento` | Bento |
| `botanical` | Botanical |
| `carbon` | Carbon |
| `chaos-theory` | Chaos Theory |
| `cherry-blossom` | Cherry Blossom |
| `comfy` | Comfy |
| `dark-magic-girl` | Dark Magic Girl |
| `godspeed` | Godspeed |
| `horizon` | Horizon |
| `iceberg-dark` | Iceberg Dark |
| `iceberg-light` | Iceberg Light |
| `laser` | Laser |
| `lavender` | Lavender |
| `matrix` | Matrix |
| `miami-nights` | Miami Nights |
| `modern-dolch` | Modern Dolch |
| `nautilus` | Nautilus |
| `olivia` | Olivia |
| `paper` | Paper |
| `serika-dark` | Serika Dark |
| `serika` | Serika |
| `superuser` | Superuser |
