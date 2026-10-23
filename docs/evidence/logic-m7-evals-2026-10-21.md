# Logic Pro M7: the manual, the instruction pack and the eval, 2026-10-21

Status: the eval of how the model knows Logic Pro and works in it ran with the producer's configured model,
`anthropic/claude-sonnet-5-5`. The first run passed 14 of 23. After the fixes below, the whole eval passed 22 of 23
(at c9979e28), and its one miss, a plug-in moved, passed on the final words (a17f9f76) when run alone.

Scope: `feat/logic` at a17f9f76: `logic_manual` over Apple's Logic Pro User Guide for Mac, Logic's instruction pack
(`crates/kumi-runtime/src/integrations/logic_instructions.txt`), and `crates/kumi/examples/eval_logic.rs`.

## How the eval runs

Each of the 23 cases is its own session over the simulated Logic, Kumi's host called in process, with the LOGIC
profile, listening on, and a throwaway folder for memory, recipes, techniques, gaps and the library. `logic_manual`
answers from a fixture of the guide laid out as Apple's site (20 topics in Kumi's own words), so nothing reaches the
network and a run can never launch Logic. Every check is code, not a judgement:

- 10 knowledge probes (a region and not a clip, the Stereo Out, Track Stacks, the Marker, Tempo and Signature Lists,
  key commands on the selection, what Logic's undo keeps no step of, the Controls View, quantize as a region
  parameter, an import's own track, Smart Controls): Logic's words, none of Live's, nothing changed. Where Logic's
  instruction pack states the answer, the case names the pack's own words and fails if the pack stops saying them;
  where it doesn't, the guide must be asked and a topic it gave cited by its exact title.
- 7 changes over Scratch (a fader, a rename, a marker, a tempo event, a Gain on the Stereo Out, a send, a plan of
  several): the exact kinds Logic applied, the rows in HISTORY and the simulated values.
- 5 limits (a track moved, a track's colour, a send's mode, a plug-in moved, Quick Sampler): refused saying why, or a
  way round taken and said as one (what was done instead, and why).
- 1 listening question ("is the mix too quiet?"): Kumi heard the simulated mix and spoke of its loudness, changing
  nothing.

`--dry` runs every check against canned answers (good and wrong ones) with no model; CI runs it
(`crates/kumi/tests/eval_logic_dry.rs`).

## The runs

| Run | Words at | Cases | Passed | Model calls | Tokens in (cache reads) | Tokens out |
| --- | --- | --- | --- | --- | --- | --- |
| first | db4733cd | all 23 | 14 | 40 | 766,274 (721,427) | 9,400 |
| fix 1 | e5c02231..51453e05 | the nine missed | 7 | 15 | 289,178 (261,148) | 4,645 |
| fix 2 | c9979e28 | key commands, plug-in moved | 2 | 7 | 137,985 (115,244) | 1,422 |
| fix 3 | c9979e28 | all 23 | 22 | 40 | 775,766 (746,532) | 9,852 |
| fix 4 | 4cc13f5a | plug-in moved | 0 | 3 | 58,176 (38,207) | 825 |
| fix 5 | a17f9f76 | plug-in moved | 1 | 5 | 110,702 (85,858) | 1,422 |

In all, 110 model calls and 2,138,081 tokens in (1,968,416 of them cache reads), 27,566 out. Each request carries up to
48.7 KB of instructions and tool definitions; Logic's first request is 58,578 bytes (17,229 of system text) against
Live's 73,204 (25,945).

## What the first run found, and what changed for it

- Six probes answered rightly from Logic's instruction pack but were failed for not asking the guide: the check was
  wrong for what the pack states, and now requires the guide only where the pack doesn't say the answer (51453e05).
- The key commands probe asked the guide and cited a topic it hadn't given ("Edit regions"): the manual's answer now
  says only a topic it gives may be cited, by its exact title, each passage under its topic's title, and the pack says
  to ask the guide rather than answer from memory where it is silent (e5c02231, c9979e28).
- A plug-in "moved" was taken off and added again with six of its twenty-five settings carried over, the others
  silently back at Logic's defaults: `fx_add` and the pack now say Logic moves a plug-in only by a drag (which keeps its
  settings and which Kumi can't make), that one inserted again starts at its defaults, and that Kumi says which settings
  it carried over or asks first, as a way round (c676f407, 4cc13f5a, a17f9f76).
- Quick Sampler: Kumi loaded the sample into Logic's Sampler and ended with Kumi's "Done" alone. The pack now says to
  leave `final` out when doing something other than what was asked and to say what was done instead and why; the case
  takes a refusal or the Sampler way round said as one (c676f407, 51453e05).

## What the model still gets wrong that words only nudge

- It still sets `final: true` on a way round at times (the plug-in case passed two of five runs, each miss different);
  a guard in code would hold where words only nudge.
- Said before a non-final step and again after it, a point gets said twice.
- `fx_params`' `values` was given as an object twice instead of a list, refused and given again.
- "Logic can't drag a plug-in" says it backwards (Logic can; Kumi can't).

## The guide on Apple's site

The guide's welcome page (`support.apple.com/guide/logicpro/welcome/mac`) lists its contents whole (1,975 topics); a
topic is a page of its own, read without the site around it with `?displayMode=headless`. The parser was run over a
dozen of Apple's pages while developing; none are committed. The tool reads the contents once and a topic the first
time an answer needs it, and keeps both in the library folder.
