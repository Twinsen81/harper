# Third-party notices

Parts of `harper-llm` are ported from [Margin](https://github.com/phoenixak/margin)
(commit `610bb84dd89b9c98b8c5ea95d498015c79ffb14e`, 2026-10-25):

- The check prompt (`src/prompt.rs`), from `packages/core/src/prompts/check.ts` (prompt version 3).
- The fix guards (`src/guards.rs`), from `packages/core/src/guards.ts`.
- The protected-span masking (`src/mask.rs`), from `packages/core/src/mask.ts`.
- The hunk grouping of the word diff (`src/diff.rs`), from `packages/core/src/diff.ts`.
- The evaluation fixtures (`eval/fixtures/*.json`), copied unchanged from `packages/eval/fixtures/`.

Margin is licensed under the MIT License:

```
MIT License

Copyright (c) 2026 Ahmed Khaled

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
