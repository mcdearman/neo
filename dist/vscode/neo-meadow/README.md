# Neo Meadow

The Meadow colour scheme from NeoCode, for VS Code. Two themes, **Neo Meadow Dark** and **Neo Meadow Light**.

Each kind of name has a colour of its own, after the way the Meadow REPL colours what is typed at it:

| What | Dark | Light |
|---|---|---|
| Keywords | `#B393F4` | `#8A3FB5` |
| Types | `#FFB07A` | `#B4541A` |
| Modules and paths | `#E0A3F5` | `#9A3FB8` |
| Constructors and enum members | `#889FEC` | `#3F5BC4` |
| Functions | `#FFE08A` | `#9A6A0E` |
| Variables and parameters | `#A5B7F2` | `#4A67D6` |
| Numbers | `#FFD866` | `#8A5D08` |
| Strings | `#A9DC76` | `#1E7A4F` |
| Attributes and lifetimes | `#E6B66C` | `#8A6A2A` |
| Comments | `#8C898D` | `#5C6576` |

Operators and punctuation are left the colour of the text. The window itself uses Neo's palette and royal blue accent, and the terminal Neo's sixteen colours.

The distinctions between variables, constructors and modules come from a language server's semantic tokens, so they show best with one running (Meadow's, rust-analyzer and so on).

## Install

    code --install-extension neo-meadow-theme-0.1.0.vsix

then choose it with **Preferences: Color Theme**. `dist/vscode/package.sh` in the Neo repository builds the `.vsix`.

The colours are defined in `crates/neo/src/widgets/highlight.rs` and `crates/neo-theme` in the Neo repository; this is a copy of them, to be regenerated if they change.
