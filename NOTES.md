# snapRAID Output Notes

## Targeted Output Shapes

The parser targets captured-style `snapraid status` output shaped around:

- A preamble such as `Self test...`, `Loading state from ...`, and memory usage.
- `SnapRAID status report:` followed by a disk table.
- snapRAID 12.x-style disk rows with `Files Fragmented Excess Wasted Used Free Use Name`.
- snapRAID 11.x-style disk rows without the `Wasted` column.
- Array-level scrub lines such as `47% of the array is not scrubbed.` and `The oldest block was scrubbed 12 days ago, the median 6, the newest 0.`
- Sync state lines including `No sync is in progress.` and `WARNING! The array is NOT fully synced.`
- Health lines including `No error detected.`, `WARNING! ...`, and `DANGER! In the array there are N errors!`

For `snapraid diff`, the parser treats the trailing summary block as authoritative:

- `N added`
- `N removed`
- `N updated`
- `N moved`
- `N copied`
- `N restored`

It ignores per-file action lines except where they happen to match the count-summary form.

## Per-Disk Scrub Age

Plain `snapraid status` does not expose per-disk scrub dates. It reports scrub freshness for the array as a whole: the percent not scrubbed plus oldest, median, and newest scrubbed block ages.

snapglass now models that honestly as array-level scrub state. Disk rows retain facts that `snapraid status` actually provides: file counts, fragmentation, excess fragments, used/free space, and utilization.

A true per-disk scrub-age heatmap would need data from somewhere else, such as:

- External tracking of when scrub or sync jobs ran and which disks or block ranges were targeted.
- Another snapRAID command or metadata source that exposes block-to-disk scrub recency.
- A snapglass-maintained history database populated by scheduled runs.

`snapraid status` alone is not that source.

## Remaining Fragility

The status parser is intentionally whitespace-tolerant and keys disk rows by configured disk names, but it still assumes the table row ends with the disk name and that the numeric columns are in a known 11.x or 12.x order. If future snapRAID versions rename columns, add columns before `Name`, or print sizes with unexpected units, fixtures should be added before changing parser behavior.
