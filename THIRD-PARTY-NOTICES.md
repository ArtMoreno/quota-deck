# Third-party notices

## Upstream project

This is a fork of [levi-qiao/herdr-agent-quota](https://github.com/levi-qiao/herdr-agent-quota),
MIT licensed, Copyright (c) 2026 Levi Qiao. The upstream licence text is
reproduced in full in [LICENSE](LICENSE) and continues to cover the code this
fork inherits.

Fork-specific Windows, Hermes, OpenRouter, and brand-layer changes are
Copyright (c) 2026 Art Moreno.

## Brand marks in `docs/icons/`

These are third-party trademarks, included to identify the services this plugin
reports on. Inclusion is not affiliation, sponsorship, or endorsement by any of
them. Each file records its own source in a comment.

### Simple Icons — CC0 1.0 Universal

`agy.svg`, `claude.svg`, `cursor.svg`, `opencode.svg`, `openrouter.svg` use path
data from [simple-icons/simple-icons](https://github.com/simple-icons/simple-icons),
released under [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/).
CC0 requires no attribution; it is given here because the marks are trademarks
of their respective owners even where the path data is not copyrighted.

`agy.svg` uses the Google Gemini mark: Antigravity is Google's harness and there
is no separate Antigravity mark in the set. This substitution is noted in the
file.

### lobe-icons — MIT

`codex.svg` uses path data from
[lobehub/lobe-icons](https://github.com/lobehub/lobe-icons).

```
MIT License

Copyright (c) 2023 LobeHub

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### Drawn for this project

`hermes.svg`, `omp.svg`, and `pi.svg` are original geometric marks created for
QuotaDeck. The winged-H Hermes mark is not an official Nous Research asset or a
trace; it is used only to identify that integration. These files and their PNG
renders are covered by this project's MIT licence.

### Glyph codepoints

`src/brand.rs` references private-use codepoints in the `Herdr Agent Icons Max`
font, which ships with the separate
[qintmb/herdr-icon-agent-ui](https://github.com/qintmb/herdr-icon-agent-ui)
plugin. The bundled `docs/icons/QuotaDeckIcons-Regular.ttf` is an eight-provider
subset of that MIT-licensed font, with the CC0 OpenRouter path added. Its
license is reproduced in `docs/icons/FONT-LICENSE.txt`. The reproducible
builder is `scripts/build-icon-font.py`. The `unicode` glyph set exists for users without that font.

## Herdr website logo

The unchanged Herdr logo at docs/site/assets/herdr.png comes from
https://herdr.dev/assets/logo.png and identifies the host application.
Herdr's name and logo remain the property of their respective owners; use here
does not imply sponsorship or endorsement.

