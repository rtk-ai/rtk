# Perl A/B check

This check tests whether the Perl filters keep the facts an agent needs. Two agents answer the
same questions about the same run. One agent reads the raw tool output. The other agent reads
the rtk output. If their answers differ, the rtk filter dropped or changed something that
matters.

## Files

- `sample/` is a small distribution, Acme::RtkSample. Its tests fail on purpose, in nine
  ways: a wrong value, a deep structure difference and a regex mismatch; a Test2 table, a
  `die` after the plan and a compile error; a TODO, a skip and a `BAIL_OUT`. Its modules break
  perlcritic policies on purpose.
- `build-scenarios.sh` runs each tool on a fresh copy of `sample/` and writes the output pairs.
- `questions.md` holds the fixed questions for each scenario.
- `perlcriticrc` is the profile for the perlcritic scenario. It names 15 policies from the Perl::Critic distribution and sets `only = 1`. Add-on policy distributions on the machine do not run, and `~/.perlcriticrc` is not read, so the scenario gives the same result everywhere.

## Build the output pairs

You need rtk, perl and the tools on `PATH`: prove, perlcritic, perldoc, dzil (with the
`@Basic` bundle), Devel::Cover's `cover`, and optionally yath and cpanm. If a tool is missing, the
script skips its scenario and lists it in `skipped.txt`.

```bash
scripts/perl-ab/build-scenarios.sh /tmp/perl-ab
RTK=target/release/rtk scripts/perl-ab/build-scenarios.sh /tmp/perl-ab --network
```

`RTK` selects the rtk binary. `--network` adds the cpanm scenario, which downloads from CPAN.
For each scenario the script writes `<scenario>.raw.txt` (through `rtk proxy`) and
`<scenario>.rtk.txt` (through `rtk`), both with stderr. `exit_codes.txt` records both exit
codes. The two codes must be equal.

## Run the check

1. Start two agents in parallel with the same prompt, except for the file suffix. Agent A reads
   only `*.raw.txt`. Agent B reads only `*.rtk.txt`. Tell both to answer every question in
   `questions.md` for every scenario, from the output alone, and to not read `sample/`.
2. Compare the two sets of answers question by question. You can do this yourself, or give a
   third agent both answer sets.
3. Record each difference, with the scenario and the question.

A difference where agent B has less information, or wrong information, is a filter defect.
Fix the filter before you merge. A difference where agent A is wrong and agent B is correct
is not a defect. Record it anyway, because it shows where the raw output misleads.
