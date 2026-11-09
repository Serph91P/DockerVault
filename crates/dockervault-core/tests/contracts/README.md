# Independent filter controls

- `filter-oracle.json` is an unchanged copy of the original240-case R0 Linux
  engine/worker control from revision18d8c3d. The original frozen corpus is not
  modified by this copy.
- `fnmatch-control.json` contains1443 independent Python3.14.10 `fnmatchcase`
  results for malformed brackets, ranges, Unicode, slashes, newlines, repeated
  stars and literal backslashes. It was captured in the unchanged Python control
  image before implementing the Rust matcher, with no application import/startup.

- `fsdecode-control.json` captures original Python filesystem surrogateescape
  matching for raw non-UTF8 names, including bytes resembling UTF-8-encoded
  surrogates and the U+FFFD collision boundary. Expected values stay independent
  of the native matcher.

These JSON files are historical language-neutral expected values, not maintained
Python implementations or evidence that a native test ran. Native assertions
are in `../filters.rs`. Broader character-class qualification and retained
negative samples are recorded in the task's external evidence; the compact
regression for range-removal/negated-empty classes stays in the maintained suite.
