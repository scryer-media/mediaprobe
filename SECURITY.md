# Security policy

## Supported versions

Only the latest published release of `mediaprobe` receives fixes.

## Reporting a vulnerability

Please do not open a public issue for security problems. Report privately
through GitHub at <https://github.com/scryer-media/mediaprobe/security/advisories/new>
(the Security tab, "Report a vulnerability"). You will get an acknowledgement
within a few days, and a fix ships as a new release of the crate, credited to
you unless you ask otherwise.

## What counts

A media parser reads hostile input by design. Reports of any of these are
security reports and are handled as such:

- an out-of-bounds read or write, or any other undefined behaviour, for any
  input file or options;
- a panic, an infinite loop, or memory, reads or work beyond the configured
  budget, reachable from caller-supplied data or options.

Include the file (or how it was generated), the options used, the platform
and the crate version. Never attach private media.
