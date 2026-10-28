# Security policy

## Reporting a vulnerability

Please report security problems privately, not in a public issue.

- **Preferred:** GitHub's private vulnerability reporting. On the repository's
  **Security** tab, choose **Report a vulnerability**.
- **Otherwise:** email sean.true@gmail.com with "pycommonkit security" in the
  subject.

Include the version (`pycommonkit` on PyPI or `commonkit` on crates.io), the
platform, and the smallest steps that show the problem. You should hear back
within a week.

## Supported versions

Only the latest release of each package receives fixes. Both are pre-1.0, so a
fix may come with other changes.

## What is in scope

The store writes, maps and deletes files under a root directory that other
processes share, so the most important reports are any way that:

- the reaper, `remove()` or a publish could unlink, truncate or write a file
  outside the segment layout, or through a symbolic link (contract §9, C6);
- a crafted key, `.npy` header or `.policy.json` could make a reader crash,
  allocate without bound, or read outside a segment;
- one process could change bytes another process has mapped read-only.

Denial of service by filling the shared root is expected behaviour: the store
declines to publish and every caller recomputes. That is the design, not a
vulnerability.
